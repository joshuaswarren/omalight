use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One step the palette or an agent can run against the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Step {
    #[serde(rename = "develop")]
    Develop { control: String, value: f64 },
    #[serde(rename = "mask")]
    Mask { kind: String },
    #[serde(rename = "export")]
    Export {
        #[serde(default = "edge_2048", rename = "longEdge")]
        long_edge: u32,
        #[serde(default = "quality_90")]
        quality: u8,
        #[serde(default = "jpeg")]
        format: String,
    },
    #[serde(rename = "undo")]
    Undo,
    #[serde(rename = "command")]
    Command {
        id: String,
        #[serde(default)]
        params: Value,
    },
}

fn edge_2048() -> u32 {
    2048
}
fn quality_90() -> u8 {
    90
}
fn jpeg() -> String {
    "jpeg".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkPlan {
    pub summary: String,
    pub steps: Vec<Step>,
    /// "rules" or "model". Models omit this; the parser fills it in.
    #[serde(default = "source_rules")]
    pub source: String,
}

fn source_rules() -> String {
    "rules".into()
}

impl WorkPlan {
    pub fn empty(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            steps: Vec::new(),
            source: "rules".into(),
        }
    }
}

/// Deterministic reading of a short edit request. Returns `None` when nothing matched, so the
/// caller can ask a model. Explicit numbers win over words like "warm".
pub fn plan_rules(text: &str) -> Option<WorkPlan> {
    let lower = text.to_lowercase();
    let mut steps = Vec::new();

    if let Some(ev) = number_after(&lower, &["exposure", "ev"]) {
        steps.push(Step::Develop {
            control: "light.exposure".into(),
            value: ev,
        });
    } else if lower.contains("brighten") || lower.contains("lighter") {
        let ev = if lower.contains("slightly") {
            0.15
        } else {
            0.35
        };
        steps.push(Step::Develop {
            control: "light.exposure".into(),
            value: ev,
        });
    } else if lower.contains("darken") || lower.contains("darker") {
        let ev = if lower.contains("slightly") {
            -0.15
        } else {
            -0.35
        };
        steps.push(Step::Develop {
            control: "light.exposure".into(),
            value: ev,
        });
    }

    if let Some(k) = kelvin(&lower) {
        steps.push(Step::Develop {
            control: "wb.temp".into(),
            value: k,
        });
    } else if lower.contains("warm") {
        let k = if lower.contains("slightly") {
            5600.0
        } else {
            4800.0
        };
        steps.push(Step::Develop {
            control: "wb.temp".into(),
            value: k,
        });
    } else if lower.contains("cool") || lower.contains("colder") {
        let k = if lower.contains("slightly") {
            7200.0
        } else {
            8500.0
        };
        steps.push(Step::Develop {
            control: "wb.temp".into(),
            value: k,
        });
    }

    for (word, kind) in [
        ("sky", "sky"),
        ("subject", "subject"),
        ("background", "background"),
    ] {
        if lower.contains(word) && (lower.contains("mask") || lower.contains("select")) {
            steps.push(Step::Mask { kind: kind.into() });
        }
    }

    if lower.contains("export") || lower.contains("jpeg") || lower.contains("jpg") {
        let long_edge = px_size(&lower).unwrap_or(2048);
        let format = if lower.contains("png") { "png" } else { "jpeg" };
        steps.push(Step::Export {
            long_edge,
            quality: 90,
            format: format.into(),
        });
    }

    if lower.trim() == "undo" || lower.contains("undo that") || lower.contains("undo the") {
        steps.push(Step::Undo);
    }

    if steps.is_empty() {
        return None;
    }
    let summary = steps
        .iter()
        .map(|s| match s {
            Step::Develop { control, value } => format!("{control} = {value}"),
            Step::Mask { kind } => format!("mask {kind}"),
            Step::Export {
                long_edge, format, ..
            } => format!("export {long_edge}px {format}"),
            Step::Undo => "undo".into(),
            Step::Command { id, .. } => id.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(WorkPlan {
        summary,
        steps,
        source: "rules".into(),
    })
}

fn number_after(text: &str, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(idx) = text.find(key) else { continue };
        let rest = text[idx + key.len()..]
            .trim_start_matches(|c: char| c == ':' || c.is_whitespace() || c == '=');
        if let Some(n) = take_number(rest) {
            return Some(n);
        }
    }
    None
}

fn kelvin(text: &str) -> Option<f64> {
    if let Some(n) = number_after(text, &["temperature", "temp", "white balance", "wb"]) {
        return Some(n);
    }
    // "5200K" or "5200 K"
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'k' || bytes[j] == b'K') {
                if let Ok(n) = text[start..i].parse::<f64>() {
                    if (2000.0..=50000.0).contains(&n) {
                        return Some(n);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn px_size(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if text[j..].starts_with("px") {
                if let Ok(n) = text[start..i].parse::<u32>() {
                    if n > 0 {
                        return Some(n);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn take_number(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '+' || *c == '-'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    s[..end].parse().ok()
}

/// Model output is JSON, optionally wrapped in a fence.
pub fn parse_model_plan(text: &str) -> Result<WorkPlan, String> {
    let trimmed = text.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|rest| rest.trim_start_matches('\n').trim_end_matches('`').trim())
        .unwrap_or(trimmed);
    let mut plan: WorkPlan = serde_json::from_str(json).map_err(|e| e.to_string())?;
    plan.source = "model".into();
    if plan.summary.is_empty() {
        plan.summary = "model plan".into();
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_portraits_and_export() {
        let plan = plan_rules("warm these portraits slightly and export 2048 px JPEGs").unwrap();
        assert!(plan.steps.iter().any(|s| matches!(s, Step::Develop { control, value } if control == "wb.temp" && *value == 5600.0)));
        assert!(plan.steps.iter().any(|s| matches!(s, Step::Export { long_edge, format, .. } if *long_edge == 2048 && format == "jpeg")));
        assert!(!plan.steps.iter().any(|s| matches!(s, Step::Mask { .. })));
    }

    #[test]
    fn explicit_exposure_and_sky_mask() {
        let plan = plan_rules("set exposure +0.4 and mask the sky").unwrap();
        assert!(plan.steps.iter().any(|s| matches!(s, Step::Develop { control, value } if control == "light.exposure" && (*value - 0.4).abs() < 1e-6)));
        assert!(plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::Mask { kind } if kind == "sky")));
    }

    #[test]
    fn unknown_sentence_is_none() {
        assert!(plan_rules("what time is it").is_none());
    }

    #[test]
    fn parses_fenced_model_json() {
        let raw = "```json\n{\"summary\":\"warm\",\"steps\":[{\"op\":\"develop\",\"control\":\"wb.temp\",\"value\":5600}]}\n```";
        let plan = parse_model_plan(raw).unwrap();
        assert_eq!(plan.source, "model");
        assert_eq!(plan.steps.len(), 1);
    }
}
