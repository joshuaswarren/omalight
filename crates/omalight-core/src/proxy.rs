/// A local stand-in for the last preview. Slider moves multiply it immediately; the engine's JPEG
/// replaces it when it arrives. This is a display approximation, not the develop pipeline.
pub fn apply(rgba: &[u8], from_ev: f64, to_ev: f64, from_k: f64, to_k: f64) -> Vec<u8> {
    let gain = (2.0_f64).powf(to_ev - from_ev);
    // Lower kelvin is warmer: push red up and blue down. 5000 K across the slider is the full swing.
    let dk = ((to_k - from_k) / 5000.0).clamp(-1.0, 1.0);
    let r_gain = (1.0 - dk * 0.18) * gain;
    let g_gain = gain;
    let b_gain = (1.0 + dk * 0.18) * gain;
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.push(scale(px[0], r_gain));
        out.push(scale(px[1], g_gain));
        out.push(scale(px[2], b_gain));
        out.push(px[3]);
    }
    out
}

fn scale(sample: u8, gain: f64) -> u8 {
    (sample as f64 * gain).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_lifts_the_proxy() {
        let base = vec![40, 40, 40, 255, 10, 10, 10, 255];
        let out = apply(&base, 0.0, 1.0, 6500.0, 6500.0);
        assert!(out[0] > base[0]);
        assert!(out[4] > base[4]);
    }

    #[test]
    fn warmer_raises_red_against_blue() {
        let base = vec![100, 100, 100, 255];
        let out = apply(&base, 0.0, 0.0, 6500.0, 4500.0);
        assert!(out[0] > out[2]);
    }
}
