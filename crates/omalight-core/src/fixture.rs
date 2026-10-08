use lightcraft_raw::{
    write_dng, BlackLevel, Cfa, ColorData, DngCompression, DngWriteOptions, Mat3, Orientation,
    RawData, RawFormat, RawImage, Rect,
};
use std::fs;
use std::path::{Path, PathBuf};

/// A small Bayer DNG the LightCraft decoder accepts. It is generated, not a camera file, so the
/// tests do not ship someone else's raw. Sony ARW goes through the same engine after decode.
pub fn write_fixture(dir: &Path, width: usize, height: usize) -> Result<PathBuf, String> {
    if width < 32 || height < 32 {
        return Err("fixture needs at least 32 px on each edge".into());
    }
    let bytes = encode(width, height)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("omalight-{width}x{height}.dng"));
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn encode(width: usize, height: usize) -> Result<Vec<u8>, String> {
    let cfa = Cfa::bayer("RGGB").ok_or("RGGB")?;
    let black = 512.0f32;
    let white = 14000.0f32;
    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let c = cfa.color_at(x, y);
            let v = scene(x, y, width, height, c);
            data.push((black + v * (white - black)).round().clamp(black, white) as u16);
        }
    }
    let color = ColorData {
        illuminant: [17, 21],
        color_matrix: [
            Some(Mat3([
                [0.9, 0.2, -0.15],
                [-0.3, 1.25, 0.08],
                [0.02, -0.12, 0.85],
            ])),
            Some(Mat3([
                [0.7, 0.3, -0.1],
                [-0.35, 1.3, 0.1],
                [0.05, -0.2, 1.0],
            ])),
        ],
        as_shot_neutral: Some([0.55, 1.0, 0.68]),
        baseline_exposure: 0.2,
        ..Default::default()
    };
    let metadata = lightcraft_raw::Metadata {
        make: Some("Omalight".into()),
        model: Some("Fixture".into()),
        ..Default::default()
    };
    let raw = RawImage {
        format: RawFormat::Dng,
        width,
        height,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits: 14,
        black: BlackLevel::uniform(black),
        white: vec![white],
        active_area: Rect::new(0, 0, width, height),
        crop: Rect::new(0, 0, width, height),
        orientation: Orientation::Normal,
        color,
        wb_multipliers: None,
        linearized: false,
        opcodes: Default::default(),
        metadata,
    };
    let opts = DngWriteOptions {
        compression: DngCompression::Uncompressed,
        ..Default::default()
    };
    write_dng(&raw, &opts).map_err(|e| e.to_string())
}

/// Sky on top, warm ground below, so a white-balance move is visible.
fn scene(x: usize, y: usize, w: usize, h: usize, channel: u8) -> f32 {
    let nx = x as f32 / w as f32;
    let ny = y as f32 / h as f32;
    let sky = ny < 0.42;
    let base = if sky {
        match channel {
            0 => 0.32,
            1 => 0.48,
            _ => 0.72,
        }
    } else {
        match channel {
            0 => 0.52 + 0.18 * nx,
            1 => 0.36,
            _ => 0.2,
        }
    };
    let shade = 0.82 + 0.18 * (nx * std::f32::consts::TAU).sin().abs();
    (base * shade).clamp(0.02, 0.98)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_raw::{decode, probe, RawFormat};

    #[test]
    fn fixture_decodes_as_dng() {
        let bytes = encode(64, 48).unwrap();
        assert_eq!(probe(&bytes), Some(RawFormat::Dng));
        let img = decode(&bytes).unwrap();
        assert_eq!((img.width, img.height), (64, 48));
    }
}
