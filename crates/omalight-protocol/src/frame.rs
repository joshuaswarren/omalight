use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use thiserror::Error;

pub const VERSION: u8 = 1;
pub const MAX_FRAME: u32 = 32 * 1024 * 1024;

pub const HELLO: u8 = 1;
pub const HELLO_OK: u8 = 2;
pub const PING: u8 = 3;
pub const PONG: u8 = 4;
pub const COMMAND: u8 = 5;
pub const COMMAND_RESULT: u8 = 6;
pub const PREVIEW: u8 = 7;
pub const PREVIEW_IMAGE: u8 = 8;
pub const EXPORT: u8 = 9;
pub const EXPORT_RESULT: u8 = 10;
pub const ERROR: u8 = 11;
pub const CANCEL: u8 = 12;
pub const OPEN: u8 = 13;
pub const OPEN_RESULT: u8 = 14;
pub const PAIR_START: u8 = 15;
pub const PAIR_OFFER: u8 = 16;
pub const PAIR_CONFIRM: u8 = 17;
pub const PAIR_RESULT: u8 = 18;

pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        HELLO => "hello",
        HELLO_OK => "hello-ok",
        PING => "ping",
        PONG => "pong",
        COMMAND => "command",
        COMMAND_RESULT => "command-result",
        PREVIEW => "preview",
        PREVIEW_IMAGE => "preview-image",
        EXPORT => "export",
        EXPORT_RESULT => "export-result",
        ERROR => "error",
        CANCEL => "cancel",
        OPEN => "open",
        OPEN_RESULT => "open-result",
        PAIR_START => "pair-start",
        PAIR_OFFER => "pair-offer",
        PAIR_CONFIRM => "pair-confirm",
        PAIR_RESULT => "pair-result",
        _ => "unknown",
    }
}

pub const KIND_NAME: fn(u8) -> &'static str = kind_name;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("frame is {0} bytes, limit is {MAX_FRAME}")]
    TooLarge(u32),
    #[error("frame version {0}")]
    Version(u8),
    #[error("short frame")]
    Short,
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Msg(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u64,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn json<T: Serialize>(id: u64, kind: u8, body: &T) -> Result<Self, ProtoError> {
        Ok(Self {
            id,
            kind,
            payload: serde_json::to_vec(body)?,
        })
    }

    pub fn parse<T: DeserializeOwned>(&self) -> Result<T, ProtoError> {
        Ok(serde_json::from_slice(&self.payload)?)
    }
}

/// `length` counts the bytes after itself: version, kind, id, payload.
pub fn write_frame(w: &mut impl Write, frame: &Frame) -> Result<(), ProtoError> {
    let len = 10u32
        .checked_add(frame.payload.len() as u32)
        .ok_or(ProtoError::TooLarge(u32::MAX))?;
    if len > MAX_FRAME {
        return Err(ProtoError::TooLarge(len));
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&[VERSION, frame.kind])?;
    w.write_all(&frame.id.to_be_bytes())?;
    w.write_all(&frame.payload)?;
    w.flush()?;
    Ok(())
}

pub fn read_frame(r: &mut impl Read) -> Result<Frame, ProtoError> {
    let mut len_b = [0u8; 4];
    r.read_exact(&mut len_b)?;
    let len = u32::from_be_bytes(len_b);
    if !(10..=MAX_FRAME).contains(&len) {
        return Err(if len < 10 {
            ProtoError::Short
        } else {
            ProtoError::TooLarge(len)
        });
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    if buf[0] != VERSION {
        return Err(ProtoError::Version(buf[0]));
    }
    let id = u64::from_be_bytes(buf[2..10].try_into().expect("8 bytes"));
    Ok(Frame {
        id,
        kind: buf[1],
        payload: buf[10..].to_vec(),
    })
}

/// Preview image payload: u32be JSON length, JSON metadata, then raw JPEG bytes.
pub fn encode_image_payload(meta_json: &[u8], jpeg: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let n = u32::try_from(meta_json.len()).map_err(|_| ProtoError::TooLarge(u32::MAX))?;
    let mut out = Vec::with_capacity(4 + meta_json.len() + jpeg.len());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(meta_json);
    out.extend_from_slice(jpeg);
    Ok(out)
}

pub fn decode_image_payload(payload: &[u8]) -> Result<(&[u8], &[u8]), ProtoError> {
    if payload.len() < 4 {
        return Err(ProtoError::Short);
    }
    let n = u32::from_be_bytes(payload[0..4].try_into().expect("4 bytes")) as usize;
    if payload.len() < 4 + n {
        return Err(ProtoError::Short);
    }
    Ok((&payload[4..4 + n], &payload[4 + n..]))
}

/// MCP stdio uses the same Content-Length framing as LSP.
pub fn write_lsp(w: &mut impl Write, body: &[u8]) -> Result<(), ProtoError> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()?;
    Ok(())
}

pub fn read_lsp(r: &mut impl std::io::BufRead) -> Result<Vec<u8>, ProtoError> {
    let mut len = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Err(ProtoError::Msg("mcp stream closed".into()));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            len = Some(
                rest.trim()
                    .parse::<usize>()
                    .map_err(|e| ProtoError::Msg(e.to_string()))?,
            );
        }
    }
    let len = len.ok_or_else(|| ProtoError::Msg("missing Content-Length".into()))?;
    if len > MAX_FRAME as usize {
        return Err(ProtoError::TooLarge(len as u32));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let frame = Frame {
            id: 42,
            kind: PING,
            payload: b"{}".to_vec(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &frame).unwrap();
        let back = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn rejects_a_huge_length() {
        let mut raw = 5u32.to_be_bytes().to_vec();
        raw.extend_from_slice(&[1, 1, 0, 0, 0]);
        assert!(matches!(
            read_frame(&mut Cursor::new(raw)).unwrap_err(),
            ProtoError::Short
        ));
        let mut raw = (MAX_FRAME + 1).to_be_bytes().to_vec();
        raw.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            read_frame(&mut Cursor::new(raw)).unwrap_err(),
            ProtoError::TooLarge(_)
        ));
    }

    #[test]
    fn image_payload_roundtrip() {
        let meta = br#"{"stage":"draft"}"#;
        let jpeg = b"\xff\xd8\xff";
        let payload = encode_image_payload(meta, jpeg).unwrap();
        let (m, j) = decode_image_payload(&payload).unwrap();
        assert_eq!(m, meta);
        assert_eq!(j, jpeg);
    }

    #[test]
    fn lsp_roundtrip() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let mut buf = Vec::new();
        write_lsp(&mut buf, body).unwrap();
        let back = read_lsp(&mut Cursor::new(buf)).unwrap();
        assert_eq!(back, body);
    }
}
