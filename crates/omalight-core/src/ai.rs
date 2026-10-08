use serde_json::{json, Value};

/// AI-heavy work is supposed to run on the engine host, next to the originals. Mask kinds that
/// LightCraft already builds are forwarded. The rest stay behind this interface so a later pass
/// can fill them in without changing the channel.
#[derive(Clone, Debug)]
pub enum HeavyKind {
    Mask { kind: String },
    Denoise { strength: f64 },
    Upscale { scale: u32 },
    Tag,
}

#[derive(Clone, Debug)]
pub enum HeavyOutcome {
    /// A LightCraft command the server should run.
    Ready { command: String, params: Value },
    /// The trait is here; the implementation is not.
    Stub {
        kind: &'static str,
        interface: &'static str,
    },
}

pub fn prepare(task: &HeavyKind) -> HeavyOutcome {
    match task {
        HeavyKind::Mask { kind } if matches!(kind.as_str(), "sky" | "subject" | "background" | "linear" | "radial" | "luminanceRange" | "colorRange") => {
            HeavyOutcome::Ready { command: "mask.add".into(), params: json!({ "kind": kind }) }
        }
        HeavyKind::Mask { kind: _ } => HeavyOutcome::Stub {
            kind: "mask",
            interface: "Object and prompt masks need LightCraft's SAM 3 feature on the engine host (`mask.add` kind object|prompt). Sky, subject, background, linear, and radial are wired today.",
        },
        HeavyKind::Denoise { .. } => HeavyOutcome::Stub {
            kind: "denoise",
            interface: "DenoiseRequest { photo: u64, strength: f64 in 0..=1 } runs on the engine host and returns { photo, path }. No denoise model is linked in this pass.",
        },
        HeavyKind::Upscale { .. } => HeavyOutcome::Stub {
            kind: "upscale",
            interface: "UpscaleRequest { photo: u64, scale: 2|4 } runs on the engine host and returns a new photo id. No upscaler is linked in this pass.",
        },
        HeavyKind::Tag => HeavyOutcome::Stub {
            kind: "tag",
            interface: "TagRequest { photo: u64 } runs on the engine host and returns { keywords: [string] }. No tagger is linked in this pass.",
        },
    }
}

pub fn outcome_json(outcome: &HeavyOutcome) -> Value {
    match outcome {
        HeavyOutcome::Ready { command, params } => {
            json!({ "status": "ready", "command": command, "params": params })
        }
        HeavyOutcome::Stub { kind, interface } => {
            json!({ "status": "unsupported", "kind": kind, "interface": interface })
        }
    }
}

pub fn parse_task(kind: &str, params: &Value) -> Result<HeavyKind, String> {
    match kind {
        "mask" => {
            let shape = params
                .get("mask")
                .or_else(|| params.get("shape"))
                .and_then(Value::as_str)
                .unwrap_or("radial");
            Ok(HeavyKind::Mask {
                kind: shape.to_string(),
            })
        }
        "denoise" => Ok(HeavyKind::Denoise {
            strength: params
                .get("strength")
                .and_then(Value::as_f64)
                .unwrap_or(0.5),
        }),
        "upscale" => Ok(HeavyKind::Upscale {
            scale: params.get("scale").and_then(Value::as_u64).unwrap_or(2) as u32,
        }),
        "tag" => Ok(HeavyKind::Tag),
        other => Err(format!("unknown task `{other}` (mask|denoise|upscale|tag)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_mask_is_a_real_command() {
        match prepare(&HeavyKind::Mask { kind: "sky".into() }) {
            HeavyOutcome::Ready { command, .. } => assert_eq!(command, "mask.add"),
            HeavyOutcome::Stub { .. } => panic!("sky should be wired"),
        }
    }

    #[test]
    fn denoise_is_an_explicit_stub() {
        match prepare(&HeavyKind::Denoise { strength: 0.4 }) {
            HeavyOutcome::Stub { kind, interface } => {
                assert_eq!(kind, "denoise");
                assert!(interface.contains("DenoiseRequest"));
            }
            HeavyOutcome::Ready { .. } => panic!("denoise is not implemented"),
        }
    }
}
