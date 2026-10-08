use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use lightcraft_raster::Rgba8;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

pub fn encode(img: &Rgba8, quality: u8) -> Result<Vec<u8>, String> {
    if img.width == 0
        || img.height == 0
        || img.width > u16::MAX as usize
        || img.height > u16::MAX as usize
    {
        return Err(format!("cannot encode {}x{}", img.width, img.height));
    }
    let mut rgb = Vec::with_capacity(img.width * img.height * 3);
    for px in &img.data {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, quality.clamp(1, 100));
    // 4:4:4 so a viewport edge that is odd still encodes.
    enc.set_sampling_factor(SamplingFactor::R_4_4_4);
    enc.encode(&rgb, img.width as u16, img.height as u16, ColorType::Rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.is_empty() {
        return Err("empty jpeg".into());
    }
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let px = dec.decode().map_err(|e| e.to_string())?;
    let info = dec.info().ok_or("jpeg has no header")?;
    let (w, h) = (info.width as usize, info.height as usize);
    if px.len() < w * h * 3 {
        return Err("jpeg was short".into());
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for chunk in px.chunks_exact(3).take(w * h) {
        rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
    }
    Ok((u32::from(info.width), u32::from(info.height), rgba))
}

pub fn mean_luma(rgba: &[u8]) -> f64 {
    if rgba.len() < 4 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut n = 0.0;
    for px in rgba.chunks_exact(4) {
        sum += 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
        n += 1.0;
    }
    sum / n
}

pub fn mean_channels(rgba: &[u8]) -> (f64, f64, f64) {
    let mut r = 0.0;
    let mut g = 0.0;
    let mut b = 0.0;
    let mut n = 0.0;
    for px in rgba.chunks_exact(4) {
        r += px[0] as f64;
        g += px[1] as f64;
        b += px[2] as f64;
        n += 1.0;
    }
    if n == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    (r / n, g / n, b / n)
}

pub fn write_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    Ok(())
}
