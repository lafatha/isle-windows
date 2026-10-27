//! The assistant's body: eight outlines, twelve palettes and the two eyes.
//!
//! The script handed WPF the `botShapes` geometry strings and a radial brush;
//! here the same strings are filled with the same brush (`Get-BotBrush`'s
//! light/base/shade stops) and the eyes are drawn with each host element's
//! own sizes, exactly as the XAML declared them.

use crate::icons;
use crate::ui::Canvas;

/// The shape keys, in the order the preferences lay their tiles out.
pub const SHAPES: [&str; 8] = [
    "circle", "pebble", "squircle", "capsule", "cloud", "hexagon", "drop", "ghost",
];

/// The colour keys, in the order the preferences lay their swatches out.
pub const COLOURS: [&str; 12] = [
    "cloud",
    "ink",
    "peach",
    "coral",
    "amber",
    "mint",
    "teal",
    "sky",
    "lavender",
    "rose",
    "graphite",
    "eyedropper",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub base: u32,
    pub light: u32,
    pub shade: u32,
    pub eyes: u32,
}

const fn shade(base: u32, light: u32, shade: u32) -> Palette {
    Palette {
        base,
        light,
        shade,
        eyes: 0x121217,
    }
}

/// `"ShapeGhost"`, `"ghost"` and anything unknown all land on one of the eight.
pub fn shape_key(key: &str) -> &'static str {
    let name = key.trim_start_matches("Shape").to_ascii_lowercase();
    SHAPES
        .iter()
        .find(|shape| **shape == name)
        .copied()
        .unwrap_or("circle")
}

/// `"ColSky"`, `"sky"` and the eyedropper, which the script treated as the sky.
pub fn palette(key: &str) -> Palette {
    match key.trim_start_matches("Col").to_ascii_lowercase().as_str() {
        "ink" => Palette {
            base: 0x2A2A33,
            light: 0x595960,
            shade: 0x1B1B25,
            eyes: 0xF5F5FA,
        },
        "peach" => shade(0xFFB48F, 0xFFDDCD, 0xB27E67),
        "coral" => shade(0xFF6F61, 0xFFBEB8, 0xB24E47),
        "amber" => shade(0xFFC94A, 0xFFE7AE, 0xB28D37),
        "mint" => shade(0x63E2AD, 0xB9F2DA, 0x459E7C),
        "teal" => shade(0x2FC6B8, 0xA1E5DF, 0x218B84),
        "sky" | "eyedropper" => shade(0x5EAAFF, 0xB7D9FF, 0x4277B6),
        "lavender" => shade(0xAB8EFF, 0xD9CCFF, 0x7863B6),
        "rose" => shade(0xFF80B8, 0xFFC6DF, 0xB25A84),
        "graphite" => shade(0x8F909C, 0xCDCDD2, 0x646570),
        _ => shade(0xF5F5F8, 0xFBFBFC, 0xBDC2D6),
    }
}

/// The path data behind one shape name, verbatim from the script.
pub fn path_data(name: &str) -> &'static str {
    match name {
        "drop" => icons::SHAPE_DROP,
        "circle" => icons::SHAPE_CIRCLE,
        "pebble" => icons::SHAPE_PEBBLE,
        "ghost" => icons::SHAPE_GHOST,
        "capsule" => icons::SHAPE_CAPSULE,
        "hexagon" => icons::SHAPE_HEXAGON,
        "cloud" => icons::SHAPE_CLOUD,
        "squircle" => icons::SHAPE_SQUIRCLE,
        _ => icons::SHAPE_CIRCLE,
    }
}

/// The five places the script hosted the mascot, each with its own `Path`
/// slot and eye sizes from the XAML.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    /// The compact island: an 18 px path in the 20 px bot border.
    Compact,
    /// A chat message's avatar: a 20 px path in its 22 px grid.
    Avatar,
    /// The chat input row: a 26 px path in its 30 px grid.
    Input,
    /// The preferences preview: a 36 px path in its 38 px grid.
    Preview,
    /// One of the shape tiles: a 28 px path in its 32 px grid.
    Tile,
}

impl Spot {
    /// The `Path Width="..."` the XAML declared, the Stretch="Uniform" box.
    pub fn slot(self) -> f32 {
        match self {
            Spot::Compact => 18.0,
            Spot::Avatar => 20.0,
            Spot::Input => 26.0,
            Spot::Preview => 36.0,
            Spot::Tile => 28.0,
        }
    }

    /// Eye width, height and half-gap, in the host's pixels.
    fn eyes(self, shape: &str) -> (f32, f32, f32) {
        match self {
            Spot::Compact | Spot::Avatar => (2.2, 4.8, 2.2),
            Spot::Input => (3.0, 6.8, 3.0),
            Spot::Preview => (3.5, 8.5, 3.55),
            Spot::Tile => match shape {
                "hexagon" => (2.7, 6.4, 2.7),
                "drop" => (2.5, 6.0, 2.5),
                _ => (2.8, 6.8, 2.8),
            },
        }
    }

    /// How far the face drops from the centre, per shape, per host.
    ///
    /// The face was a centred child carrying a top margin, so WPF laid it
    /// out half that margin below the middle: the compact cloud bot's 1.2
    /// puts its eyes 0.6 under the centre. That is what the `eyerows.ps1`
    /// probe read off `ps-bot2.png` against `rust-bot8.png` — the capture's
    /// eye mass sits 0.75 device pixels lower at 125 %, 0.6 in these units.
    pub fn eye_drop(self, shape: &str) -> f32 {
        let fit = eye_fit(shape);
        let margin = match self {
            // The compact island and the inline avatars took the compact
            // column; the voice views and the preferences preview took the
            // preview column, which the tile XAML hard-codes as well.
            Spot::Compact | Spot::Avatar => fit.compact,
            Spot::Tile => fit.tile,
            Spot::Input | Spot::Preview => fit.preview,
        };
        margin / 2.0
    }
}

/// `$script:botEyeFits`, plus the margins the tile page wrote into its own
/// XAML. The reference's fourth field, `EyeSize`, is never read, so it is not
/// carried over here.
#[derive(Clone, Copy)]
struct EyeFit {
    preview: f32,
    compact: f32,
    tile: f32,
}

fn eye_fit(shape: &str) -> EyeFit {
    match shape {
        "cloud" => EyeFit {
            preview: 2.5,
            compact: 1.2,
            tile: 2.5,
        },
        "drop" => EyeFit {
            preview: 4.5,
            compact: 2.2,
            tile: 4.5,
        },
        "ghost" => EyeFit {
            preview: -1.8,
            compact: -0.9,
            tile: -2.0,
        },
        _ => EyeFit {
            preview: 0.0,
            compact: 0.0,
            tile: 0.0,
        },
    }
}

/// Draws the body in `spot`, centred on `cx`/`cy`, with the eyes at
/// `openness` (1 fully open) and `glance` 0/3 centre, 1 right, 2 left, 4 up,
/// as the script's 2.4 s eye timer picked.
pub fn draw(
    c: &mut Canvas,
    cx: f32,
    mut cy: f32,
    spot: Spot,
    shape: &str,
    colour: &str,
    openness: f32,
    glance: u8,
) {
    let name = shape_key(shape);
    let palette = palette(colour);
    let data = path_data(name);
    let slot = spot.slot();

    // The XAML's root grid carries SnapsToDevicePixels, so WPF pins the
    // compact bot's 20 px border to whole device pixels. At 125% DPI the
    // border lands half a device pixel below the exact centre and the body,
    // glow and eyes all ride along; reproduce that snap here.
    if spot == Spot::Compact {
        let s = c.r.scale;
        cy = ((cy - 10.0) * s).round() / s + 10.0;
    }

    // The compact island's bot wore a soft blue halo: a blurred drop shadow
    // of the silhouette, approximated by the shape at growing sizes.
    if spot == Spot::Compact {
        for (extra, alpha) in [(2.0, 0.10), (4.0, 0.10), (6.0, 0.08), (8.0, 0.06)] {
            c.fill_path(cx, cy, slot + extra, data, 0x5EAFFF, alpha);
        }
    }

    c.radial_path(
        cx,
        cy,
        slot,
        data,
        palette.light,
        palette.base,
        palette.shade,
    );

    let (eye_w, eye_h, gap) = spot.eyes(name);
    let drop = spot.eye_drop(name);
    // The script's blink pulled the eyes down to a 1 px sliver.
    let height = (eye_h * openness).max(1.0);
    // The glance moves both eyes together; the compact bot is the only host
    // the script ever looked around.
    let (gx, gy) = match (spot, glance) {
        (Spot::Compact, 1) => (0.7, 0.0),
        (Spot::Compact, 2) => (-0.7, 0.0),
        (Spot::Compact, 4) => (0.0, -1.0),
        _ => (0.0, 0.0),
    };
    for index in 0..2 {
        let x = cx + if index == 0 { -gap } else { gap } + gx;
        let y = cy + drop + gy;
        c.rect(
            crate::ui::Rect::new(x - eye_w / 2.0, y - height / 2.0, eye_w, height),
            eye_w / 2.0,
            palette.eyes,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks the `M x,y C ... Z` syntax the shape data uses and returns the
    /// bounding box of its anchor and endpoint coordinates.
    fn bounds(data: &str) -> (f32, f32, f32, f32) {
        let mut points: Vec<(f32, f32)> = Vec::new();
        let mut numbers: Vec<f32> = Vec::new();
        let mut command = ' ';
        let flush = |cmd: char, numbers: &mut Vec<f32>, points: &mut Vec<(f32, f32)>| {
            match cmd {
                'M' => {
                    for pair in numbers.chunks_exact(2) {
                        points.push((pair[0], pair[1]));
                    }
                }
                'C' => {
                    for step in numbers.chunks_exact(6) {
                        points.push((step[4], step[5]));
                    }
                }
                _ => {}
            }
            numbers.clear();
        };
        for token in data.split_whitespace() {
            let mut rest = token;
            if let Some(first) = token.chars().next() {
                if first.is_ascii_alphabetic() {
                    flush(command, &mut numbers, &mut points);
                    command = first;
                    rest = &token[first.len_utf8()..];
                    if rest.is_empty() {
                        continue;
                    }
                }
            }
            numbers.extend(
                rest.split(',')
                    .filter_map(|part| part.trim().parse::<f32>().ok()),
            );
        }
        flush(command, &mut numbers, &mut points);
        let mut lo = (f32::MAX, f32::MAX);
        let mut hi = (f32::MIN, f32::MIN);
        for (x, y) in points {
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
        (lo.0, lo.1, hi.0, hi.1)
    }

    #[test]
    fn every_shape_fits_its_hundred_box() {
        for name in SHAPES {
            let (l, t, r, b) = bounds(path_data(name));
            assert!(r > l && b > t, "{name} has no area");
            assert!(
                l >= -2.0 && t >= -2.0 && r <= 102.0 && b <= 102.0,
                "{name} escapes its box: {l} {t} {r} {b}"
            );
        }
    }

    #[test]
    fn the_settings_names_find_their_shape() {
        assert_eq!(shape_key("ShapeGhost"), "ghost");
        assert_eq!(shape_key("ShapeCircle"), "circle");
        assert_eq!(shape_key("capsule"), "capsule");
        // The script fell back to a circle for anything it did not know.
        assert_eq!(shape_key("ShapeBanana"), "circle");
        assert_eq!(shape_key(""), "circle");
    }

    #[test]
    fn the_palettes_carry_the_script_colours() {
        assert_eq!(palette("ColSky").base, 0x5EAAFF);
        assert_eq!(palette("ColEyedropper").base, palette("sky").base);
        assert_eq!(palette("ColInk").eyes, 0xF5F5FA);
        assert_eq!(palette("nonsense").base, 0xF5F5F8);
    }

    #[test]
    fn each_spot_declares_the_xaml_eye_sizes() {
        assert_eq!(Spot::Compact.eyes("circle"), (2.2, 4.8, 2.2));
        assert_eq!(Spot::Avatar.eyes("ghost"), (2.2, 4.8, 2.2));
        assert_eq!(Spot::Input.eyes("circle"), (3.0, 6.8, 3.0));
        assert_eq!(Spot::Preview.eyes("cloud"), (3.5, 8.5, 3.55));
        assert_eq!(Spot::Tile.eyes("drop"), (2.5, 6.0, 2.5));
        assert_eq!(Spot::Tile.eyes("hexagon"), (2.7, 6.4, 2.7));
        assert_eq!(Spot::Tile.eyes("ghost"), (2.8, 6.8, 2.8));
        assert_eq!(Spot::slot(Spot::Compact), 18.0);
        assert_eq!(Spot::slot(Spot::Preview), 36.0);
    }

    #[test]
    fn the_eyes_sit_where_each_shape_wants_them() {
        // The script's fit table, read the way WPF arranged it: a centred
        // face lands half of its top margin below the middle.
        assert_eq!(Spot::Compact.eye_drop("circle"), 0.0);
        assert_eq!(Spot::Compact.eye_drop("cloud"), 0.6);
        assert_eq!(Spot::Compact.eye_drop("drop"), 1.1);
        assert_eq!(Spot::Compact.eye_drop("ghost"), -0.45);
        // The inline avatar and the chat input both used those columns.
        assert_eq!(Spot::Avatar.eye_drop("cloud"), 0.6);
        assert_eq!(Spot::Input.eye_drop("drop"), 2.25);
        // The voice mascots and the preferences preview took the preview
        // column; the ghost's -1.8 there is the one value the tile XAML
        // disagreed with, keeping -2.
        assert_eq!(Spot::Preview.eye_drop("cloud"), 1.25);
        assert_eq!(Spot::Preview.eye_drop("drop"), 2.25);
        assert_eq!(Spot::Preview.eye_drop("ghost"), -0.9);
        assert_eq!(Spot::Tile.eye_drop("cloud"), 1.25);
        assert_eq!(Spot::Tile.eye_drop("drop"), 2.25);
        assert_eq!(Spot::Tile.eye_drop("ghost"), -1.0);
        assert_eq!(Spot::Tile.eye_drop("hexagon"), 0.0);
    }

    #[test]
    fn the_compact_cloud_face_sits_where_the_capture_has_it() {
        // The PowerShell capture put the compact bot's eye mass 0.75 device
        // pixels under the body's centre at the 125 % this machine runs;
        // the script's compact cloud margin is what produces it.
        let s = 1.25f32;
        assert!((Spot::Compact.eye_drop("cloud") * s - 0.75).abs() < 1e-6);
        // A shape with no margin stays on the centre.
        assert_eq!(Spot::Compact.eye_drop("pebble") * s, 0.0);
    }

    #[test]
    fn the_compact_bot_snaps_to_the_device_grid_like_wpf() {
        // The XAML's SnapsToDevicePixels pins the compact bot's 20 px border
        // to whole device pixels. At 125% DPI its top at 7.5 device snaps to
        // 8, so the centre lands 0.4 DIP below the slot centre, exactly where
        // the PowerShell capture put the eyes and the body rim.
        let s = 1.25f32;
        let cy = 16.0f32;
        let snapped = ((cy - 10.0) * s).round() / s + 10.0;
        assert!((snapped - 16.4).abs() < 1e-6);
        // At 100% the border already sits on the grid: no shift.
        assert_eq!(((16.0f32 - 10.0) * 1.0).round() / 1.0 + 10.0, 16.0);
    }
}
