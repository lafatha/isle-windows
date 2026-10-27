//! Album art. WinRT hands the sleeve over as an encoded stream; Direct2D wants
//! premultiplied BGRA at the size it will be drawn.
//!
//! The decoder does the scaling as well as the colour conversion, so a 1400 px
//! cover never becomes a 7 MB buffer just to be drawn at 112 px.

use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

/// The largest sleeve that is worth keeping: the biggest box on screen is 112 px
/// at 100 %, so 192 leaves room for a high-DPI display without waste.
const MAX_EDGE: u32 = 192;

/// Encoded bytes in, premultiplied BGRA out. `None` for anything the decoder
/// cannot make sense of, which leaves the drawn placeholder in place.
pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if bytes.is_empty() {
        return None;
    }
    let stream = InMemoryRandomAccessStream::new().ok()?;
    let writer = DataWriter::CreateDataWriter(&stream).ok()?;
    writer.WriteBytes(bytes).ok()?;
    writer.StoreAsync().ok()?.get().ok()?;
    writer.FlushAsync().ok()?.get().ok()?;
    writer.DetachStream().ok()?;
    stream.Seek(0).ok()?;

    let decoder = BitmapDecoder::CreateAsync(&stream).ok()?.get().ok()?;
    let (width, height) = (decoder.PixelWidth().ok()?, decoder.PixelHeight().ok()?);
    let (out_w, out_h) = fit(width, height, MAX_EDGE);

    let transform = BitmapTransform::new().ok()?;
    if out_w != width || out_h != height {
        transform.SetScaledWidth(out_w).ok()?;
        transform.SetScaledHeight(out_h).ok()?;
    }
    let data = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .ok()?
        .get()
        .ok()?;
    let pixels = data.DetachPixelData().ok()?.to_vec();
    if pixels.len() < (out_w * out_h * 4) as usize {
        return None;
    }
    Some((out_w, out_h, pixels))
}

/// The size a `width`x`height` image shrinks to so neither edge passes `edge`.
fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= edge {
        return (width.max(1), height.max(1));
    }
    let scale = edge as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_sleeves_keep_their_size() {
        assert_eq!(fit(112, 112, 192), (112, 112));
        // A tall sleeve is capped on its long edge, not squashed to a square.
        assert_eq!(fit(60, 600, 192), (19, 192));
    }

    #[test]
    fn large_sleeves_shrink_to_the_cap_without_losing_the_shape() {
        assert_eq!(fit(1400, 1400, 192), (192, 192));
        let (w, h) = fit(1000, 500, 192);
        assert_eq!(w, 192);
        assert_eq!(h, 96);
    }

    #[test]
    fn an_empty_buffer_is_not_decoded() {
        assert!(decode(&[]).is_none());
    }
}
