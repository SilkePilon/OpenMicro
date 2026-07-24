use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Push agent state events to openmicrod")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Push a raw {agent,session,state} event (the universal adapter contract).
    Push {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        state: String,
    },
    /// Read a Claude Code (or Claude-compatible) hook JSON from stdin, extract
    /// `session_id`, and push a state event. `--agent` lets Claude-compatible
    /// agents (e.g. Grok Code) reuse this stdin mechanism; it defaults to claude.
    ClaudeHook {
        #[arg(long)]
        state: String,
        #[arg(long, default_value = "claude")]
        agent: String,
    },
    /// Read a Codex CLI `notify` JSON (first positional arg, else stdin), map its
    /// event `type` to a state, and push a codex event.
    CodexNotify {
        /// The JSON payload Codex passes as the trailing positional argument.
        payload: Option<String>,
    },
}

/// Extract `session_id` from a Claude Code hook JSON object; fall back to
/// `"default"` when stdin is empty, not JSON, or lacks a non-empty `session_id`.
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

/// Map a Codex `notify` payload's event `type` to one of the daemon states.
///
/// Codex's confirmed notify event is `agent-turn-complete` (fired when the turn
/// finishes and the agent waits for input) -> `idle`. Anything that reads as an
/// approval/notification request -> `awaiting_approval`; everything else (a
/// known in-progress/start event, or an unrecognised type) -> `working`.
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

/// Extract a stable session id from a Codex notify payload; Codex currently
/// exposes none (the payload carries only a per-turn `turn-id`), so this returns
/// `"default"` unless a `session_id`/`session-id` field is present in a future
/// payload.
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

/// Best-effort: connect to the daemon socket and write one newline-JSON line.
/// If the daemon is down, silently succeed so hooks never block the agent.
fn push(agent: &str, session: &str, state: &str) {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro.sock");
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let mut line = serde_json::json!({
            "agent": agent,
            "session": session,
            "state": state,
        })
        .to_string();
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
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
    // Always exit 0 — never block the agent.
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
