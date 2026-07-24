//! Command-line interface for the `openmicro` binary.
//!
//! `openmicro` is a CLI-with-default-TUI: with **no** subcommand it launches
//! the interactive TUI (see `main::run_tui`); with a subcommand it runs that
//! action and exits. The pure, testable pieces (argv parsing, the `status`
//! snapshot renderer, the agent/firmware summaries) live here; the
//! side-effecting runners are thin wrappers around them.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::agents::{self, AgentKind, HookStatus};
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
    /// Run the guided setup wizard (device check, firmware, agent hooks).
    Setup,
    /// List the coding agents found on this machine and their hook status.
    Agents,
    /// Install OpenMicro hooks into a coding agent's own config.
    InstallAgent {
        /// Which agent adapter to set up. Omit when using `--all`.
        agent: Option<AgentKind>,
        /// Install every detected agent that is missing its hooks.
        #[arg(long)]
        all: bool,
        /// Print the adapter's install documentation instead of changing anything.
        #[arg(long)]
        print: bool,
    },
    /// Obtain a flashable firmware image.
    Firmware {
        #[command(subcommand)]
        action: FirmwareAction,
    },
    /// Flash device firmware to a Micro 2 in bootloader mode.
    Flash {
        /// Path to the flashable image (defaults to the firmware build output).
        #[arg(long)]
        image: Option<PathBuf>,
        /// Serial port to flash over (e.g. /dev/ttyACM0); auto-detected if omitted.
        #[arg(long)]
        port: Option<String>,
    },
    /// Save the device's current flash so the stock firmware can be restored.
    Backup {
        /// Where to write the dump (defaults to ~/.local/share/openmicro).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        port: Option<String>,
    },
    /// Write a previously saved stock-firmware backup back to the device.
    Restore {
        /// Backup to write (defaults to the one `openmicro backup` created).
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        port: Option<String>,
    },
}

#[derive(Subcommand, Debug, PartialEq, Eq, Clone)]
pub enum FirmwareAction {
    /// Compile `firmware/` with the Xtensa toolchain.
    Build,
    /// List the published firmware versions.
    List,
    /// Fetch a prebuilt release image.
    Download {
        /// Release tag to install (e.g. v1.2.0). Defaults to the newest
        /// release that has a firmware asset, preferring stable over
        /// pre-release.
        #[arg(long)]
        version: Option<String>,
    },
    /// Report which firmware sources are available and which image would be used.
    Status,
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

/// Dispatch a parsed subcommand. Some arms terminate the process directly
/// (`status` on a missing daemon, `service` with the child's exit code) rather
/// than returning, which matches the "print and exit non-zero" contract.
///
/// `Setup` is handled by `main` (it needs the terminal), never here.
pub fn run(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Status => run_status(),
        Commands::Service { action } => run_service(action),
        Commands::Setup => unreachable!("`setup` is dispatched by main"),
        Commands::Agents => run_agents(),
        Commands::InstallAgent { agent, all, print } => run_install_agent(agent, all, print),
        Commands::Firmware { action } => run_firmware(action),
        Commands::Flash { image, port } => run_flash(image.as_deref(), port.as_deref()),
        Commands::Backup { out, port } => run_backup(out.as_deref(), port.as_deref()),
        Commands::Restore { image, port } => run_restore(image.as_deref(), port.as_deref()),
    }
}

/// Run the firmware flash. Prints the actionable guidance and exits non-zero on
/// the common "no image / no flash tool / not in bootloader" prerequisites — it
/// never claims success it did not perform.
fn run_flash(image: Option<&std::path::Path>, port: Option<&str>) -> anyhow::Result<()> {
    match crate::flash::flash(image, port) {
        Ok(()) => {
            println!("flash complete.");
            Ok(())
        }
        Err(msg) => {
            eprintln!("cannot flash: {msg}");
            std::process::exit(1);
        }
    }
}

/// Dump the device's flash to a file so a later `restore` is possible.
fn run_backup(out: Option<&std::path::Path>, port: Option<&str>) -> anyhow::Result<()> {
    match crate::flash::backup(out, port) {
        Ok((path, lines)) => {
            for l in lines {
                println!("{l}");
            }
            println!("keep {} safe — it is the only way back to stock.", path.display());
            Ok(())
        }
        Err(msg) => {
            eprintln!("cannot back up: {msg}");
            std::process::exit(1);
        }
    }
}

/// Write a saved stock-firmware backup back to the device.
fn run_restore(image: Option<&std::path::Path>, port: Option<&str>) -> anyhow::Result<()> {
    match crate::flash::restore(image, port) {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
            Ok(())
        }
        Err(msg) => {
            eprintln!("cannot restore: {msg}");
            std::process::exit(1);
        }
    }
}

/// Build or download a firmware image, or report what is available.
fn run_firmware(action: FirmwareAction) -> anyhow::Result<()> {
    let result = match action {
        FirmwareAction::Status => {
            print!("{}", format_firmware_status(&crate::firmware::Sources::detect()));
            return Ok(());
        }
        FirmwareAction::List => {
            match crate::firmware::fetch_releases() {
                Ok(releases) => {
                    print!("{}", format_releases(&releases));
                    return Ok(());
                }
                Err(msg) => {
                    eprintln!("{msg}");
                    std::process::exit(1);
                }
            }
        }
        FirmwareAction::Build => crate::firmware::build(),
        FirmwareAction::Download { version } => crate::firmware::download(version.as_deref()),
    };
    match result {
        Ok((path, lines)) => {
            for l in lines {
                println!("{l}");
            }
            println!("firmware image ready: {}", path.display());
            Ok(())
        }
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    }
}

/// Render the published-release list. Pure, so the wording is testable.
///
/// The version `firmware download` would pick without `--version` is marked, so
/// the default is visible rather than implied.
pub fn format_releases(releases: &[crate::firmware::Release]) -> String {
    if releases.is_empty() {
        return "no firmware releases published yet — build from source instead.\n".to_string();
    }
    let default = crate::firmware::pick_release(releases, None).ok().map(|r| r.tag.clone());
    let mut out = String::new();
    for r in releases {
        let marker = if Some(&r.tag) == default.as_ref() { "*" } else { " " };
        let note = match r.blocker() {
            Some(why) => format!("  [{why}]"),
            None => format!("  {} KiB", r.asset_size / 1024),
        };
        out.push_str(&format!("{marker} {}{note}\n", r.label()));
    }
    if default.is_some() {
        out.push_str("\n* installed by `openmicro firmware download` (override with --version)\n");
    }
    out
}

/// Render the firmware-source summary. Pure, so the wording is testable.
pub fn format_firmware_status(sources: &crate::firmware::Sources) -> String {
    let mut out = String::new();
    out.push_str(&format!("toolchain:    {}\n", sources.toolchain.describe()));
    out.push_str(&format!(
        "source dir:   {}\n",
        sources
            .firmware_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found (run from a checkout to build)".into())
    ));
    out.push_str(&format!(
        "releases:     {}{}\n",
        sources.url,
        if sources.forced { "  (forced by OPENMICRO_FIRMWARE_URL)" } else { "" }
    ));
    out.push_str(&format!(
        "image:        {}{}\n",
        sources
            .existing
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none yet — `openmicro firmware build|download`".into()),
        sources
            .cached_version
            .as_ref()
            .map(|v| format!("  (version {v})"))
            .unwrap_or_default()
    ));
    out.push_str(&format!(
        "can build: {}   can download: {}\n",
        sources.can_build(),
        sources.can_download()
    ));
    out
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

/// List every adapter with what we found on this machine.
fn run_agents() -> anyhow::Result<()> {
    print!("{}", format_agents(&agents::detect(&agents::home())));
    if agents::hook_binary().is_none() {
        eprintln!(
            "warning: openmicro-hook is not on PATH — installed hooks will do nothing until it is."
        );
    }
    Ok(())
}

/// Render the agent table. Pure, so the wording is testable.
pub fn format_agents(rows: &[agents::AgentRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let presence = if row.present { "installed" } else { "not found" };
        let status = match &row.status {
            HookStatus::Installed => "hooks: installed".to_string(),
            HookStatus::Missing => "hooks: missing".to_string(),
            HookStatus::Blocked(why) => format!("hooks: blocked — {why}"),
        };
        out.push_str(&format!("{:<8} {:<10} {}\n", row.kind.slug(), presence, status));
    }
    out
}

/// Install hooks for one agent, for every detected agent (`--all`), or just
/// print the adapter docs (`--print`).
fn run_install_agent(agent: Option<AgentKind>, all: bool, print: bool) -> anyhow::Result<()> {
    if print {
        let Some(agent) = agent else {
            eprintln!(
                "--print needs a specific agent, e.g. `openmicro install-agent claude --print`"
            );
            std::process::exit(2);
        };
        print_adapter_docs(agent);
        return Ok(());
    }

    let home = agents::home();
    let targets: Vec<AgentKind> = match (agent, all) {
        (Some(a), false) => vec![a],
        (None, true) => agents::detect(&home)
            .into_iter()
            .filter(|r| r.present && r.status == HookStatus::Missing)
            .map(|r| r.kind)
            .collect(),
        (Some(_), true) => {
            eprintln!("pass either an agent name or --all, not both");
            std::process::exit(2);
        }
        (None, false) => {
            eprintln!("specify an agent (e.g. `openmicro install-agent claude`) or --all");
            std::process::exit(2);
        }
    };

    if targets.is_empty() {
        println!("nothing to do: no detected agent is missing its OpenMicro hooks.");
        return Ok(());
    }

    if agents::hook_binary().is_none() {
        eprintln!(
            "warning: openmicro-hook is not on PATH — the hooks below will be installed but \
             will do nothing until it is."
        );
    }

    let mut failed = false;
    for kind in targets {
        match agents::install(kind, &home) {
            Ok(report) => println!("{}", report.summary()),
            Err(e) => {
                failed = true;
                eprintln!("{}: {e}", kind.slug());
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Print the adapter's install instructions from `adapters/<name>/install.md`
/// if readable, else a short pointer.
fn print_adapter_docs(agent: AgentKind) {
    let dir = agent.adapter_dir();
    let path = format!("adapters/{dir}/install.md");
    match std::fs::read_to_string(&path) {
        Ok(contents) => print!("{contents}"),
        Err(_) => println!(
            "No install docs found at {path}. See adapters/{dir}/ in the OpenMicro \
             source tree for setup steps for this agent."
        ),
    }
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
            Some(Commands::InstallAgent {
                agent: Some(AgentKind::Claude),
                all: false,
                print: false,
            })
        );
    }

    #[test]
    fn parses_install_agent_all_and_print() {
        let cli = Cli::try_parse_from(["openmicro", "install-agent", "--all"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::InstallAgent { agent: None, all: true, print: false })
        );
        let cli = Cli::try_parse_from(["openmicro", "install-agent", "codex", "--print"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::InstallAgent {
                agent: Some(AgentKind::Codex),
                all: false,
                print: true,
            })
        );
    }

    #[test]
    fn parses_setup_and_agents() {
        assert_eq!(
            Cli::try_parse_from(["openmicro", "setup"]).unwrap().command,
            Some(Commands::Setup)
        );
        assert_eq!(
            Cli::try_parse_from(["openmicro", "agents"]).unwrap().command,
            Some(Commands::Agents)
        );
    }

    #[test]
    fn parses_firmware_actions() {
        for (arg, expected) in [
            ("build", FirmwareAction::Build),
            ("list", FirmwareAction::List),
            ("download", FirmwareAction::Download { version: None }),
            ("status", FirmwareAction::Status),
        ] {
            let cli = Cli::try_parse_from(["openmicro", "firmware", arg]).unwrap();
            assert_eq!(cli.command, Some(Commands::Firmware { action: expected }));
        }
        assert!(Cli::try_parse_from(["openmicro", "firmware", "bogus"]).is_err());
    }

    #[test]
    fn parses_firmware_download_with_a_version() {
        let cli =
            Cli::try_parse_from(["openmicro", "firmware", "download", "--version", "v1.2.0"])
                .unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Firmware {
                action: FirmwareAction::Download { version: Some("v1.2.0".to_string()) }
            })
        );
    }

    #[test]
    fn format_releases_marks_the_default_and_flags_assetless_versions() {
        let releases = crate::firmware::parse_releases(
            r#"[{"tag_name":"v2.0.0-rc1","prerelease":true,"draft":false,
                 "published_at":"2026-07-22T00:00:00Z",
                 "assets":[{"name":"openmicro-fw.bin","size":204800,
                            "browser_download_url":"https://x/rc"}]},
                {"tag_name":"v1.0.0","prerelease":false,"draft":false,
                 "published_at":"2026-07-01T00:00:00Z",
                 "assets":[{"name":"openmicro-fw.bin","size":204800,
                            "browser_download_url":"https://x/stable"}]},
                {"tag_name":"v0.9.0","prerelease":false,"draft":false,
                 "published_at":"2026-06-01T00:00:00Z","assets":[]}]"#,
        )
        .unwrap();

        let text = format_releases(&releases);
        assert!(text.contains("v2.0.0-rc1") && text.contains("(pre-release)"), "{text}");
        // The newest *stable* build with an asset is the default.
        assert!(
            text.lines().any(|l| l.starts_with("* v1.0.0")),
            "default not marked:\n{text}"
        );
        assert!(text.contains("no firmware asset"), "assetless release not flagged:\n{text}");
        assert!(text.contains("--version"), "{text}");
    }

    #[test]
    fn format_releases_of_nothing_points_at_building() {
        assert!(format_releases(&[]).contains("build from source"));
    }

    #[test]
    fn parses_backup_and_restore() {
        let cli = Cli::try_parse_from(["openmicro", "backup", "--out", "/tmp/s.bin"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Backup { out: Some(PathBuf::from("/tmp/s.bin")), port: None })
        );
        let cli = Cli::try_parse_from(["openmicro", "restore"]).unwrap();
        assert_eq!(cli.command, Some(Commands::Restore { image: None, port: None }));
    }

    #[test]
    fn parses_flash_bare() {
        let cli = Cli::try_parse_from(["openmicro", "flash"]).unwrap();
        assert_eq!(cli.command, Some(Commands::Flash { image: None, port: None }));
    }

    #[test]
    fn parses_flash_with_image_and_port() {
        let cli = Cli::try_parse_from([
            "openmicro", "flash", "--image", "/tmp/fw.bin", "--port", "/dev/ttyACM0",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Flash {
                image: Some(PathBuf::from("/tmp/fw.bin")),
                port: Some("/dev/ttyACM0".to_string()),
            })
        );
    }

    #[test]
    fn rejects_unknown_service_action() {
        assert!(Cli::try_parse_from(["openmicro", "service", "bogus"]).is_err());
    }

    #[test]
    fn rejects_unknown_agent_name() {
        assert!(Cli::try_parse_from(["openmicro", "install-agent", "bogus"]).is_err());
    }

    #[test]
    fn service_action_maps_to_systemctl_verb() {
        assert_eq!(ServiceAction::Restart.as_arg(), "restart");
        assert_eq!(ServiceAction::Disable.as_arg(), "disable");
    }

    #[test]
    fn agent_name_maps_to_adapter_dir() {
        assert_eq!(AgentKind::Claude.adapter_dir(), "claude-code");
        assert_eq!(AgentKind::Grok.adapter_dir(), "grok-code");
    }

    #[test]
    fn format_agents_lists_every_adapter_with_its_status() {
        let rows = agents::detect(&std::env::temp_dir().join("openmicro-nonexistent-home"));
        let text = format_agents(&rows);
        for kind in agents::ALL_AGENTS {
            assert!(text.contains(kind.slug()), "{} missing from:\n{text}", kind.slug());
        }
        assert_eq!(text.lines().count(), agents::ALL_AGENTS.len(), "one row per agent");
    }

    #[test]
    fn format_firmware_status_reports_both_paths() {
        let sources = crate::firmware::Sources {
            toolchain: crate::firmware::Toolchain::Missing,
            firmware_dir: None,
            existing: None,
            url: "https://example/releases".into(),
            forced: false,
            cached_version: None,
            have_curl: true,
        };
        let text = format_firmware_status(&sources);
        assert!(text.contains("espup"), "{text}");
        assert!(text.contains("https://example/releases"), "{text}");
        assert!(text.contains("can build: false"), "{text}");
        assert!(text.contains("can download: true"), "{text}");
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
