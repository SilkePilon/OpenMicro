use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Push agent state events to openmicrod")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Push {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        state: String,
    },
    ClaudeHook {
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "claude")]
        agent: String,
    },
    CodexNotify {
        payload: Option<String>,
    },
}

fn session_from_claude_hook(stdin_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(stdin_json.trim())
        .ok()
        .and_then(|v| {
            v.get("session_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn codex_event_to_state(payload_json: &str) -> &'static str {
    let ty = serde_json::from_str::<serde_json::Value>(payload_json.trim())
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default();
    if ty.is_empty() {
        return "working";
    }
    if ty.contains("approval") || ty.contains("request") || ty.contains("notification") {
        "awaiting_approval"
    } else if ty.contains("complete")
        || ty.contains("finish")
        || ty.contains("done")
        || ty.contains("idle")
        || ty.contains("stop")
    {
        "idle"
    } else {
        "working"
    }
}

fn session_from_codex(payload_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(payload_json.trim())
        .ok()
        .and_then(|v| {
            v.get("session_id")
                .or_else(|| v.get("session-id"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn read_stdin() -> String {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    buf
}

const PUSH_WRITE_TIMEOUT: Duration = Duration::from_millis(300);
const PUSH_DEADLINE: Duration = Duration::from_millis(500);

fn push(agent: &str, session: &str, state: &str) {
    let path = openmicro_proto::paths::hook_socket();
    let mut line = serde_json::json!({
        "agent": agent,
        "session": session,
        "state": state,
    })
    .to_string();
    line.push('\n');
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(mut stream) = UnixStream::connect(&path) {
            let _ = stream.set_write_timeout(Some(PUSH_WRITE_TIMEOUT));
            let _ = stream.write_all(line.as_bytes());
        }
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(PUSH_DEADLINE);
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Push {
            agent,
            session,
            state,
        } => push(&agent, &session, &state),
        Command::ClaudeHook { state, agent } => {
            let stdin = read_stdin();
            let session = session_from_claude_hook(&stdin);
            push(&agent, &session, &state);
        }
        Command::CodexNotify { payload } => {
            let json = payload.unwrap_or_else(read_stdin);
            let state = codex_event_to_state(&json);
            let session = session_from_codex(&json);
            push("codex", &session, state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hook_extracts_session_id() {
        let json = r#"{"session_id":"abc123","hook_event_name":"PreToolUse"}"#;
        assert_eq!(session_from_claude_hook(json), "abc123");
    }

    #[test]
    fn claude_hook_empty_stdin_is_default() {
        assert_eq!(session_from_claude_hook(""), "default");
    }

    #[test]
    fn claude_hook_garbage_is_default() {
        assert_eq!(session_from_claude_hook("not json at all"), "default");
    }

    #[test]
    fn claude_hook_missing_session_id_is_default() {
        assert_eq!(
            session_from_claude_hook(r#"{"hook_event_name":"Stop"}"#),
            "default"
        );
    }

    #[test]
    fn claude_hook_empty_session_id_is_default() {
        assert_eq!(session_from_claude_hook(r#"{"session_id":""}"#), "default");
    }

    #[test]
    fn codex_turn_complete_is_idle() {
        let json = r#"{"type":"agent-turn-complete","turn-id":"12345","last-assistant-message":"done"}"#;
        assert_eq!(codex_event_to_state(json), "idle");
    }

    #[test]
    fn codex_approval_request_is_awaiting() {
        assert_eq!(
            codex_event_to_state(r#"{"type":"approval-requested"}"#),
            "awaiting_approval"
        );
    }

    #[test]
    fn codex_unknown_type_is_working() {
        assert_eq!(codex_event_to_state(r#"{"type":"agent-turn-start"}"#), "working");
    }

    #[test]
    fn codex_garbage_is_working() {
        assert_eq!(codex_event_to_state("not json"), "working");
    }

    #[test]
    fn codex_session_defaults_without_id() {
        assert_eq!(
            session_from_codex(r#"{"type":"agent-turn-complete","turn-id":"12345"}"#),
            "default"
        );
    }

    #[test]
    fn codex_session_uses_session_id_when_present() {
        assert_eq!(
            session_from_codex(r#"{"type":"agent-turn-complete","session_id":"sess-9"}"#),
            "sess-9"
        );
    }
}
