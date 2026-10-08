use std::process::{Command, Stdio};

/// Desktop notification for a long job. Omarchy's shell listens to the freedesktop notification
/// bus; `notify-send` is the usual client. Missing binary is not an error.
pub fn notify(summary: &str, body: &str) {
    if let Some(bin) = which("notify-send") {
        let _ = Command::new(bin)
            .args(["--app-name=Omalight", "--urgency=low", summary, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    } else {
        notify_fallback(summary, body);
    }
}

#[cfg(target_os = "macos")]
fn notify_fallback(summary: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(summary)
    );
    let _ = Command::new("osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn notify_fallback(_summary: &str, _body: &str) {}

#[cfg(target_os = "macos")]
fn escape_applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
    }
}
