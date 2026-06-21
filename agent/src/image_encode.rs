//! RGBA image encoding helpers shared by the engine tools.

/// Encode an RGBA image to PNG bytes.
pub fn encode_rgba_to_png(img: &blueprint::RgbaImage, output: &mut Vec<u8>) -> Result<(), String> {
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

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

    let rgb = image::DynamicImage::ImageRgba8(img.clone()).into_rgb8();
    let (width, height) = rgb.dimensions();
    let mut output = Vec::new();
    JpegEncoder::new_with_quality(&mut output, quality)
        .write_image(rgb.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG encoding error: {e}"))?;
    Ok(output)
}
