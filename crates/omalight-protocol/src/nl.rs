use crate::plan::{parse_model_plan, plan_rules, WorkPlan};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

#[derive(Debug)]
pub struct LlmError(pub String);

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

const SYSTEM: &str = "\
You turn a photographer's sentence into Omalight edits. Reply with one JSON object and no other text.\n\
Schema: {\"summary\": string, \"steps\": [ step, ... ]}\n\
A step is one of:\n\
{\"op\":\"develop\",\"control\":\"light.exposure\",\"value\": number}  value is EV, -5 to 5\n\
{\"op\":\"develop\",\"control\":\"wb.temp\",\"value\": number}  kelvin, 2000 to 50000. Warmer is a lower number.\n\
{\"op\":\"develop\",\"control\":\"wb.tint\",\"value\": number}  -150 to 150\n\
{\"op\":\"mask\",\"kind\":\"sky\"|\"subject\"|\"background\"|\"linear\"|\"radial\"}\n\
{\"op\":\"export\",\"longEdge\": number, \"quality\": 1-100, \"format\":\"jpeg\"|\"png\"}\n\
{\"op\":\"undo\"}\n\
{\"op\":\"command\",\"id\": string, \"params\": object}  id is a LightCraft command such as photo.rate or photo.flag\n\
Do not invent file paths. Do not set ratings or flags unless the sentence asks to cull.\n";

/// Ask an OpenAI-compatible `/chat/completions` endpoint, then fall back to the rule parser.
pub fn plan_request(text: &str, endpoint: Option<&Endpoint>) -> WorkPlan {
    if let Some(ep) = endpoint {
        match plan_with(ep, text) {
            Ok(plan) => return plan,
            Err(err) => {
                if let Some(mut rules) = plan_rules(text) {
                    rules.summary = format!("{} (model unavailable: {err})", rules.summary);
                    return rules;
                }
                return WorkPlan::empty(format!("model unavailable: {err}"));
            }
        }
    }
    plan_rules(text).unwrap_or_else(|| WorkPlan::empty("no edit in that sentence"))
}

pub fn plan_with(endpoint: &Endpoint, text: &str) -> Result<WorkPlan, LlmError> {
    let base = endpoint.base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");
    let body = json!({
        "model": endpoint.model,
        "temperature": 0,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": text}
        ]
    });
    let mut req = ureq::post(&url).set("Content-Type", "application/json");
    if let Some(key) = &endpoint.api_key {
        if !key.is_empty() {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
    }
    let response = req.send_json(body).map_err(|e| LlmError(e.to_string()))?;
    let parsed: Value = response.into_json().map_err(|e| LlmError(e.to_string()))?;
    let content = parsed
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError("response had no message content".into()))?;
    parse_model_plan(content).map_err(LlmError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn rules_used_when_no_endpoint() {
        let plan = plan_request(
            "warm these portraits slightly and export 2048 px JPEGs",
            None,
        );
        assert_eq!(plan.source, "rules");
        assert!(plan.summary.contains("wb.temp"));
    }

    #[test]
    fn model_json_is_accepted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let body = br#"{"choices":[{"message":{"content":"{\"summary\":\"from-model\",\"steps\":[{\"op\":\"undo\"}]}"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                std::str::from_utf8(body).unwrap()
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        let plan = plan_with(
            &Endpoint {
                base_url: format!("http://127.0.0.1:{port}/v1"),
                model: "local".into(),
                api_key: None,
            },
            "undo that",
        )
        .unwrap();
        assert_eq!(plan.source, "model");
        assert_eq!(plan.summary, "from-model");
        assert_eq!(plan.steps.len(), 1);
    }
}
