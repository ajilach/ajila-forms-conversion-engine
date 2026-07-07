//! RGBA image encoding helpers shared by the engine tools.

use std::borrow::Cow;

/// Maximum length (in pixels) of an image's long edge before it is downscaled.
///
/// The Anthropic vision API rejects any image whose long edge exceeds 8000 px
/// (HTTP 400) and downscales anything past ~2576 px server-side on the
/// high-resolution models this agent uses. Page renders are normally split into
/// per-page images that stay well under this, but this clamp is a final safety
/// net for a pathological single page (or a large render scale): capping at the
/// API's own high-res limit costs no fidelity the model would have used and
/// shrinks the request payload.
const MAX_IMAGE_EDGE: u32 = 2576;

/// Downscale `img` so its long edge is at most [`MAX_IMAGE_EDGE`], preserving
/// aspect ratio. Returns the original borrowed when it already fits.
fn clamp_to_max_edge(img: &blueprint::RgbaImage) -> Cow<'_, blueprint::RgbaImage> {
    let (width, height) = img.dimensions();
    let longest = width.max(height);
    if longest <= MAX_IMAGE_EDGE {
        return Cow::Borrowed(img);
    }

    let ratio = f64::from(MAX_IMAGE_EDGE) / f64::from(longest);
    let new_width = ((f64::from(width) * ratio).round() as u32).max(1);
    let new_height = ((f64::from(height) * ratio).round() as u32).max(1);

    Cow::Owned(image::imageops::resize(
        img,
        new_width,
        new_height,
        image::imageops::FilterType::Lanczos3,
    ))
}

/// Encode an RGBA image to PNG bytes.
pub fn encode_rgba_to_png(img: &blueprint::RgbaImage, output: &mut Vec<u8>) -> Result<(), String> {
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let img = clamp_to_max_edge(img);
    let (width, height) = img.dimensions();
    let encoder = PngEncoder::new(output);

    encoder
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG encoding error: {}", e))
}

/// Encode an RGBA image to JPEG bytes at the given quality (1–100).
///
/// JPEG has no alpha channel; the alpha is dropped (rendered form pages are
/// opaque). Far smaller than PNG for page renders, which keeps the AI request
/// payload within provider size limits without sacrificing resolution.
pub fn encode_rgba_to_jpeg(img: &blueprint::RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::jpeg::JpegEncoder;

    let img = clamp_to_max_edge(img);
    let rgb = image::DynamicImage::ImageRgba8(img.into_owned()).into_rgb8();
    let (width, height) = rgb.dimensions();
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, quality)
        .write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG encoding error: {e}"))?;
    Ok(output)
}
