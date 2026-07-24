//! Command-line interface for the `openmicro` binary.
//!
//! `openmicro` is a CLI-with-default-TUI: with **no** subcommand it launches
//! the interactive TUI (see `main::run_tui`); with a subcommand it runs that
//! action and exits. The pure, testable pieces (argv parsing and the
//! `status` snapshot renderer) live here; the side-effecting runners are thin
//! wrappers around them.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;

use clap::{Parser, Subcommand, ValueEnum};

use crate::client::parse_snapshot;

#[derive(Parser, Debug)]
#[command(
    name = "openmicro",
    version,
    about = "OpenMicro macropad control plane (run with no subcommand for the TUI)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    /// Print a one-shot summary of the daemon's current state.
    Status,
    /// Control the openmicrod systemd user service.
    Service {
        /// Action to forward to `systemctl --user ... openmicrod.service`.
        action: ServiceAction,
    },
    /// Print setup instructions for an agent adapter.
    InstallAgent {
        /// Which agent adapter to set up.
        agent: AgentName,
    },
    /// Flash device firmware (implemented in P6).
    Flash,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Status,
    Enable,
    Disable,
}

impl ServiceAction {
    /// The literal `systemctl` verb this action maps to.
    pub fn as_arg(self) -> &'static str {
        match self {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
            ServiceAction::Status => "status",
            ServiceAction::Enable => "enable",
            ServiceAction::Disable => "disable",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentName {
    Claude,
    Codex,
    Grok,
    T3,
}

impl AgentName {
    /// Directory under `adapters/` that holds this agent's install docs.
    pub fn adapter_dir(self) -> &'static str {
        match self {
            AgentName::Claude => "claude-code",
            AgentName::Codex => "codex",
            AgentName::Grok => "grok-code",
            AgentName::T3 => "t3-code",
        }
    }
}

/// Dispatch a parsed subcommand. Some arms terminate the process directly
/// (`status` on a missing daemon, `service` with the child's exit code) rather
/// than returning, which matches the "print and exit non-zero" contract.
pub fn run(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Status => run_status(),
        Commands::Service { action } => run_service(action),
        Commands::InstallAgent { agent } => run_install_agent(agent),
        Commands::Flash => {
            println!("firmware flashing: see `openmicro flash` in P6 / docs");
            Ok(())
        }
    }
}

/// Path to the daemon's control socket under `$XDG_RUNTIME_DIR`.
fn control_socket_path() -> String {
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    format!("{rt}/openmicro-ctl.sock")
}

/// Connect to the control socket, read one snapshot line, and print its
/// summary. If the daemon isn't running, print "daemon not running" to stderr
/// and exit non-zero.
fn run_status() -> anyhow::Result<()> {
    let path = control_socket_path();
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("daemon not running");
            std::process::exit(1);
        }
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    println!("{}", format_status(&line));
    Ok(())
}

/// Forward a service action to `systemctl --user ... openmicrod.service` and
/// exit with the child's status code.
fn run_service(action: ServiceAction) -> anyhow::Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", action.as_arg(), "openmicrod.service"])
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Print the adapter's install instructions from `adapters/<name>/install.md`
/// if readable, else a short pointer. Non-destructive by default.
fn run_install_agent(agent: AgentName) -> anyhow::Result<()> {
    let dir = agent.adapter_dir();
    let path = format!("adapters/{dir}/install.md");
    match std::fs::read_to_string(&path) {
        Ok(contents) => print!("{contents}"),
        Err(_) => println!(
            "No install docs found at {path}. See adapters/{dir}/ in the OpenMicro \
             source tree for setup steps for this agent."
        ),
    }
    Ok(())
}

/// Render a control-socket snapshot line into a human-readable summary.
///
/// Pure: takes the raw JSON line, returns the text to print. Empty or
/// malformed input yields a single clear "no data" line rather than an error.
pub fn format_status(snapshot_json: &str) -> String {
    let Some(snap) = parse_snapshot(snapshot_json) else {
        return "no data (empty or malformed snapshot from daemon)".to_string();
    };

    let mut out = String::new();
    let owner = snap.owner.as_deref().unwrap_or("(none)");
    out.push_str(&format!("owner:   {owner}\n"));

    match snap.battery {
        Some(pct) => {
            let charging = if snap.charging { " (charging)" } else { "" };
            out.push_str(&format!("battery: {pct}%{charging}\n"));
        }
        None => out.push_str("battery: unknown\n"),
    }

    if snap.sessions.is_empty() {
        out.push_str("sessions: none");
    } else {
        out.push_str(&format!("sessions ({}):", snap.sessions.len()));
        for s in &snap.sessions {
            let slot = s.slot.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "\n  [slot {slot}] {}:{} -> {}",
                s.agent, s.session, s.state
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_is_none() {
        let cli = Cli::try_parse_from(["openmicro"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_status_subcommand() {
        let cli = Cli::try_parse_from(["openmicro", "status"]).unwrap();
        assert_eq!(cli.command, Some(Commands::Status));
    }

    #[test]
    fn parses_service_start() {
        let cli = Cli::try_parse_from(["openmicro", "service", "start"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Service { action: ServiceAction::Start })
        );
    }

    #[test]
    fn parses_install_agent() {
        let cli = Cli::try_parse_from(["openmicro", "install-agent", "claude"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::InstallAgent { agent: AgentName::Claude })
        );
    }

    #[test]
    fn rejects_unknown_service_action() {
        assert!(Cli::try_parse_from(["openmicro", "service", "bogus"]).is_err());
    }

    #[test]
    fn service_action_maps_to_systemctl_verb() {
        assert_eq!(ServiceAction::Restart.as_arg(), "restart");
        assert_eq!(ServiceAction::Disable.as_arg(), "disable");
    }

    #[test]
    fn agent_name_maps_to_adapter_dir() {
        assert_eq!(AgentName::Claude.adapter_dir(), "claude-code");
        assert_eq!(AgentName::T3.adapter_dir(), "t3-code");
    }

    #[test]
    fn format_status_empty_is_no_data() {
        assert!(format_status("").contains("no data"));
    }

    #[test]
    fn format_status_garbage_is_no_data() {
        assert!(format_status("not json").contains("no data"));
    }

    #[test]
    fn format_status_renders_owner_battery_sessions() {
        let line = r#"{"sessions":[{"agent":"claude","session":"s1","state":"working","slot":0}],"owner":"claude:s1","battery":84,"charging":true}"#;
        let out = format_status(line);
        assert!(out.contains("claude:s1"), "owner missing: {out}");
        assert!(out.contains("84%"), "battery missing: {out}");
        assert!(out.contains("charging"), "charging missing: {out}");
        assert!(out.contains("working"), "state missing: {out}");
    }

    #[test]
    fn format_status_no_sessions_and_unknown_battery() {
        let line = r#"{"sessions":[],"owner":null}"#;
        let out = format_status(line);
        assert!(out.contains("none"), "expected 'none' sessions: {out}");
        assert!(out.contains("unknown"), "expected unknown battery: {out}");
    }
}
