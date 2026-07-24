//! Coding-agent detection and hook/plugin installation.
//!
//! Every OpenMicro adapter ultimately does one thing: make the agent run
//! `openmicro-hook` on lifecycle transitions (see `adapters/README.md`). This
//! module turns those hand-written install docs into something the TUI can do
//! *for* the user:
//!
//! * [`detect`] — which coding agents are actually installed on this machine,
//!   and whether their OpenMicro hooks are already wired.
//! * [`install`] — idempotently merge the hook config into the agent's own
//!   settings file, keeping every other key the user has there.
//!
//! The file-mutating half is deliberately thin: all of the interesting logic
//! ([`merge_claude_hooks`], [`insert_codex_notify`], and the `*_installed`
//! predicates) is pure string-in/string-out so it is unit-tested without
//! touching a real `~/.claude/settings.json`.

use std::path::{Path, PathBuf};

/// The four lifecycle transitions OpenMicro cares about, as
/// `(Claude-compatible event name, OpenMicro state)`.
pub const HOOK_EVENTS: [(&str, &str); 4] = [
    ("UserPromptSubmit", "thinking"),
    ("PreToolUse", "working"),
    ("Notification", "awaiting_approval"),
    ("Stop", "idle"),
];

/// The `notify` line the Codex CLI adapter needs as a **root** key.
pub const CODEX_NOTIFY_LINE: &str = r#"notify = ["openmicro-hook", "codex-notify"]"#;

/// Substring that identifies a hook command as ours, used to keep installation
/// idempotent (we never append a second copy of our own hook).
const HOOK_MARKER: &str = "openmicro-hook";

/// A coding agent OpenMicro ships an adapter for.
///
/// `ValueEnum` so the same type is the CLI's `install-agent <agent>` argument;
/// clap derives the value names (`claude`, `codex`, `grok`) from the
/// variants, matching [`AgentKind::slug`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentKind {
    Claude,
    Codex,
    Grok,
}

/// Every adapter, in display order.
pub const ALL_AGENTS: [AgentKind; 3] =
    [AgentKind::Claude, AgentKind::Codex, AgentKind::Grok];

/// How an agent's hooks are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// Claude Code-style JSON hooks; `agent_flag` is passed as `--agent` when
    /// the agent is not Claude itself.
    ClaudeHooks { agent_flag: Option<&'static str> },
    /// Codex CLI `notify` root key in a TOML config.
    CodexNotify,
}

impl AgentKind {
    /// Short stable name used on the wire (`--agent`) and in the CLI.
    pub fn slug(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Grok => "grok",
        }
    }

    /// Human-facing name for the TUI list.
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::Codex => "Codex CLI",
            AgentKind::Grok => "Grok Code",
        }
    }

    /// Directory under `adapters/` holding this agent's install docs.
    pub fn adapter_dir(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Grok => "grok-code",
        }
    }

    /// Config file (relative to `$HOME`) whose contents we merge into.
    pub fn config_rel(self) -> &'static str {
        match self {
            AgentKind::Claude => ".claude/settings.json",
            AgentKind::Codex => ".codex/config.toml",
            AgentKind::Grok => ".grok/user-settings.json",
        }
    }

    /// Paths (relative to `$HOME`) whose existence means the agent is installed.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &[".claude", ".claude.json"],
            AgentKind::Codex => &[".codex"],
            AgentKind::Grok => &[".grok"],
        }
    }

    /// Executable names that mean the agent is installed even without a config
    /// directory yet (a fresh install that has never been run).
    pub fn binaries(self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &["claude"],
            AgentKind::Codex => &["codex"],
            AgentKind::Grok => &["grok", "grok-cli"],
        }
    }

    /// How this agent's hooks are wired.
    pub fn mechanism(self) -> Mechanism {
        match self {
            AgentKind::Claude => Mechanism::ClaudeHooks { agent_flag: None },
            AgentKind::Grok => Mechanism::ClaudeHooks { agent_flag: Some("grok") },
            AgentKind::Codex => Mechanism::CodexNotify,
        }
    }

    /// Parse a CLI slug back into a kind.
    pub fn from_slug(s: &str) -> Option<AgentKind> {
        ALL_AGENTS.into_iter().find(|a| a.slug() == s)
    }
}

/// Whether an agent's OpenMicro hooks are wired up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStatus {
    /// All of our hook entries are present.
    Installed,
    /// The config is readable/absent and our hooks can be added.
    Missing,
    /// Something in the config blocks a safe merge (unparseable, or a
    /// conflicting `notify` key). The payload is shown to the user as-is.
    Blocked(String),
}

/// One row of the TUI's agent picker.
#[derive(Debug, Clone)]
pub struct AgentRow {
    pub kind: AgentKind,
    /// Whether the agent itself appears to be installed on this machine.
    pub present: bool,
    /// Whether *our* hooks are already in its config.
    pub status: HookStatus,
    /// Config file we would write (absolute).
    pub config_path: PathBuf,
    /// Preselected in the picker (present + installable + not already done).
    pub selected: bool,
}

/// `$HOME` as a path, falling back to `/` so callers never have to handle the
/// (practically impossible) unset case with their own error branch.
pub fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// True if the agent looks installed: a config marker exists, or one of its
/// executables is on `PATH`.
pub fn is_present(kind: AgentKind, home: &Path) -> bool {
    kind.markers().iter().any(|m| home.join(m).exists())
        || crate::flash::which(kind.binaries()).is_some()
}

/// Read an agent's config file, returning `""` when it does not exist yet
/// (an absent file is a perfectly installable starting point).
fn read_config(kind: AgentKind, home: &Path) -> Result<String, String> {
    let path = home.join(kind.config_rel());
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Inspect an agent's current hook status without modifying anything.
pub fn hook_status(kind: AgentKind, home: &Path) -> HookStatus {
    match kind.mechanism() {
        Mechanism::ClaudeHooks { agent_flag } => {
            let contents = match read_config(kind, home) {
                Ok(c) => c,
                Err(e) => return HookStatus::Blocked(e),
            };
            if let Err(e) = merge_claude_hooks(&contents, agent_flag) {
                return HookStatus::Blocked(e);
            }
            if claude_hooks_installed(&contents) {
                HookStatus::Installed
            } else {
                HookStatus::Missing
            }
        }
        Mechanism::CodexNotify => {
            let contents = match read_config(kind, home) {
                Ok(c) => c,
                Err(e) => return HookStatus::Blocked(e),
            };
            match insert_codex_notify(&contents) {
                Err(e) => HookStatus::Blocked(e),
                Ok(_) if codex_notify_installed(&contents) => HookStatus::Installed,
                Ok(_) => HookStatus::Missing,
            }
        }
    }
}

/// Build the picker rows for every adapter: presence, hook status, target path.
pub fn detect(home: &Path) -> Vec<AgentRow> {
    ALL_AGENTS
        .into_iter()
        .map(|kind| {
            let present = is_present(kind, home);
            let status = hook_status(kind, home);
            let selected = present && status == HookStatus::Missing;
            AgentRow { kind, present, status, config_path: home.join(kind.config_rel()), selected }
        })
        .collect()
}

/// The hook command string for one `(event, state)` pair.
pub fn hook_command(state: &str, agent_flag: Option<&str>) -> String {
    match agent_flag {
        Some(a) => format!("openmicro-hook claude-hook --agent {a} --state {state}"),
        None => format!("openmicro-hook claude-hook --state {state}"),
    }
}

/// True if a JSON hooks config already contains an OpenMicro command for every
/// one of [`HOOK_EVENTS`]. Empty/unparseable input is simply "not installed".
pub fn claude_hooks_installed(existing: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(blank_to_empty_object(existing))
    else {
        return false;
    };
    let Some(hooks) = value.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    HOOK_EVENTS.iter().all(|(event, _)| {
        hooks
            .get(*event)
            .and_then(|e| e.as_array())
            .is_some_and(|groups| groups.iter().any(group_has_openmicro_hook))
    })
}

/// True if a single matcher group already runs an `openmicro-hook` command.
fn group_has_openmicro_hook(group: &serde_json::Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|entries| {
            entries.iter().any(|e| {
                e.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
        })
}

/// Treat a whitespace-only config as an empty JSON object, so a brand new
/// (or empty) `settings.json` merges instead of failing to parse.
fn blank_to_empty_object(existing: &str) -> &str {
    if existing.trim().is_empty() {
        "{}"
    } else {
        existing
    }
}

/// Merge OpenMicro's hooks into a Claude Code-style `settings.json`.
///
/// Pure: takes the current file contents, returns the new contents. Every
/// unrelated key is preserved (serde_json's `preserve_order` feature keeps the
/// user's key order too), existing hook entries for the same events are kept
/// alongside ours, and re-running is a no-op — a group already invoking
/// `openmicro-hook` is never duplicated.
///
/// Errors when the file is not a JSON **object**, or when `hooks` /
/// `hooks.<event>` exist with an incompatible shape; in that case we refuse to
/// touch the file rather than clobbering the user's config.
pub fn merge_claude_hooks(existing: &str, agent_flag: Option<&str>) -> Result<String, String> {
    let mut root: serde_json::Value = serde_json::from_str(blank_to_empty_object(existing))
        .map_err(|e| format!("config is not valid JSON ({e}) — fix it by hand first"))?;

    let obj = root
        .as_object_mut()
        .ok_or_else(|| "config is valid JSON but not an object".to_string())?;

    let hooks_entry = obj
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    let hooks = hooks_entry
        .as_object_mut()
        .ok_or_else(|| "`hooks` exists but is not an object".to_string())?;

    for (event, state) in HOOK_EVENTS {
        let groups_entry = hooks
            .entry(event.to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let groups = groups_entry
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.{event}` exists but is not an array"))?;
        if groups.iter().any(group_has_openmicro_hook) {
            continue;
        }
        groups.push(serde_json::json!({
            "hooks": [ { "type": "command", "command": hook_command(state, agent_flag) } ]
        }));
    }

    let mut out = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("cannot serialize merged config: {e}"))?;
    out.push('\n');
    Ok(out)
}

/// True if a Codex `config.toml` already has a **root** `notify` key pointing at
/// `openmicro-hook`.
pub fn codex_notify_installed(existing: &str) -> bool {
    matches!(root_notify_line(existing), Some((_, line)) if line.contains(HOOK_MARKER))
}

/// Find the root-level `notify = ...` assignment: the first line that assigns
/// `notify` **before** any `[table]` header (a `notify` inside a table is a
/// different key and must be ignored). Returns `(line index, line)`.
fn root_notify_line(existing: &str) -> Option<(usize, &str)> {
    for (i, line) in existing.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('[') {
            return None; // entered the first table; no root notify above it
        }
        if let Some(rest) = t.strip_prefix("notify") {
            if rest.trim_start().starts_with('=') {
                return Some((i, line));
            }
        }
    }
    None
}

/// Add the OpenMicro `notify` root key to a Codex `config.toml`.
///
/// Pure: contents in, contents out. TOML requires root keys to appear *before*
/// the first `[table]`, so the line is inserted at the top of the file (after
/// any leading comments) rather than appended. Re-running is a no-op.
///
/// Errors when a root `notify` already points at something else — overwriting
/// it would silently break whatever the user had wired up.
pub fn insert_codex_notify(existing: &str) -> Result<String, String> {
    if let Some((_, line)) = root_notify_line(existing) {
        if line.contains(HOOK_MARKER) {
            return Ok(existing.to_string());
        }
        return Err(format!(
            "~/.codex/config.toml already sets a different `notify` program ({}). \
             Chain it yourself or remove it, then re-run.",
            line.trim()
        ));
    }

    // Insert after any leading comment/blank block so a file that opens with a
    // header comment keeps reading naturally.
    let lines: Vec<&str> = existing.lines().collect();
    let mut at = 0;
    while at < lines.len() {
        let t = lines[at].trim();
        if t.is_empty() || t.starts_with('#') {
            at += 1;
        } else {
            break;
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 3);
    out.extend(lines[..at].iter().map(|s| s.to_string()));
    out.push("# OpenMicro macropad adapter (openmicro install-agent codex)".to_string());
    out.push(CODEX_NOTIFY_LINE.to_string());
    if at < lines.len() {
        out.push(String::new());
    }
    out.extend(lines[at..].iter().map(|s| s.to_string()));

    let mut text = out.join("\n");
    text.push('\n');
    Ok(text)
}

/// What [`install`] did.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub kind: AgentKind,
    /// The config file written (or that would have been written).
    pub path: PathBuf,
    /// False when the hooks were already present and nothing was rewritten.
    pub changed: bool,
    /// Where the previous contents were saved, when we replaced an existing file.
    pub backup: Option<PathBuf>,
}

impl InstallReport {
    /// One-line summary for the TUI log / CLI output.
    pub fn summary(&self) -> String {
        if !self.changed {
            return format!("{}: already installed ({})", self.kind.slug(), self.path.display());
        }
        match &self.backup {
            Some(b) => format!(
                "{}: hooks installed in {} (backup: {})",
                self.kind.slug(),
                self.path.display(),
                b.display()
            ),
            None => format!("{}: hooks installed in {}", self.kind.slug(), self.path.display()),
        }
    }
}

/// Install (or verify) one agent's OpenMicro hooks under `home`.
///
/// Idempotent, non-destructive: the existing file is backed up to
/// `<name>.openmicro.bak` before the merged version is written, the write is
/// atomic (temp file + rename), and an unchanged config is not rewritten at all.
pub fn install(kind: AgentKind, home: &Path) -> Result<InstallReport, String> {
    let path = home.join(kind.config_rel());
    let existing = read_config(kind, home)?;

    let merged = match kind.mechanism() {
        Mechanism::ClaudeHooks { agent_flag } => merge_claude_hooks(&existing, agent_flag)?,
        Mechanism::CodexNotify => insert_codex_notify(&existing)?,
    };

    if merged == existing {
        return Ok(InstallReport { kind, path, changed: false, backup: None });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }

    // Back up the previous contents before replacing them. Only when the file
    // actually existed — a first-time install has nothing to preserve.
    let backup = if path.exists() {
        let b = sibling(&path, ".openmicro.bak");
        std::fs::write(&b, &existing).map_err(|e| format!("cannot write {}: {e}", b.display()))?;
        Some(b)
    } else {
        None
    };

    let tmp = sibling(&path, ".openmicro.tmp");
    std::fs::write(&tmp, merged.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("cannot replace {}: {e}", path.display()))?;

    Ok(InstallReport { kind, path, changed: true, backup })
}

/// A sibling path with `suffix` appended to the whole filename (extension
/// included), e.g. `settings.json` + `.openmicro.bak` ->
/// `settings.json.openmicro.bak`. Appending to the full name (rather than
/// replacing the extension) keeps the original name recognisable and never
/// collides with the live config.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Locate the `openmicro-hook` binary the installed hooks will invoke.
/// Hooks call it by bare name, so it has to be resolvable from the agent's
/// environment — a `None` here is worth warning about before installing.
pub fn hook_binary() -> Option<PathBuf> {
    crate::flash::which(&["openmicro-hook"])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch `$HOME` that cleans itself up.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "openmicro-agents-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            TempHome(base)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn merge_into_empty_config_adds_all_four_events() {
        let out = merge_claude_hooks("", None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        for (event, state) in HOOK_EVENTS {
            let groups = v["hooks"][event].as_array().unwrap();
            assert_eq!(groups.len(), 1, "{event}");
            let cmd = groups[0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains(state), "{event} -> {cmd}");
            assert!(cmd.starts_with("openmicro-hook claude-hook"), "{cmd}");
        }
        assert!(claude_hooks_installed(&out));
    }

    #[test]
    fn merge_preserves_unrelated_keys() {
        let existing = r#"{"model":"opus","permissions":{"allow":["Bash"]}}"#;
        let out = merge_claude_hooks(existing, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["allow"][0], "Bash");
        assert!(v["hooks"]["Stop"].is_array());
    }

    #[test]
    fn merge_keeps_user_hooks_for_the_same_event() {
        let existing = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}"#;
        let out = merge_claude_hooks(existing, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user's group must survive alongside ours");
        assert_eq!(stop[0]["hooks"][0]["command"], "notify-send done");
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_claude_hooks("", None).unwrap();
        let twice = merge_claude_hooks(&once, None).unwrap();
        assert_eq!(once, twice, "re-running must not duplicate hook entries");
    }

    #[test]
    fn merge_uses_agent_flag_for_grok() {
        let out = merge_claude_hooks("", Some("grok")).unwrap();
        assert!(out.contains("--agent grok"), "{out}");
    }

    #[test]
    fn merge_rejects_invalid_json() {
        let err = merge_claude_hooks("{not json", None).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn merge_rejects_non_object_root() {
        let err = merge_claude_hooks("[1,2,3]", None).unwrap_err();
        assert!(err.contains("not an object"), "{err}");
    }

    #[test]
    fn merge_rejects_wrong_shaped_hooks_key() {
        let err = merge_claude_hooks(r#"{"hooks":"nope"}"#, None).unwrap_err();
        assert!(err.contains("`hooks`"), "{err}");
        let err = merge_claude_hooks(r#"{"hooks":{"Stop":"nope"}}"#, None).unwrap_err();
        assert!(err.contains("hooks.Stop"), "{err}");
    }

    #[test]
    fn hooks_installed_is_false_when_only_some_events_present() {
        let partial = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"openmicro-hook claude-hook --state idle"}]}]}}"#;
        assert!(!claude_hooks_installed(partial));
    }

    #[test]
    fn hooks_installed_false_for_garbage() {
        assert!(!claude_hooks_installed("not json"));
        assert!(!claude_hooks_installed(""));
    }

    #[test]
    fn codex_notify_inserted_at_root_before_tables() {
        let existing = "[tui]\ntheme = \"dark\"\n";
        let out = insert_codex_notify(existing).unwrap();
        let notify_at = out.find("notify = ").unwrap();
        let table_at = out.find("[tui]").unwrap();
        assert!(notify_at < table_at, "root key must precede the first table:\n{out}");
        assert!(out.contains("[tui]") && out.contains("theme = \"dark\""));
        assert!(codex_notify_installed(&out));
    }

    #[test]
    fn codex_notify_inserted_after_leading_comments() {
        let existing = "# my codex config\n\nmodel = \"gpt\"\n";
        let out = insert_codex_notify(existing).unwrap();
        assert!(out.starts_with("# my codex config"), "{out}");
        assert!(out.contains("model = \"gpt\""));
        assert!(codex_notify_installed(&out));
    }

    #[test]
    fn codex_notify_into_empty_file() {
        let out = insert_codex_notify("").unwrap();
        assert!(codex_notify_installed(&out), "{out}");
    }

    #[test]
    fn codex_notify_is_idempotent() {
        let once = insert_codex_notify("[tui]\n").unwrap();
        let twice = insert_codex_notify(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn codex_notify_conflict_is_reported_not_overwritten() {
        let existing = "notify = [\"my-own-notifier\"]\n[tui]\n";
        let err = insert_codex_notify(existing).unwrap_err();
        assert!(err.contains("different `notify`"), "{err}");
    }

    #[test]
    fn codex_notify_inside_a_table_is_not_the_root_key() {
        // `notify` under [some.table] is a different key entirely; we must still
        // add the root one rather than reporting a conflict.
        let existing = "[mcp_servers.x]\nnotify = [\"other\"]\n";
        let out = insert_codex_notify(existing).unwrap();
        assert!(codex_notify_installed(&out), "{out}");
        assert!(out.contains("[mcp_servers.x]"));
    }

    #[test]
    fn install_creates_claude_settings_and_is_idempotent() {
        let home = TempHome::new("claude");
        let first = install(AgentKind::Claude, home.path()).unwrap();
        assert!(first.changed);
        assert!(first.backup.is_none(), "nothing to back up on a fresh install");
        assert_eq!(hook_status(AgentKind::Claude, home.path()), HookStatus::Installed);

        let second = install(AgentKind::Claude, home.path()).unwrap();
        assert!(!second.changed, "second run must be a no-op");
    }

    #[test]
    fn install_backs_up_an_existing_config() {
        let home = TempHome::new("backup");
        let path = home.path().join(".claude/settings.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"model":"opus"}"#).unwrap();

        let report = install(AgentKind::Claude, home.path()).unwrap();
        let backup = report.backup.expect("existing config must be backed up");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), r#"{"model":"opus"}"#);
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["model"], "opus", "unrelated keys survive the write");
    }

    #[test]
    fn install_codex_writes_config_toml() {
        let home = TempHome::new("codex");
        let report = install(AgentKind::Codex, home.path()).unwrap();
        assert!(report.changed);
        let text = std::fs::read_to_string(home.path().join(".codex/config.toml")).unwrap();
        assert!(text.contains("openmicro-hook"), "{text}");
        assert_eq!(hook_status(AgentKind::Codex, home.path()), HookStatus::Installed);
    }

    #[test]
    fn install_grok_uses_its_own_settings_file_and_agent_flag() {
        let home = TempHome::new("grok");
        let report = install(AgentKind::Grok, home.path()).unwrap();
        assert!(report.changed);
        assert!(report.path.ends_with(".grok/user-settings.json"), "{:?}", report.path);
        let text = std::fs::read_to_string(&report.path).unwrap();
        assert!(text.contains("--agent grok"), "{text}");
        assert_eq!(hook_status(AgentKind::Grok, home.path()), HookStatus::Installed);
    }

    #[test]
    fn hook_status_blocked_on_broken_config() {
        let home = TempHome::new("broken");
        let path = home.path().join(".claude/settings.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(hook_status(AgentKind::Claude, home.path()), HookStatus::Blocked(_)));
    }

    #[test]
    fn detect_marks_present_agents_and_preselects_missing_hooks() {
        let home = TempHome::new("detect");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        let rows = detect(home.path());
        let claude = rows.iter().find(|r| r.kind == AgentKind::Claude).unwrap();
        assert!(claude.present, "a ~/.claude dir means Claude Code is installed");
        assert_eq!(claude.status, HookStatus::Missing);
        assert!(claude.selected, "installed + missing hooks is preselected");
        assert!(claude.config_path.ends_with(".claude/settings.json"));

        let grok = rows.iter().find(|r| r.kind == AgentKind::Grok).unwrap();
        assert!(!grok.present, "no ~/.grok in this scratch home");
        assert!(!grok.selected, "an agent that isn't installed is never preselected");
    }

    #[test]
    fn detect_covers_every_adapter() {
        let home = TempHome::new("all");
        assert_eq!(detect(home.path()).len(), ALL_AGENTS.len());
    }

    #[test]
    fn agent_slug_roundtrips() {
        for a in ALL_AGENTS {
            assert_eq!(AgentKind::from_slug(a.slug()), Some(a));
        }
        assert_eq!(AgentKind::from_slug("nope"), None);
    }
}
