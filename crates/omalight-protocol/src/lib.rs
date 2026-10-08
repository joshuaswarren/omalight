//! Shared types for the Omalight engine channel.
//!
//! Frames are length-prefixed and the image body is raw JPEG. Command payloads are JSON because
//! they are small next to a preview and they match LightCraft's command parameters.

mod auth;
mod frame;
mod nl;
mod plan;
mod theme;

pub use auth::{token_ok, tokens_eq};
pub use frame::{
    decode_image_payload, encode_image_payload, kind_name, read_frame, read_lsp, write_frame,
    write_lsp, Frame, ProtoError, CANCEL, COMMAND, COMMAND_RESULT, ERROR, EXPORT, EXPORT_RESULT,
    HELLO, HELLO_OK, KIND_NAME, MAX_FRAME, OPEN, OPEN_RESULT, PAIR_CONFIRM, PAIR_OFFER,
    PAIR_RESULT, PAIR_START, PING, PONG, PREVIEW, PREVIEW_IMAGE, VERSION,
};
pub use nl::{plan_request, plan_with, Endpoint, LlmError};
pub use plan::{plan_rules, Step, WorkPlan};
pub use theme::{active_theme_paths, parse_colors_toml, Color, Theme, ThemeWatch};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// First frame on a connection that already has a credential.
///
/// An empty token never authenticates by itself. `local` asks a loopback engine to accept the
/// connection when the peer is the same user. The engine secret is never a valid credential off
/// loopback.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(default)]
    pub token: String,
    pub client: String,
    #[serde(default)]
    pub local: bool,
}

/// Client starts pairing. `client_pub` is a base64 X25519 public key. The code is not in this frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub name: String,
    pub client_pub: String,
}

/// Engine reply. The code is derived by both sides and is not sent here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairOffer {
    pub server_pub: String,
}

/// Client says whether the codes matched. `code` is what the client computed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairConfirm {
    pub code: String,
    pub accept: bool,
}

/// On success, `credential` is a per-device token. The engine secret is never returned.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairResult {
    pub ok: bool,
    pub credential: String,
    pub name: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelloOk {
    pub version: u32,
    pub auth: String,
    pub engine: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenReq {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    pub photo: u64,
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub exposure: f64,
    pub temp: f64,
}

/// One preview. `generation` is the client's monotonic id. The server finishes only the newest
/// generation it has seen for this connection: an older full-resolution pass is cancelled.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReq {
    pub photo: u64,
    pub generation: u64,
    pub viewport_w: u32,
    pub viewport_h: u32,
    pub values: BTreeMap<String, f64>,
    /// Start a slider interaction so the whole drag is one undo step.
    #[serde(default)]
    pub begin: bool,
    /// Commit the interaction after the full preview.
    #[serde(default)]
    pub end: bool,
    /// When false, render `values` without writing them into the catalog.
    #[serde(default = "default_commit")]
    pub commit: bool,
}

fn default_commit() -> bool {
    true
}

impl PreviewReq {
    pub fn fit(&self, draft: bool) -> (u32, u32) {
        let (mut w, mut h) = (self.viewport_w.max(64), self.viewport_h.max(64));
        if draft {
            w = (w / 4).clamp(160, 640);
            h = (h / 4).clamp(120, 480);
        } else {
            w = w.clamp(160, 2048);
            h = h.clamp(120, 2048);
        }
        (w, h)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewMeta {
    pub photo: u64,
    pub generation: u64,
    pub stage: String,
    pub width: u32,
    pub height: u32,
    pub server_us: u64,
}

impl PreviewMeta {
    pub fn cancelled(photo: u64, generation: u64) -> Self {
        Self {
            photo,
            generation,
            stage: "cancelled".into(),
            width: 0,
            height: 0,
            server_us: 0,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.stage == "full" || self.stage == "cancelled"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReq {
    pub photo: u64,
    pub dest: String,
    pub long_edge: u32,
    pub quality: u8,
    #[serde(default = "jpeg_format")]
    pub format: String,
}

fn jpeg_format() -> String {
    "jpeg".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub server_us: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandReq {
    pub id: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorMsg {
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelReq {
    pub generation: u64,
}

/// Keep every non-preview, in arrival order, and only the newest preview. Older previews are
/// dropped. The surviving preview is last so an open or export queued ahead of a drag still runs.
pub fn coalesce(jobs: Vec<Queued>) -> Vec<Queued> {
    let mut newest: Option<PreviewReq> = None;
    let mut out = Vec::with_capacity(jobs.len());
    for job in jobs {
        match job {
            Queued::Preview(p) => {
                if newest.as_ref().is_none_or(|n| p.generation >= n.generation) {
                    newest = Some(p);
                }
            }
            other => out.push(other),
        }
    }
    if let Some(p) = newest {
        out.push(Queued::Preview(p));
    }
    out
}

#[derive(Clone, Debug)]
pub enum Queued {
    Preview(PreviewReq),
    Other(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prev(generation: u64) -> Queued {
        Queued::Preview(PreviewReq {
            photo: 1,
            generation,
            viewport_w: 800,
            viewport_h: 600,
            values: BTreeMap::new(),
            begin: false,
            end: false,
            commit: true,
        })
    }

    #[test]
    fn coalesce_keeps_newest_preview_and_other_work() {
        let jobs = vec![
            Queued::Other("open"),
            prev(1),
            prev(3),
            Queued::Other("export"),
            prev(2),
        ];
        let out = coalesce(jobs);
        let gens: Vec<u64> = out
            .iter()
            .filter_map(|j| match j {
                Queued::Preview(p) => Some(p.generation),
                Queued::Other(_) => None,
            })
            .collect();
        assert_eq!(gens, vec![3]);
        assert!(matches!(out.first(), Some(Queued::Other("open"))));
        assert!(out.iter().any(|j| matches!(j, Queued::Other("export"))));
    }

    #[test]
    fn draft_is_smaller_than_the_viewport() {
        let req = PreviewReq {
            photo: 1,
            generation: 1,
            viewport_w: 1600,
            viewport_h: 1000,
            values: BTreeMap::new(),
            begin: false,
            end: true,
            commit: true,
        };
        assert_eq!(req.fit(true), (400, 250));
        assert_eq!(req.fit(false), (1600, 1000));
    }
}
