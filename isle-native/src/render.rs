//! The frame: a premultiplied 32-bpp DIB drawn on with Direct2D, handed to
//! Windows with `UpdateLayeredWindow`. Pixels nobody painted stay at alpha 0,
//! which is what makes the rest of the host window click-through.
//!
//! The software rasterizer is deliberate. The default render target type binds
//! a D3D device and the vendor's user-mode driver - tens of megabytes of code
//! for under a megabyte of pixels.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows::core::{w, Interface, Result};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
    D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

/// A straight-alpha colour. The GDI DC render target unpremultiplies brush
/// colours itself and premultiplies the pixels it writes back, so passing the
/// raw channels here makes `alpha < 1` draws land at exactly WPF's `Opacity`.
pub fn rgb(hex: u32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: alpha.clamp(0.0, 1.0),
    }
}

/// A colour key for the brush cache: the same hex and alpha share a brush.
fn key(hex: u32, alpha: f32) -> u64 {
    ((hex as u64) << 8) | (alpha.clamp(0.0, 1.0) * 255.0).round() as u64
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FormatKey {
    size: u32,
    weight: i32,
    align: u8,
    wrap: bool,
    line: u32,
    /// 0 is the interface face, 1 the monospace the command card asked for.
    family: u8,
}

/// A shaped text run. Position is deliberately not part of the key: only the
/// string, the format and the box it is laid out in change the glyphs, so a
/// label that slides as the island grows reuses the same layout instead of
/// reshaping its text every frame.
#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    size: u32,
    weight: i32,
    align: u8,
    wrap: bool,
    line: u32,
    family: u8,
    width: u32,
    height: u32,
}

/// A layout extent, in tenths of a logical pixel; the open-ended extent the
/// measurer asks for collapses to a single shared sentinel.
fn extent(value: f32) -> u32 {
    if value >= 1.0e30 {
        u32::MAX
    } else {
        (value.max(0.0) * 10.0) as u32
    }
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    pub dc_target: ID2D1DCRenderTarget,
    pub target: ID2D1RenderTarget,
    mem_dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pub width_px: i32,
    pub height_px: i32,
    pub scale: f32,
    brushes: RefCell<HashMap<u64, ID2D1SolidColorBrush>>,
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    layouts: RefCell<HashMap<LayoutKey, IDWriteTextLayout>>,
    paths: RefCell<HashMap<String, ID2D1PathGeometry>>,
    inside: RefCell<bool>,
    /// How many `PushAxisAlignedClip` calls the target still owes a `Pop`.
    /// Direct2D keeps the clip stack on the render target across `EndDraw` and
    /// `BeginDraw`, so an unbalanced push is not discarded at the frame
    /// boundary - it accumulates and quietly eats the picture. The count lives
    /// here, on the long-lived renderer, so `begin` can always reset the target
    /// to a clean slate even if one frame ever leaves a clip unclosed.
    clip_depth: Cell<u32>,
}

impl Renderer {
    pub fn new(width_px: i32, height_px: i32, scale: f32) -> Result<Self> {
        unsafe {
            let screen = GetDC(None);
            let mem_dc = CreateCompatibleDC(screen);

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width_px,
                    biHeight: -height_px, // top-down: row 0 is the top of the screen
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(mem_dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let previous = SelectObject(mem_dc, HGDIOBJ(bitmap.0));
            ReleaseDC(None, screen);

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let dc_target = factory.CreateDCRenderTarget(&props)?;
            let target: ID2D1RenderTarget = dc_target.cast()?;
            let rect = RECT {
                left: 0,
                top: 0,
                right: width_px,
                bottom: height_px,
            };
            dc_target.BindDC(mem_dc, &rect)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            Ok(Self {
                factory,
                dwrite,
                dc_target,
                target,
                mem_dc,
                bitmap,
                previous,
                width_px,
                height_px,
                scale,
                brushes: RefCell::new(HashMap::new()),
                formats: RefCell::new(HashMap::new()),
                layouts: RefCell::new(HashMap::new()),
                paths: RefCell::new(HashMap::new()),
                inside: RefCell::new(false),
                clip_depth: Cell::new(0),
            })
        }
    }

    /// A colour that is used over and over (white, the greys, the blacks) lives
    /// in a brush that is made once; anything one-off shares the cache too,
    /// which tops out at a few dozen entries.
    pub fn solid(&self, hex: u32, alpha: f32) -> Result<ID2D1SolidColorBrush> {
        let key = key(hex, alpha);
        if let Some(brush) = self.brushes.borrow().get(&key) {
            return Ok(brush.clone());
        }
        let color = rgb(hex, alpha);
        let brush = unsafe { self.target.CreateSolidColorBrush(&color, None)? };
        self.brushes.borrow_mut().insert(key, brush.clone());
        Ok(brush)
    }

    /// A vertical linear-gradient brush, in the same logical pixels the rest of
    /// the frame draws in, for the given colour stops (`0xRRGGBB` and an alpha
    /// each) run from `top` to `bottom`. Used to wash the lyric page in the
    /// sleeve's own colour.
    pub fn linear_gradient(
        &self,
        top: f32,
        bottom: f32,
        stops: &[(u32, f32)],
    ) -> Result<ID2D1LinearGradientBrush> {
        use windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F;
        let mapped: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .enumerate()
            .map(|(index, (hex, alpha))| D2D1_GRADIENT_STOP {
                position: if stops.len() < 2 {
                    0.0
                } else {
                    index as f32 / (stops.len() as f32 - 1.0)
                },
                color: rgb(*hex, *alpha),
            })
            .collect();
        unsafe {
            let collection =
                self.target
                    .CreateGradientStopCollection(&mapped, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            let brush = self.target.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: D2D_POINT_2F { x: 0.0, y: top },
                    endPoint: D2D_POINT_2F { x: 0.0, y: bottom },
                },
                None,
                &collection,
            )?;
            Ok(brush)
        }
    }

    /// Text formats are immutable once made, so one per size/weight/alignment.
    pub fn format(
        &self,
        size: f32,
        weight: i32,
        align: u8,
        wrap: bool,
        line: f32,
    ) -> Result<IDWriteTextFormat> {
        self.format_family(size, weight, align, wrap, line, 0)
    }

    /// The script's `Consolas, monospace`, for the command card.
    pub fn mono(&self, size: f32, weight: i32) -> Result<IDWriteTextFormat> {
        self.format_family(size, weight, 0, false, 0.0, 1)
    }

    /// How wide a run of text is, which is what a row of right-aligned items
    /// needs to know to sit beside each other rather than on top.
    pub fn measure(&self, value: &str, size: f32, weight: i32) -> f32 {
        // A rough average cap width, used only if the layout cannot be made.
        let guess = value.chars().count() as f32 * size * 0.5;
        let Ok(layout) = self.layout(value, size, weight, 0, false, 0.0, 0, f32::MAX, f32::MAX)
        else {
            return guess;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            if layout.GetMetrics(&mut metrics).is_err() {
                return guess;
            }
        }
        metrics.width
    }

    /// A shaped text run, cached by everything that shapes it. `DrawText` builds
    /// a fresh layout on every call, which is the one thing an expanded page
    /// does not need repeated sixty times a second; this hands back the same
    /// immutable layout whenever the string, format and box are unchanged, so
    /// the caller can place it with `DrawTextLayout` at the moving origin.
    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &self,
        value: &str,
        size: f32,
        weight: i32,
        align: u8,
        wrap: bool,
        line: f32,
        family: u8,
        width: f32,
        height: f32,
    ) -> Result<IDWriteTextLayout> {
        let key = LayoutKey {
            text: value.to_string(),
            size: (size * 10.0) as u32,
            weight,
            align,
            wrap,
            line: (line * 10.0) as u32,
            family,
            width: extent(width),
            height: extent(height),
        };
        if let Some(known) = self.layouts.borrow().get(&key) {
            return Ok(known.clone());
        }
        let format = if family == 1 {
            self.mono(size, weight)?
        } else {
            self.format(size, weight, align, wrap, line)?
        };
        let units: Vec<u16> = value.encode_utf16().collect();
        let layout =
            unsafe { self.dwrite.CreateTextLayout(&units, &format, width, height)? };
        // Pages that stream text (the lyric sheet, the clipboard) would grow
        // this without bound; a few thousand runs is more than any frame needs,
        // and starting over is cheap and rare.
        let mut cache = self.layouts.borrow_mut();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(key, layout.clone());
        Ok(layout)
    }

    fn format_family(
        &self,
        size: f32,
        weight: i32,
        align: u8,
        wrap: bool,
        line: f32,
        family: u8,
    ) -> Result<IDWriteTextFormat> {
        let key = FormatKey {
            size: (size * 10.0) as u32,
            weight,
            align,
            wrap,
            line: (line * 10.0) as u32,
            family,
        };
        if let Some(format) = self.formats.borrow().get(&key) {
            return Ok(format.clone());
        }
        let name = if family == 1 {
            w!("Consolas")
        } else {
            w!("Segoe UI")
        };
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                name,
                None,
                DWRITE_FONT_WEIGHT(weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?
        };
        unsafe {
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetTextAlignment(match align {
                1 => DWRITE_TEXT_ALIGNMENT_CENTER,
                2 => DWRITE_TEXT_ALIGNMENT_TRAILING,
                _ => DWRITE_TEXT_ALIGNMENT_LEADING,
            })?;
            format.SetWordWrapping(if wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            })?;
            if line > 0.0 {
                format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line, size * 0.8)?;
            }
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            format.SetTrimming(&trimming, &sign)?;
        }
        self.formats.borrow_mut().insert(key, format.clone());
        Ok(format)
    }

    /// Starts a frame: rebind, draw in logical units, clear to transparent.
    pub fn begin(&self) -> Result<()> {
        unsafe {
            // Guarantee a clean clip stack every frame. The count is the
            // source of truth: if a previous frame ever left a `Push` unclosed
            // (an early return between a clip and its unclip), the target would
            // otherwise carry it forward and the picture would shrink a little
            // more each repaint until it vanished - the "it froze" people saw on
            // the list pages. Popping back to zero here makes that impossible.
            while self.clip_depth.get() > 0 {
                self.target.PopAxisAlignedClip();
                self.clip_depth.set(self.clip_depth.get() - 1);
            }
            let rect = RECT {
                left: 0,
                top: 0,
                right: self.width_px,
                bottom: self.height_px,
            };
            self.dc_target.BindDC(self.mem_dc, &rect)?;
            self.target.SetDpi(96.0, 96.0);
            let scale = Matrix3x2 {
                M11: self.scale,
                M12: 0.0,
                M21: 0.0,
                M22: self.scale,
                M31: 0.0,
                M32: 0.0,
            };
            self.target.SetTransform(&scale);
            self.target.BeginDraw();
            self.target.Clear(None);
        }
        *self.inside.borrow_mut() = true;
        Ok(())
    }

    pub fn end(&self) -> Result<()> {
        *self.inside.borrow_mut() = false;
        unsafe { self.target.EndDraw(None, None) }
    }

    /// Push an axis-aligned clip and remember it, so the stack can be balanced
    /// (or unwound at the next frame) by count rather than by a lone flag.
    pub fn push_clip(&self, clip: &D2D_RECT_F) {
        unsafe { self.target.PushAxisAlignedClip(clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        self.clip_depth.set(self.clip_depth.get() + 1);
    }

    /// Pop one clip if any is outstanding; a stray call is a harmless no-op.
    pub fn pop_clip(&self) {
        let depth = self.clip_depth.get();
        if depth > 0 {
            self.clip_depth.set(depth - 1);
            unsafe { self.target.PopAxisAlignedClip() };
        }
    }

    /// A bitmap from premultiplied BGRA pixels, for album art.
    pub fn bitmap(&self, width: u32, height: u32, pixels: &[u8]) -> Option<ID2D1Bitmap> {
        if width == 0 || height == 0 || pixels.len() < (width * height * 4) as usize {
            return None;
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe {
            let bitmap = self
                .target
                .CreateBitmap(
                    D2D_SIZE_U { width, height },
                    Some(pixels.as_ptr().cast()),
                    width * 4,
                    &props,
                )
                .ok()?;
            Some(bitmap)
        }
    }

    /// Draws an image into a rounded box; the corners are cut by a geometric
    /// mask so the art keeps the same radius as the script's Border.
    pub fn image(&self, bitmap: &ID2D1Bitmap, x: f32, y: f32, size: f32, radius: f32) {
        self.image_box(bitmap, x, y, size, size, radius);
    }

    /// The same cover-fill, but into a `w`x`h` box: the largest centred region
    /// of the source matching the box's aspect ratio is mapped onto it, so a
    /// wide thumbnail crops top and bottom rather than squashing.
    pub fn image_box(&self, bitmap: &ID2D1Bitmap, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        let source = unsafe { bitmap.GetSize() };
        let crop = if source.width > 0.0 && source.height > 0.0 && w > 0.0 && h > 0.0 {
            let target_ratio = w / h;
            let src_ratio = source.width / source.height;
            let (cw, ch) = if src_ratio > target_ratio {
                (source.height * target_ratio, source.height)
            } else {
                (source.width, source.width / target_ratio)
            };
            let left = (source.width - cw) / 2.0;
            let top = (source.height - ch) / 2.0;
            Some(D2D_RECT_F {
                left,
                top,
                right: left + cw,
                bottom: top + ch,
            })
        } else {
            None
        };
        let mask = self
            .rounded_geometry(x, y, w, h, radius)
            .ok()
            .and_then(|geometry| geometry.cast::<ID2D1Geometry>().ok());
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            geometricMask: std::mem::ManuallyDrop::new(mask),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            },
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe {
            let Ok(layer) = self.target.CreateLayer(None) else {
                return;
            };
            self.target.PushLayer(&params, &layer);
            let target_rect = D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            };
            let _ = self.target.DrawBitmap(
                bitmap,
                Some(&target_rect),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                crop.as_ref().map(|rect| rect as *const D2D_RECT_F),
            );
            let _ = self.target.PopLayer();
        }
    }

    /// Hands the DIB to Windows; alpha 0 stays click-through.
    pub fn present(&self, hwnd: HWND) -> Result<()> {
        unsafe {
            let screen = GetDC(None);
            let size = SIZE {
                cx: self.width_px,
                cy: self.height_px,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let result = UpdateLayeredWindow(
                hwnd,
                screen,
                None,
                Some(&size),
                self.mem_dc,
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen);
            result
        }
    }

    /// The notch outline: flat top, two shoulder curves, rounded bottom corners.
    /// The same path the script's `BuildNotchGeometry` produced, offset to `x`.
    pub fn notch_geometry_at(
        &self,
        x: f32,
        w: f32,
        h: f32,
        r: f32,
        s: f32,
    ) -> Result<ID2D1PathGeometry> {
        let total = w + 2.0 * s;
        let radius = r.min((h - s).max(0.0)).min(w / 2.0);
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let arc = |ax: f32, ay: f32, size: f32, sweep| {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: D2D_POINT_2F { x: x + ax, y: ay },
                    size: D2D_SIZE_F {
                        width: size,
                        height: size,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: sweep,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            };
            sink.BeginFigure(D2D_POINT_2F { x, y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(D2D_POINT_2F {
                x: x + total,
                y: 0.0,
            });
            arc(total - s, s, s, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE);
            sink.AddLine(D2D_POINT_2F {
                x: x + total - s,
                y: h - radius,
            });
            arc(
                total - s - radius,
                h,
                radius,
                D2D1_SWEEP_DIRECTION_CLOCKWISE,
            );
            sink.AddLine(D2D_POINT_2F {
                x: x + s + radius,
                y: h,
            });
            arc(s, h - radius, radius, D2D1_SWEEP_DIRECTION_CLOCKWISE);
            sink.AddLine(D2D_POINT_2F { x: x + s, y: s });
            arc(0.0, 0.0, s, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// Builds a path geometry out of SVG path data: the `M`, `L`, `H`, `V`,
    /// `C`, `S`, `Q`, `T`, `A` and `Z` commands, upper or lower case, which is
    /// the exact subset the script's XAML `Path` elements carried.
    pub fn svg_path(&self, data: &str) -> Result<ID2D1PathGeometry> {
        // The icon and shape paths are constant strings painted over and over.
        // Re-tokenising the text and re-tessellating a brand-new geometry every
        // frame was the heaviest thing on the UI thread, so the built geometry
        // is cached by its source text and cloned (a cheap COM add-ref) after.
        if let Some(known) = self.paths.borrow().get(data) {
            return Ok(known.clone());
        }
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let mut tokens = SvgTokens::new(data);
            let mut start = (0.0f32, 0.0f32);
            let mut current = (0.0f32, 0.0f32);
            let mut last_ctrl: Option<(f32, f32)> = None;
            let mut open = false;

            while let Some(cmd) = tokens.command() {
                let relative = cmd.is_ascii_lowercase();
                let cmd = cmd.to_ascii_uppercase();
                if cmd == 'M' {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    let p = svg_point(&mut tokens, current, relative);
                    sink.BeginFigure(D2D_POINT_2F { x: p.0, y: p.1 }, D2D1_FIGURE_BEGIN_FILLED);
                    start = p;
                    current = p;
                    open = true;
                    // Any coordinate pairs after an `M` are implicit lines.
                    while tokens.peek_number() {
                        let p = svg_point(&mut tokens, current, relative);
                        sink.AddLine(D2D_POINT_2F { x: p.0, y: p.1 });
                        current = p;
                    }
                    continue;
                }
                if !open {
                    sink.BeginFigure(
                        D2D_POINT_2F {
                            x: current.0,
                            y: current.1,
                        },
                        D2D1_FIGURE_BEGIN_FILLED,
                    );
                    start = current;
                    open = true;
                }
                match cmd {
                    'L' => loop {
                        let p = svg_point(&mut tokens, current, relative);
                        sink.AddLine(D2D_POINT_2F { x: p.0, y: p.1 });
                        current = p;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'H' => loop {
                        let mut v = tokens.number();
                        if relative {
                            v += current.0;
                        }
                        sink.AddLine(D2D_POINT_2F { x: v, y: current.1 });
                        current.0 = v;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'V' => loop {
                        let mut v = tokens.number();
                        if relative {
                            v += current.1;
                        }
                        sink.AddLine(D2D_POINT_2F { x: current.0, y: v });
                        current.1 = v;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'C' => loop {
                        let c1 = svg_point(&mut tokens, current, relative);
                        let c2 = svg_point(&mut tokens, current, relative);
                        let end = svg_point(&mut tokens, current, relative);
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                            point1: D2D_POINT_2F { x: c1.0, y: c1.1 },
                            point2: D2D_POINT_2F { x: c2.0, y: c2.1 },
                            point3: D2D_POINT_2F { x: end.0, y: end.1 },
                        });
                        last_ctrl = Some(c2);
                        current = end;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'S' => loop {
                        let c1 = svg_reflect(last_ctrl.unwrap_or(current), current);
                        let c2 = svg_point(&mut tokens, current, relative);
                        let end = svg_point(&mut tokens, current, relative);
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                            point1: D2D_POINT_2F { x: c1.0, y: c1.1 },
                            point2: D2D_POINT_2F { x: c2.0, y: c2.1 },
                            point3: D2D_POINT_2F { x: end.0, y: end.1 },
                        });
                        last_ctrl = Some(c2);
                        current = end;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'Q' => loop {
                        let c = svg_point(&mut tokens, current, relative);
                        let end = svg_point(&mut tokens, current, relative);
                        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                            point1: D2D_POINT_2F { x: c.0, y: c.1 },
                            point2: D2D_POINT_2F { x: end.0, y: end.1 },
                        });
                        last_ctrl = Some(c);
                        current = end;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'T' => loop {
                        let c = svg_reflect(last_ctrl.unwrap_or(current), current);
                        let end = svg_point(&mut tokens, current, relative);
                        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                            point1: D2D_POINT_2F { x: c.0, y: c.1 },
                            point2: D2D_POINT_2F { x: end.0, y: end.1 },
                        });
                        last_ctrl = Some(c);
                        current = end;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'A' => loop {
                        let rx = tokens.number();
                        let ry = tokens.number();
                        let rotation = tokens.number();
                        let large = tokens.number() != 0.0;
                        let sweep = tokens.number() != 0.0;
                        let end = svg_point(&mut tokens, current, relative);
                        if rx <= 0.0 || ry <= 0.0 {
                            sink.AddLine(D2D_POINT_2F { x: end.0, y: end.1 });
                        } else if (end.0 - current.0).abs() > f32::EPSILON
                            || (end.1 - current.1).abs() > f32::EPSILON
                        {
                            sink.AddArc(&D2D1_ARC_SEGMENT {
                                point: D2D_POINT_2F { x: end.0, y: end.1 },
                                size: D2D_SIZE_F {
                                    width: rx,
                                    height: ry,
                                },
                                rotationAngle: rotation,
                                sweepDirection: if sweep {
                                    D2D1_SWEEP_DIRECTION_CLOCKWISE
                                } else {
                                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                                },
                                arcSize: if large {
                                    D2D1_ARC_SIZE_LARGE
                                } else {
                                    D2D1_ARC_SIZE_SMALL
                                },
                            });
                        }
                        current = end;
                        if !tokens.peek_number() {
                            break;
                        }
                    },
                    'Z' => {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                        current = start;
                    }
                    _ => {}
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close()?;
            self.paths
                .borrow_mut()
                .insert(data.to_string(), geometry.clone());
            Ok(geometry)
        }
    }

    /// Draws one of the script's exact SVG paths at a centre point, stretched
    /// the way WPF's `Path Stretch="Uniform"` stretched it: the geometry
    /// bounds are scaled to fit `size`, centred, and the stroke stays at
    /// `thickness` in output pixels (WPF divided the pen by the stretch
    /// scale). `angle` rotates the finished icon, as a WPF `RotateTransform`
    /// did; with `fill` and a thickness both set, the shape is filled and
    /// stroked.
    #[allow(clippy::too_many_arguments)]
    pub fn path_icon(
        &self,
        cx: f32,
        cy: f32,
        size: f32,
        data: &str,
        hex: u32,
        fill: bool,
        thickness: f32,
        round: bool,
        angle: f32,
    ) {
        let Ok(geometry) = self.svg_path(data) else {
            return;
        };
        let Ok(shape) = geometry.cast::<ID2D1Geometry>() else {
            return;
        };
        unsafe {
            let Ok(bounds) = geometry.GetBounds(None) else {
                return;
            };
            let w = bounds.right - bounds.left;
            let h = bounds.bottom - bounds.top;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let scale = (size / w).min(size / h);
            let stretch = Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: cx - (bounds.left + w / 2.0) * scale,
                M32: cy - (bounds.top + h / 2.0) * scale,
            };
            let radians = angle.to_radians();
            let (sin, cos) = radians.sin_cos();
            let rotate = Matrix3x2 {
                M11: cos,
                M12: sin,
                M21: -sin,
                M22: cos,
                M31: 0.0,
                M32: 0.0,
            };
            let to_c = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: cx,
                M32: cy,
            };
            let from_c = Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: -cx,
                M32: -cy,
            };
            let local = mul(&mul(&to_c, &rotate), &mul(&from_c, &stretch));
            let mut base = Matrix3x2::default();
            self.target.GetTransform(&mut base);
            let combined = mul(&local, &base);
            self.target.SetTransform(&combined);
            let Ok(brush) = self.solid(hex, 1.0) else {
                self.target.SetTransform(&base);
                return;
            };
            if fill {
                let _ = self.target.FillGeometry(&shape, &brush, None);
            }
            if thickness > 0.0 {
                let width = thickness / scale;
                if round {
                    let style = self.round_stroke();
                    let _ = self
                        .target
                        .DrawGeometry(&shape, &brush, width, Some(&style));
                } else {
                    let _ = self.target.DrawGeometry(&shape, &brush, width, None);
                }
            }
            self.target.SetTransform(&base);
        }
    }

    /// Fills `data` as WPF's `Path Stretch="Uniform"` did inside a `size`
    /// box centred on `cx`/`cy`, in one solid colour at `alpha`. The compact
    /// island's bot halo draws this in layers.
    pub fn fill_path(&self, cx: f32, cy: f32, size: f32, data: &str, hex: u32, alpha: f32) {
        let Ok(geometry) = self.svg_path(data) else {
            return;
        };
        let Ok(shape) = geometry.cast::<ID2D1Geometry>() else {
            return;
        };
        unsafe {
            let Ok(bounds) = geometry.GetBounds(None) else {
                return;
            };
            let w = bounds.right - bounds.left;
            let h = bounds.bottom - bounds.top;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let scale = (size / w).min(size / h);
            let stretch = Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: cx - (bounds.left + w / 2.0) * scale,
                M32: cy - (bounds.top + h / 2.0) * scale,
            };
            let mut base = Matrix3x2::default();
            self.target.GetTransform(&mut base);
            self.target.SetTransform(&mul(&stretch, &base));
            let Ok(brush) = self.solid(hex, alpha) else {
                self.target.SetTransform(&base);
                return;
            };
            let _ = self.target.FillGeometry(&shape, &brush, None);
            self.target.SetTransform(&base);
        }
    }

    /// Fills `data` inside a `size` box centred on `cx`/`cy` with the
    /// assistant's radial brush: `light` at the gradient origin, `base`
    /// mid-radius and `shade` at the rim, the three stops `Get-BotBrush`
    /// composed. The brush maps to the whole box, as WPF's bounding-box
    /// mapping did, not to the stretched path. `alpha` fades the whole body.
    #[allow(clippy::too_many_arguments)]
    pub fn radial_path(
        &self,
        cx: f32,
        cy: f32,
        size: f32,
        data: &str,
        light: u32,
        base: u32,
        shade: u32,
        alpha: f32,
    ) {
        if alpha <= 0.001 {
            return;
        }
        let Ok(geometry) = self.svg_path(data) else {
            return;
        };
        let Ok(shape) = geometry.cast::<ID2D1Geometry>() else {
            return;
        };
        unsafe {
            let Ok(bounds) = geometry.GetBounds(None) else {
                return;
            };
            let w = bounds.right - bounds.left;
            let h = bounds.bottom - bounds.top;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let scale = (size / w).min(size / h);
            let stretch = Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: cx - (bounds.left + w / 2.0) * scale,
                M32: cy - (bounds.top + h / 2.0) * scale,
            };
            // Bake the placement into the geometry and leave the target
            // transform at the DPI scale: the DC render target composes
            // brush transforms with the world transform at fill time, so a
            // brush used while the stretch was the world transform got
            // double-stretched and its gradient missed the body. Defined in
            // DIP space, the brush now only picks up the DPI scale, exactly
            // like WPF's box-relative gradient.
            let Ok(placed) = self.factory.CreateTransformedGeometry(&shape, &stretch) else {
                return;
            };
            let Ok(placed) = placed.cast::<ID2D1Geometry>() else {
                return;
            };
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: rgb(light, alpha),
                },
                D2D1_GRADIENT_STOP {
                    position: 0.45,
                    color: rgb(base, alpha),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: rgb(shade, alpha),
                },
            ];
            let Ok(collection) = self.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            // Centre (0.38, 0.32) and origin (0.32, 0.28) of the box, radius
            // 0.82 of it: the brush sits at the box origin with unit radius
            // and the transform carries it there.
            let Ok(brush) = self.target.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: D2D_POINT_2F { x: 0.0, y: 0.0 },
                    gradientOriginOffset: D2D_POINT_2F {
                        x: -0.06 / 0.82,
                        y: -0.04 / 0.82,
                    },
                    radiusX: 1.0,
                    radiusY: 1.0,
                },
                None,
                &collection,
            ) else {
                return;
            };
            let radius = 0.82 * size;
            let _ = brush.SetTransform(&Matrix3x2 {
                M11: radius,
                M12: 0.0,
                M21: 0.0,
                M22: radius,
                M31: cx - 0.12 * size,
                M32: cy - 0.18 * size,
            });
            let _ = self.target.FillGeometry(&placed, &brush, None);
        }
    }

    /// A stroke with round caps and joins, as the script's chevrons asked for.
    fn round_stroke(&self) -> ID2D1StrokeStyle {
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_FLAT,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        unsafe { self.factory.CreateStrokeStyle(&props, None).unwrap() }
    }

    /// A polygon, filled. Used for the little triangles of transport controls.
    pub fn polygon(&self, points: &[(f32, f32)]) -> Result<ID2D1PathGeometry> {
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let Some((first, rest)) = points.split_first() else {
                return Ok(geometry);
            };
            sink.BeginFigure(
                D2D_POINT_2F {
                    x: first.0,
                    y: first.1,
                },
                D2D1_FIGURE_BEGIN_FILLED,
            );
            for point in rest {
                sink.AddLine(D2D_POINT_2F {
                    x: point.0,
                    y: point.1,
                });
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// A rounded rectangle used as a mask, for the album art corners.
    pub fn rounded_geometry(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
    ) -> Result<ID2D1RoundedRectangleGeometry> {
        let rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.factory.CreateRoundedRectangleGeometry(&rounded) }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
        }
    }
}

/// One point of an SVG path, absolute or relative to the cursor.
fn svg_point(t: &mut SvgTokens, current: (f32, f32), relative: bool) -> (f32, f32) {
    let x = t.number();
    let y = t.number();
    if relative {
        (current.0 + x, current.1 + y)
    } else {
        (x, y)
    }
}

/// The control point an `S` or `T` command reflects across the cursor.
fn svg_reflect(control: (f32, f32), current: (f32, f32)) -> (f32, f32) {
    (2.0 * current.0 - control.0, 2.0 * current.1 - control.1)
}

/// `a * b`: apply `b` to a point first, then `a`, the order D2D uses.
fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

/// Walks SVG path data: commands are letters, numbers may run together with
/// their sign (`1.5-2.15` is 1.5 then -2.15) and commas count as separators.
struct SvgTokens<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SvgTokens<'a> {
    fn new(data: &'a str) -> Self {
        Self {
            data: data.as_bytes(),
            pos: 0,
        }
    }

    fn skip(&mut self) {
        while let Some(c) = self.data.get(self.pos) {
            if c.is_ascii_whitespace() || *c == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// The next command letter, if the cursor is on one.
    fn command(&mut self) -> Option<char> {
        self.skip();
        let c = *self.data.get(self.pos)?;
        if c.is_ascii_alphabetic() {
            self.pos += 1;
            Some(c as char)
        } else {
            None
        }
    }

    /// Whether a number begins at the cursor, for implicit lineto repeats.
    fn peek_number(&mut self) -> bool {
        self.skip();
        matches!(
            self.data.get(self.pos),
            Some(c) if c.is_ascii_digit() || matches!(c, b'.' | b'+' | b'-')
        )
    }

    fn number(&mut self) -> f32 {
        self.skip();
        let start = self.pos;
        if matches!(self.data.get(self.pos), Some(&b'+') | Some(&b'-')) {
            self.pos += 1;
        }
        let mut seen_dot = false;
        while let Some(c) = self.data.get(self.pos) {
            if c.is_ascii_digit() {
                self.pos += 1;
            } else if *c == b'.' && !seen_dot {
                seen_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.data[start..self.pos]).unwrap_or("0");
        text.parse().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod pixel_probe {
    use super::*;
    use windows::Win32::Graphics::Gdi::GetPixel;

    /// Sum of a COLORREF's channels: 0x00bbggrr.
    fn brightness(pixel: COLORREF) -> u32 {
        (pixel.0 & 0xFF) + ((pixel.0 >> 8) & 0xFF) + ((pixel.0 >> 16) & 0xFF)
    }

    /// The compact island's bot body must paint. This walks every step
    /// `radial_path` takes so the first one to fail names itself, then checks
    /// the pixels actually landed in the DIB.
    #[test]
    fn radial_path_paints_the_cloud_body() {
        let renderer = Renderer::new(240, 80, 1.0).expect("renderer");
        renderer.begin().expect("begin");

        let geometry = renderer
            .svg_path(crate::icons::SHAPE_CLOUD)
            .expect("svg_path");
        let bounds = unsafe { geometry.GetBounds(None) }.expect("GetBounds");
        assert!(
            bounds.right - bounds.left > 0.0 && bounds.bottom - bounds.top > 0.0,
            "cloud geometry is empty"
        );

        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: rgb(0xFBFBFC, 1.0),
            },
            D2D1_GRADIENT_STOP {
                position: 0.45,
                color: rgb(0xF5F5F8, 1.0),
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: rgb(0xBDC2D6, 1.0),
            },
        ];
        let collection = unsafe {
            renderer.target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )
        }
        .expect("gradient stop collection");
        let _brush = unsafe {
            renderer.target.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: D2D_POINT_2F { x: 0.0, y: 0.0 },
                    gradientOriginOffset: D2D_POINT_2F {
                        x: -0.06 / 0.82,
                        y: -0.04 / 0.82,
                    },
                    radiusX: 1.0,
                    radiusY: 1.0,
                },
                None,
                &collection,
            )
        }
        .expect("radial gradient brush");

        renderer.radial_path(
            60.0,
            40.0,
            18.0,
            crate::icons::SHAPE_CLOUD,
            0xFBFBFC,
            0xF5F5F8,
            0xBDC2D6,
            1.0,
        );
        renderer.end().expect("end");

        let centre = unsafe { GetPixel(renderer.mem_dc, 60, 40) };
        assert!(
            brightness(centre) > 400,
            "the body centre stayed dark: {:#010X}",
            centre.0
        );
    }

    /// The exact compact-island draw at the display scale the machine runs
    /// (1.25): an opaque notch, the content clip, then `bot::draw` where
    /// `pages::compact` puts it. The body must end up in the DIB.
    ///
    /// The scenarios below bisect the factors: the display scale, the clip,
    /// the notch fill and the full `bot::draw` (glow + body + eyes).
    #[test]
    fn compact_bot_paints_at_display_scale() {
        use crate::ui::{Canvas, Rect, INK};
        use windows::core::Interface;

        // A: the raw body draw at 1.25 scale, nothing else.
        {
            let renderer = Renderer::new(240, 80, 1.25).expect("renderer");
            renderer.begin().expect("begin");
            renderer.radial_path(
                60.0,
                40.0,
                18.0,
                crate::icons::SHAPE_CLOUD,
                0xFBFBFC,
                0xF5F5F8,
                0xBDC2D6,
                1.0,
            );
            renderer.end().expect("end");
            // Scan the whole DIB for the brightest pixel, so a misplaced body
            // names itself rather than staying a silent miss.
            let mut best = (0u32, 0i32, 0i32);
            let mut row = String::new();
            for y in 0..80 {
                for x in 0..240 {
                    let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                    let b = brightness(pixel);
                    if b > best.0 {
                        best = (b, x, y);
                    }
                    row.push(if b > 500 {
                        '#'
                    } else if b > 300 {
                        '*'
                    } else if b > 150 {
                        ':'
                    } else if b > 40 {
                        '.'
                    } else {
                        ' '
                    });
                }
                if y % 4 == 0 {
                    println!("{row}");
                    row.clear();
                }
            }
            println!(
                "A brightest pixel: {:#010X} at ({},{})",
                best.0, best.1, best.2
            );
            // 1:1 map of the body region with a column ruler, so the gradient's
            // light spot can be read off exactly.
            for y in 30..70 {
                let mut line = String::new();
                for x in 55..100 {
                    let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                    let b = brightness(pixel);
                    line.push(if b > 650 {
                        '#'
                    } else if b > 500 {
                        '*'
                    } else if b > 300 {
                        ':'
                    } else if b > 80 {
                        '.'
                    } else {
                        ' '
                    });
                }
                println!("{y:02} {line}");
            }
            println!(
                "     {:<45}",
                "|....|....|....|....|....|....|....|....|....|"
            );
            assert!(
                best.0 > 400,
                "A: body at 1.25 scale stayed dark; brightest pixel {:#010X} at ({},{})",
                best.0,
                best.1,
                best.2
            );
        }

        // B: notch + clip + bot::draw at 1.0 scale.
        {
            let renderer = Renderer::new(800, 290, 1.0).expect("renderer");
            renderer.begin().expect("begin");
            let body = Rect::new((800.0 - 252.0) / 2.0, 0.0, 252.0, 32.0);
            let shape = renderer
                .notch_geometry_at(body.x, 232.0, 32.0, 16.0, 10.0)
                .expect("notch");
            let brush = renderer.solid(INK, 1.0).expect("ink");
            unsafe {
                let geometry = shape.cast::<ID2D1Geometry>().expect("cast");
                let _ = renderer.target.FillGeometry(&geometry, &brush, None);
            }
            let mut canvas = Canvas::new(&renderer);
            let area = Rect::new(body.x + 10.0, 0.0, 232.0, 32.0);
            canvas.clip(area);
            let slot = Rect::new(area.x + 10.0, area.h / 2.0 - 12.0, 24.0, 24.0);
            crate::bot::draw(
                &mut canvas,
                slot.x + 12.0,
                area.h / 2.0,
                crate::bot::Spot::Compact,
                "cloud",
                "ColCloud",
                1.0,
                0,
            );
            canvas.unclip();
            renderer.end().expect("end");
            let centre = unsafe { GetPixel(renderer.mem_dc, 306, 16) };
            assert!(
                brightness(centre) > 400,
                "B: clip+notch at 1.0 stayed dark: {:#010X}",
                centre.0
            );
        }

        // C: everything, at 1.25 scale - the app's exact conditions.
        {
            let renderer = Renderer::new(1000, 363, 1.25).expect("renderer");
            renderer.begin().expect("begin");
            let body = Rect::new((800.0 - 252.0) / 2.0, 0.0, 252.0, 32.0);
            let shape = renderer
                .notch_geometry_at(body.x, 232.0, 32.0, 16.0, 10.0)
                .expect("notch");
            let brush = renderer.solid(INK, 1.0).expect("ink");
            unsafe {
                let geometry = shape.cast::<ID2D1Geometry>().expect("cast");
                let _ = renderer.target.FillGeometry(&geometry, &brush, None);
            }
            let mut canvas = Canvas::new(&renderer);
            let area = Rect::new(body.x + 10.0, 0.0, 232.0, 32.0);
            canvas.clip(area);
            let slot = Rect::new(area.x + 10.0, area.h / 2.0 - 12.0, 24.0, 24.0);
            crate::bot::draw(
                &mut canvas,
                slot.x + 12.0,
                area.h / 2.0,
                crate::bot::Spot::Compact,
                "cloud",
                "ColCloud",
                1.0,
                0,
            );
            canvas.unclip();
            renderer.end().expect("end");
            let centre = unsafe { GetPixel(renderer.mem_dc, 382, 20) };
            assert!(
                brightness(centre) > 400,
                "C: clip+notch at 1.25 stayed dark: {:#010X}",
                centre.0
            );
            // The WPF brush put the light stop up-left of the body centre and
            // the shade at the rim: the body's own pixels must show the
            // gradient mapped to the box, not to the screen origin.
            let tl = unsafe { GetPixel(renderer.mem_dc, 378, 14) };
            let br = unsafe { GetPixel(renderer.mem_dc, 391, 24) };
            println!(
                "C body corners: top-left {:#010X} bottom-right {:#010X}",
                tl.0, br.0
            );
            assert!(
                brightness(tl) > 700,
                "C: the light stop missed the body's top-left: {:#010X}",
                tl.0
            );
            assert!(
                brightness(br) < 700 && brightness(br) > 400,
                "C: the rim is not shaded: {:#010X}",
                br.0
            );
            // The eyes sit at cx +/- 2.2, cy + 1.2 and must punch dark holes
            // through the body.
            let eye = unsafe { GetPixel(renderer.mem_dc, 380, 21) };
            println!("C eye pixel: {:#010X}", eye.0);
            assert!(
                brightness(eye) < 200,
                "C: the eye did not draw: {:#010X}",
                eye.0
            );
        }

        // Bisect what makes the gradient go flat: the clip, the notch fill,
        // the glow layers, each alone in front of the same body draw.
        let variants: [(&str, bool, bool, bool); 4] = [
            ("clip only", true, false, false),
            ("notch only", false, true, false),
            ("glow only", false, false, true),
            ("all", true, true, true),
        ];
        for (name, clip, notch, glow) in variants {
            let renderer = Renderer::new(1000, 363, 1.25).expect("renderer");
            renderer.begin().expect("begin");
            let body = Rect::new((800.0 - 252.0) / 2.0, 0.0, 252.0, 32.0);
            if notch {
                let shape = renderer
                    .notch_geometry_at(body.x, 232.0, 32.0, 16.0, 10.0)
                    .expect("notch");
                let brush = renderer.solid(INK, 1.0).expect("ink");
                unsafe {
                    let geometry = shape.cast::<ID2D1Geometry>().expect("cast");
                    let _ = renderer.target.FillGeometry(&geometry, &brush, None);
                }
            }
            let canvas = Canvas::new(&renderer);
            let area = Rect::new(body.x + 10.0, 0.0, 232.0, 32.0);
            if clip {
                canvas.clip(area);
            }
            let cx = body.x + 32.0;
            if glow {
                for (extra, alpha) in [(2.0, 0.10), (4.0, 0.10), (6.0, 0.08), (8.0, 0.06)] {
                    canvas.fill_path(
                        cx,
                        16.0,
                        18.0 + extra,
                        crate::icons::SHAPE_CLOUD,
                        0x5EAFFF,
                        alpha,
                    );
                }
            }
            renderer.radial_path(
                cx,
                16.0,
                18.0,
                crate::icons::SHAPE_CLOUD,
                0xFBFBFC,
                0xF5F5F8,
                0xBDC2D6,
                1.0,
            );
            if clip {
                canvas.unclip();
            }
            renderer.end().expect("end");
            let tl = unsafe { GetPixel(renderer.mem_dc, 378, 14) };
            let mut best = (0u32, 0i32, 0i32);
            for y in 0..60 {
                for x in 300..460 {
                    let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                    let b = brightness(pixel);
                    if b > best.0 {
                        best = (b, x, y);
                    }
                }
            }
            println!(
                "{name}: top-left {:#010X}, brightest {:#010X} at ({},{})",
                tl.0, best.0, best.1, best.2
            );
        }
    }

    /// The probe `eyerows.ps1` ran over a pair of bot captures: eleven
    /// columns across the eyes, one darkness sum per row, a pixel's darkness
    /// being `1 - (r + g + b) / 749`. The script read `ps-bot2.png` and
    /// `rust-bot8.png` this way to see where the eyes had landed.
    ///
    /// `f64` throughout, because the script's accumulator was a
    /// `System.Double`: every term is the same IEEE operation in the same
    /// order, so the sums below come out bit-identical to the script's
    /// rather than merely close.
    fn row_darkness(renderer: &Renderer, x0: i32, x1: i32, y0: i32, y1: i32) -> Vec<f64> {
        // `GetPixel` answers CLR_INVALID outside the bitmap, which would sum
        // as 765 and read as negative darkness - a number the script never
        // produced, because it threw instead. Fail the same way it did, and
        // only for a window that samples anything at all.
        if x0 <= x1 && y0 <= y1 {
            assert!(
                x0 >= 0 && y0 >= 0 && x1 < renderer.width_px && y1 < renderer.height_px,
                "the probe asked for pixels outside the {}x{} bitmap",
                renderer.width_px,
                renderer.height_px
            );
        }
        (y0..=y1)
            .map(|y| {
                (x0..=x1)
                    .map(|x| {
                        let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                        1.0 - (brightness(pixel) as f64 / 749.0)
                    })
                    .sum()
            })
            .collect()
    }

    /// `rows`' darkness-weighted centre, the y the eye mass sits on. A window
    /// with no mass at all centres on its own middle rather than on NaN.
    fn row_centre(rows: &[f64], first: i32) -> f64 {
        let mut weight = 0.0;
        let mut moment = 0.0;
        for (index, row) in rows.iter().enumerate() {
            weight += row;
            moment += row * (first + index as i32) as f64;
        }
        if weight <= 0.0 {
            return first as f64 + (rows.len().saturating_sub(1)) as f64 / 2.0;
        }
        moment / weight
    }

    /// The compact island's bot, drawn where `pages::compact` puts it: an
    /// opaque notch, the content clip, then the cloud body and its eyes.
    fn compact_bot_at_display_scale(scale: f32) -> Renderer {
        use crate::ui::{Canvas, Rect, INK};
        use windows::core::Interface;

        let renderer =
            Renderer::new((800.0 * scale) as i32, (290.0 * scale) as i32, scale).expect("renderer");
        renderer.begin().expect("begin");
        let body = Rect::new((800.0 - 252.0) / 2.0, 0.0, 252.0, 32.0);
        let shape = renderer
            .notch_geometry_at(body.x, 232.0, 32.0, 16.0, 10.0)
            .expect("notch");
        let brush = renderer.solid(INK, 1.0).expect("ink");
        unsafe {
            let geometry = shape.cast::<ID2D1Geometry>().expect("cast");
            let _ = renderer.target.FillGeometry(&geometry, &brush, None);
        }
        let mut canvas = Canvas::new(&renderer);
        let area = Rect::new(body.x + 10.0, 0.0, 232.0, 32.0);
        canvas.clip(area);
        let slot = Rect::new(area.x + 10.0, area.h / 2.0 - 12.0, 24.0, 24.0);
        crate::bot::draw(
            &mut canvas,
            slot.x + 12.0,
            area.h / 2.0,
            crate::bot::Spot::Compact,
            "cloud",
            "ColCloud",
            1.0,
            0,
        );
        canvas.unclip();
        renderer.end().expect("end");
        renderer
    }

    /// The eye rows of the port, read the way `eyerows.ps1` read the two
    /// captures: eleven columns across the face, one darkness sum per row.
    /// Over the same columns the script summed
    ///
    /// ```text
    ///        y=12   13    14    15    16    17    18   (19    20)
    ///   ps      0.93  3.29  4.96  5.17  5.19  4.81  2.70  (0.87  0.85)
    ///   rust    1.10  4.50  5.14  5.16  5.18  4.64  1.43  (0.46  0.63)
    /// ```
    ///
    /// so the capture's eye mass sat most of a row lower than the port's:
    /// the compact face carried the script's 1.2 margin there, and the port
    /// left it at 0. The rows 19/20 in brackets are the body's rim, not the
    /// eyes.
    ///
    /// With the margin in place the port sums
    ///
    /// ```text
    ///        y=18   19    20    21    22    23    24
    ///     2.03  4.83  5.15  5.17  5.20  4.46  0.90
    /// ```
    /// and its eye mass lands on row 21.2: 0.70 device pixels, 0.56 DIP,
    /// under the body's own centre. That is the shift the reference's 1.2
    /// margin asks for, where the port used to leave the face dead on the
    /// centre; the live window measures the same 21.2.
    #[test]
    fn compact_eye_rows_land_where_the_capture_has_them() {
        let scale = 1.25;
        let renderer = compact_bot_at_display_scale(scale);

        // WPF snapped the 20 px border to whole device pixels, so the body
        // centre sits at 16.4 DIP and the face's margin puts the eyes at
        // 17.0: rows 20.5 and 21.25.
        let body_centre = ((16.0 - 10.0) * scale).round() / scale + 10.0;
        let body_row = body_centre * scale;
        let eye_row = (body_centre + crate::bot::Spot::Compact.eye_drop("cloud")) * scale;

        // Twelve columns centred on the face, twelve rows around it.
        let rows = row_darkness(&renderer, 377, 388, 16, 27);
        let centre = row_centre(&rows, 16);
        let body_row = body_row as f64;
        let eye_row = eye_row as f64;
        assert!(
            (centre - eye_row).abs() < 0.3,
            "the eye mass sits at row {centre:.2}, not the capture's {eye_row:.2}"
        );
        // The port used to leave the face on the body's own centre.
        assert!(
            centre - body_row > 0.5,
            "the eye mass still sits on the body centre, {:.2} off",
            centre - body_row
        );

        // Both captures show five rows of eye, and the port must too.
        let deep = rows.iter().filter(|row| **row > 3.0).count();
        assert_eq!(deep, 5, "the face is not the height the captures show");
    }

    /// Writes `0xRRGGBB` cells into the DIB at (`x0`, `y0`), straight into its
    /// memory, so `GetPixel` hands the probe back exactly those bytes.
    fn put_pixels<const N: usize, const M: usize>(
        renderer: &Renderer,
        x0: usize,
        y0: usize,
        rows: &[[u32; N]; M],
    ) {
        use windows::Win32::Graphics::Gdi::{GetObjectW, BITMAP};
        unsafe {
            let mut bmp = BITMAP::default();
            let layout = GetObjectW(
                HGDIOBJ(renderer.bitmap.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bmp as *mut _ as *mut core::ffi::c_void),
            );
            assert!(layout > 0 && !bmp.bmBits.is_null(), "the DIB has no memory");
            assert!(
                x0 + N <= bmp.bmWidth as usize && y0 + M <= bmp.bmHeight as usize,
                "{N}x{M} at ({x0},{y0}) does not fit the {}x{} DIB",
                bmp.bmWidth,
                bmp.bmHeight
            );
            let stride = bmp.bmWidthBytes as usize;
            let base = bmp.bmBits as *mut u8;
            for (row, cells) in rows.iter().enumerate() {
                for (col, hex) in cells.iter().enumerate() {
                    let at = (y0 + row) * stride + (x0 + col) * 4;
                    *base.add(at) = (*hex & 0xFF) as u8; // blue
                    *base.add(at + 1) = ((*hex >> 8) & 0xFF) as u8; // green
                    *base.add(at + 2) = ((*hex >> 16) & 0xFF) as u8; // red
                    *base.add(at + 3) = 0xFF;
                }
            }
        }
    }

    /// `ps-bot2.png`, rows 8..=20 by columns 12..=22, read out of
    /// `System.Drawing` exactly as `GetPixel` saw them: the input the script
    /// summed. One `0xRRGGBB` per cell, no alpha (the whole block is 255).
    const PS_EYE_PIXELS: [[u32; 11]; 13] = [
        [
            0xF9F9FA, 0xF9F9FA, 0xF9F9FA, 0xF8F8FA, 0xF8F8FA, 0xF7F7FA, 0xF7F7F9, 0xF6F6F9,
            0xF6F6F8, 0xF4F4F8, 0xF1F1F6,
        ],
        [
            0xF9F9FB, 0xF9F9FB, 0xF9F9FB, 0xF9F9FB, 0xF9F9FA, 0xF8F8FA, 0xF7F7FA, 0xF6F6F9,
            0xF6F6F9, 0xF5F5F8, 0xF2F2F7,
        ],
        [
            0xFAFAFB, 0xFAFAFC, 0xFAFAFB, 0xF9F9FB, 0xF9F9FA, 0xF8F8FA, 0xF7F7FA, 0xF7F7F9,
            0xF6F6F9, 0xF6F6F8, 0xF3F3F7,
        ],
        [
            0xFAFAFB, 0xFAFAFB, 0xFAFAFB, 0xF9F9FB, 0xF9F9FA, 0xF8F8FA, 0xF7F7FA, 0xF7F7F9,
            0xF6F6F9, 0xF6F6F8, 0xF3F3F7,
        ],
        [
            0xF9F9FB, 0xF4F4F5, 0xBCBCBE, 0xCFCFD1, 0xF8F8FA, 0xF8F8FA, 0xF7F7FA, 0xD8D8DB,
            0xB6B6B9, 0xE9E9EC, 0xF3F3F7,
        ],
        [
            0xF8F8FA, 0xBBBBBD, 0x48484C, 0x5D5D61, 0xEFEFF1, 0xF7F7FA, 0xF1F1F4, 0x6C6C71,
            0x444448, 0xA8A8AD, 0xF2F2F6,
        ],
        [
            0xF7F7FA, 0x78787B, 0x121217, 0x141419, 0xE0E0E2, 0xF7F7F9, 0xE2E2E5, 0x19191E,
            0x121217, 0x6D6D72, 0xF0F0F5,
        ],
        [
            0xF6F6F9, 0x68686C, 0x121217, 0x121217, 0xD9D9DD, 0xF6F6F9, 0xD9D9DC, 0x121217,
            0x121217, 0x65656A, 0xEDEDF3,
        ],
        [
            0xF6F6F9, 0x68686C, 0x121217, 0x121217, 0xD9D9DC, 0xF5F5F8, 0xD8D8DC, 0x121217,
            0x121217, 0x646469, 0xEAEAF1,
        ],
        [
            0xF5F5F8, 0x8B8B8E, 0x121217, 0x19191E, 0xE3E3E6, 0xF4F4F7, 0xE4E4E8, 0x27272C,
            0x121217, 0x737379, 0xE6E7EF,
        ],
        [
            0xF2F2F6, 0xCFCFD3, 0x6E6E73, 0x7F7F85, 0xEEEEF2, 0xF0F0F5, 0xEEEEF3, 0x8D8E94,
            0x66676D, 0xB2B3BB, 0xE1E3EC,
        ],
        [
            0xECEDF3, 0xEDEDF3, 0xDADAE0, 0xE4E4EA, 0xECECF2, 0xEAEBF1, 0xE8E9F0, 0xE1E2EA,
            0xCECFD7, 0xE0E2EB, 0xDDDFE9,
        ],
        [
            0xE6E7EF, 0xE7E8F0, 0xE7E8F0, 0xE7E8EF, 0xE6E7EF, 0xE4E5EE, 0xE3E4ED, 0xE1E2EC,
            0xDEE0EA, 0xDCDEE8, 0xD9DBE7,
        ],
    ];

    /// What `eyerows.ps1` printed for those rows, at the full `System.Double`
    /// precision of its accumulator (`ToString('R')`), rounded to three
    /// decimals in the script's own table as 0.095 0.072 0.055 0.056 0.931
    /// 3.287 4.959 5.168 5.191 4.810 2.701 0.870 0.849.
    const PS_EYE_ROWS: [f64; 13] = [
        0.094793057409879977,
        0.0720961281708945,
        0.054739652870493982,
        0.056074766355140193,
        0.930574098798398,
        3.2870493991989318,
        4.9586114819759679,
        5.1682242990654208,
        5.1909212283044051,
        4.8104138851802416,
        2.7009345794392527,
        0.87049399198931909,
        0.84913217623497983,
    ];

    /// The one test of parity: the script's own capture, the script's own
    /// window (`x` 12..=22, `y` 8..=20), the script's own formula - first as
    /// arithmetic over the captured bytes, then through the port's DIB and
    /// `GetPixel`. Both must reproduce the script's doubles exactly, not
    /// approximately.
    #[test]
    fn the_probe_reproduces_the_script_bit_for_bit() {
        assert_eq!(PS_EYE_PIXELS.len(), 13, "`for ($y = 8; $y -le 20; $y++)`");
        for row in PS_EYE_PIXELS {
            assert_eq!(row.len(), 11, "`for ($x = 12; $x -le 22; $x++)`");
        }

        // `$sum = 0.0`, then `$sum += 1.0 - (($c.R + $c.G + $c.B) / 749.0)`.
        for (index, cells) in PS_EYE_PIXELS.iter().enumerate() {
            let mut sum = 0.0f64;
            for hex in cells {
                let (r, g, b) = (hex >> 16 & 0xFF, hex >> 8 & 0xFF, hex & 0xFF);
                sum += 1.0 - ((r + g + b) as f64 / 749.0);
            }
            assert_eq!(
                sum,
                PS_EYE_ROWS[index],
                "row {} differs from the script's double",
                8 + index
            );
        }

        // The same bytes, sampled the way the port samples a render.
        let renderer = Renderer::new(240, 80, 1.0).expect("renderer");
        put_pixels(&renderer, 12, 8, &PS_EYE_PIXELS);
        let probed = row_darkness(&renderer, 12, 22, 8, 20);
        assert_eq!(probed.len(), 13, "the row range has to be the script's");
        for (index, row) in probed.iter().enumerate() {
            assert_eq!(
                *row,
                PS_EYE_ROWS[index],
                "row {} differs from the script through the DIB",
                8 + index
            );
        }
    }

    /// The whole of the script's `Rows2($path)`: the capture read off disk
    /// through the app's own decoder, then the script's window and formula
    /// over its pixels. It must reach the same doubles as the script did, so
    /// the decoder agrees with `System.Drawing` pixel for pixel here. The
    /// check returns quietly when the capture is not on disk, since that is
    /// a build artefact rather than a fixture.
    #[test]
    fn the_probe_reads_the_capture_file_like_rows2_did() {
        let path = std::path::Path::new("target/ps-bot2.png");
        let Ok(bytes) = std::fs::read(path) else {
            println!("{} is not there to read", path.display());
            return;
        };
        let (width, height, pixels) = crate::art::decode(&bytes).expect("the capture decodes");
        assert!(
            width > 22 && height > 20,
            "the capture is {width}x{height}, too small for the window"
        );
        let mut rows = Vec::new();
        for y in 8..=20u32 {
            let mut sum = 0.0f64;
            for x in 12..=22u32 {
                // Premultiplied BGRA, and the capture is opaque throughout.
                let at = ((y * width + x) * 4) as usize;
                let (b, g, r) = (
                    pixels[at] as u32,
                    pixels[at + 1] as u32,
                    pixels[at + 2] as u32,
                );
                sum += 1.0 - ((r + g + b) as f64 / 749.0);
            }
            rows.push(sum);
        }
        assert_eq!(
            rows, PS_EYE_ROWS,
            "the file did not read as the script read it"
        );
    }

    /// A one-pixel probe: the channel arithmetic has to be `.NET`'s
    /// `R + G + B` over the low three bytes of the COLORREF, never the
    /// reserved byte.
    #[test]
    fn a_single_pixel_sums_as_the_script_did() {
        let renderer = Renderer::new(4, 4, 1.0).expect("renderer");
        put_pixels(&renderer, 1, 1, &[[0x121217]]);
        let rows = row_darkness(&renderer, 1, 1, 1, 1);
        // One column instead of eleven, and one row: 1 - (18 + 18 + 23) / 749.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], 1.0 - (0x12 + 0x12 + 0x17) as f64 / 749.0);
        // White is the script's -0.021 darkness, not a clamped 0: the same
        // number the capture's 749 divisor produces for a luminous pixel.
        put_pixels(&renderer, 2, 1, &[[0xFFFFFF]]);
        assert_eq!(row_darkness(&renderer, 2, 2, 1, 1)[0], 1.0 - 765.0 / 749.0);
    }

    /// Degenerate windows behave the way the script's loops did, and a blank
    /// window does not centre on NaN.
    #[test]
    fn the_probe_handles_empty_windows() {
        let renderer = Renderer::new(40, 40, 1.0).expect("renderer");
        // `for ($x = 12; $x -le 11; $x++)` never runs: `$sum` stays 0.0.
        let rows = row_darkness(&renderer, 12, 11, 8, 20);
        assert_eq!(rows.len(), 13);
        assert!(rows.iter().all(|row| *row == 0.0), "{rows:?}");
        // `for ($y = 20; $y -le 8; $y++)` never runs: `$out` stays empty.
        assert!(row_darkness(&renderer, 12, 22, 20, 8).is_empty());
        // No mass, so the centre falls back to the middle of the window.
        let centre = row_centre(&rows, 8);
        assert_eq!(centre, 14.0, "8..=20 centres on 14");
        assert_eq!(row_centre(&[], 0), 0.0);
        // A column window past the right edge samples nothing, so the script
        // never called GetPixel and never threw; the rows stay at 0.0.
        let outside = row_darkness(&renderer, 500, 400, 0, 4);
        assert_eq!(outside, vec![0.0; 5]);
    }

    /// `GetPixel` cannot answer outside the bitmap, and the script threw when
    /// asked; the port must not quietly sum CLR_INVALID into a negative row.
    #[test]
    #[should_panic(expected = "outside the")]
    fn the_probe_refuses_pixels_off_the_bitmap() {
        let renderer = Renderer::new(40, 40, 1.0).expect("renderer");
        row_darkness(&renderer, 0, 40, 0, 10);
    }

    /// A big body far from the origin, where the two brush-space readings
    /// land far apart: DIP-space puts the light spot at the body's own
    /// top-left, device-space leaves the body flat shade with the light
    /// spot near the DIP coordinates themselves. The 1:1 map and the rim
    /// samples show exactly which one the DC target picked.
    #[test]
    fn gradient_maps_to_the_body_box() {
        let renderer = Renderer::new(400, 120, 1.25).expect("renderer");
        renderer.begin().expect("begin");
        let geometry = renderer
            .svg_path(crate::icons::SHAPE_CIRCLE)
            .expect("svg_path");
        let bounds = unsafe { geometry.GetBounds(None) }.expect("bounds");
        println!(
            "circle bounds: ({:.2},{:.2})-({:.2},{:.2})",
            bounds.left, bounds.top, bounds.right, bounds.bottom
        );
        renderer.radial_path(
            200.0,
            40.0,
            40.0,
            crate::icons::SHAPE_CIRCLE,
            0xFBFBFC,
            0xF5F5F8,
            0xBDC2D6,
            1.0,
        );
        renderer.end().expect("end");
        let mut best = (0u32, 0i32, 0i32);
        for y in 0..120 {
            for x in 0..400 {
                let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                let b = brightness(pixel);
                if b > best.0 {
                    best = (b, x, y);
                }
            }
        }
        println!(
            "gradient light spot: {:#010X} at ({},{})",
            best.0, best.1, best.2
        );
        // Rim and centre samples: centre + N/E/S/W points of the circle.
        for (name, x, y) in [
            ("centre", 250, 50),
            ("left", 226, 50),
            ("top", 250, 26),
            ("right", 274, 50),
            ("bottom", 250, 74),
        ] {
            let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
            println!(
                "{name} ({x},{y}): {:#010X} b={}",
                pixel.0,
                brightness(pixel)
            );
        }
        // 1:1 map of the body region, 10-pixel ruler.
        println!("map x 205..295, y 15..85: #>650 *>500 :>300 .>100");
        for y in 15..85 {
            let mut row = String::new();
            for x in 205..295 {
                let pixel = unsafe { GetPixel(renderer.mem_dc, x, y) };
                let b = brightness(pixel);
                row.push(if b > 650 {
                    '#'
                } else if b > 500 {
                    '*'
                } else if b > 300 {
                    ':'
                } else if b > 100 {
                    '.'
                } else {
                    ' '
                });
            }
            println!("{row}");
        }
        // The centre must be lit: at 1.25 the brush must follow the body.
        let centre = unsafe { GetPixel(renderer.mem_dc, 250, 50) };
        assert!(
            brightness(centre) > 650,
            "body centre is not lit: {:#010X}",
            centre.0
        );
    }

    /// A translucent fill must land at its alpha, not at full strength: the
    /// DIB the layered window composites must carry colour × alpha.
    #[test]
    fn translucent_fill_blends_at_its_alpha() {
        let renderer = Renderer::new(240, 80, 1.0).expect("renderer");
        renderer.begin().expect("begin");
        renderer.fill_path(60.0, 40.0, 18.0, crate::icons::SHAPE_CIRCLE, 0x5EAFFF, 0.10);
        renderer.end().expect("end");
        let centre = unsafe { GetPixel(renderer.mem_dc, 60, 40) };
        let b = brightness(centre);
        // 0x5EAFFF at 10% over transparent: colour × alpha = (9, 17, 25) = 51,
        // not the full-strength blue (94, 170, 255) = 519 and not alpha².
        assert!(
            b > 15 && b < 150,
            "glow at 0.10 alpha landed at brightness {b}: {:#010X}",
            centre.0
        );
    }
}
