//! Taking OpenMicro back off the machine.
//!
//! Deliberately itemised rather than a single "remove everything" button: the
//! stock-firmware backup is the *only* route back to the vendor firmware, and
//! the agent hooks live inside config files the user owns. Both deserve a
//! separate, informed yes.
//!
//! [`survey`] answers "what is actually here", so the UI can offer only the
//! things that exist and say how big each one is; [`remove`] does the work and
//! reports per-target outcomes.

use std::path::{Path, PathBuf};

/// One removable piece of an OpenMicro installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// The systemd user unit (stopped and disabled first).
    Service,
    /// `openmicrod`, `openmicro`, `openmicro-hook` in `~/.local/bin`.
    Binaries,
    /// Hook entries inside each coding agent's own config file.
    AgentHooks,
    /// `~/.config/openmicro` — settings and the first-run marker.
    Config,
    /// `~/.cache/openmicro` — downloaded firmware images.
    Cache,
    /// `~/.local/share/openmicro` — the stock-firmware backup.
    StockBackup,
}

/// Every target, in the order they should be offered and removed. Service first
/// so nothing is holding files open, backup last because it is the dangerous one.
pub const ALL_TARGETS: [Target; 6] = [
    Target::Service,
    Target::Binaries,
    Target::AgentHooks,
    Target::Config,
    Target::Cache,
    Target::StockBackup,
];

impl Target {
    /// Short label for the picker.
    pub fn label(self) -> &'static str {
        match self {
            Target::Service => "Background service",
            Target::Binaries => "Installed binaries",
            Target::AgentHooks => "Coding-agent hooks",
            Target::Config => "Settings",
            Target::Cache => "Downloaded firmware",
            Target::StockBackup => "Stock-firmware backup",
        }
    }

    /// Whether removing this can be undone by reinstalling.
    ///
    /// Only the stock-firmware backup cannot: it is a dump of the user's own
    /// device that nobody publishes, so deleting it permanently removes the
    /// ability to go back to the vendor firmware.
    pub fn is_irreversible(self) -> bool {
        self == Target::StockBackup
    }

    /// Whether this is selected by default in the picker.
    pub fn default_selected(self) -> bool {
        !self.is_irreversible()
    }

    /// Stable id used as the multiselect value.
    pub fn id(self) -> &'static str {
        match self {
            Target::Service => "service",
            Target::Binaries => "binaries",
            Target::AgentHooks => "hooks",
            Target::Config => "config",
            Target::Cache => "cache",
            Target::StockBackup => "backup",
        }
    }

    /// Parse an id back into a target.
    pub fn from_id(id: &str) -> Option<Target> {
        ALL_TARGETS.into_iter().find(|t| t.id() == id)
    }
}

/// What a target looks like on this machine right now.
#[derive(Debug, Clone)]
pub struct Found {
    pub target: Target,
    /// False when there is nothing to remove.
    pub present: bool,
    /// Paths (or config files) that would be touched.
    pub paths: Vec<PathBuf>,
    /// Extra context for the picker hint, e.g. a size or a warning.
    pub detail: String,
}

/// The three binaries `packaging/install.sh` puts in `~/.local/bin`.
pub const BINARIES: [&str; 3] = ["openmicrod", "openmicro", "openmicro-hook"];

/// Inspect the machine and describe every target.
pub fn survey(home: &Path) -> Vec<Found> {
    ALL_TARGETS.into_iter().map(|t| survey_one(t, home)).collect()
}

fn survey_one(target: Target, home: &Path) -> Found {
    match target {
        Target::Service => {
            let path = crate::daemon::unit_path();
            let present = path.is_file();
            Found {
                target,
                present,
                detail: if present {
                    "stopped and disabled first".to_string()
                } else {
                    "not installed".to_string()
                },
                paths: vec![path],
            }
        }
        Target::Binaries => {
            let paths: Vec<PathBuf> = BINARIES
                .iter()
                .map(|b| home.join(".local/bin").join(b))
                .filter(|p| p.exists())
                .collect();
            Found {
                target,
                present: !paths.is_empty(),
                detail: match paths.len() {
                    0 => "none found".to_string(),
                    n => format!("{n} in ~/.local/bin"),
                },
                paths,
            }
        }
        Target::AgentHooks => {
            let paths: Vec<PathBuf> = crate::agents::detect(home)
                .into_iter()
                .filter(|r| r.status == crate::agents::HookStatus::Installed)
                .map(|r| r.config_path)
                .collect();
            Found {
                target,
                present: !paths.is_empty(),
                detail: match paths.len() {
                    0 => "none installed".to_string(),
                    n => format!("{n} agent config(s), merged back out"),
                },
                paths,
            }
        }
        Target::Config => dir_target(target, home.join(".config/openmicro"), "settings"),
        Target::Cache => {
            dir_target(target, home.join(".cache/openmicro"), "re-downloadable")
        }
        Target::StockBackup => {
            let dir = home.join(".local/share/openmicro");
            let backup = crate::flash::backup_path();
            let present = dir.exists();
            let detail = if backup.is_file() {
                let size = std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
                format!(
                    "{} MiB — the ONLY way back to the vendor firmware",
                    size / (1024 * 1024)
                )
            } else if present {
                "no backup image inside".to_string()
            } else {
                "nothing saved".to_string()
            };
            Found { target, present, paths: vec![dir], detail }
        }
    }
}

/// Describe a plain directory target.
fn dir_target(target: Target, dir: PathBuf, when_present: &str) -> Found {
    let present = dir.exists();
    Found {
        target,
        present,
        detail: if present { when_present.to_string() } else { "nothing saved".to_string() },
        paths: vec![dir],
    }
}

/// The outcome of removing one target.
#[derive(Debug, Clone)]
pub struct Removed {
    pub target: Target,
    /// Lines to show the user.
    pub lines: Vec<String>,
    /// Set when the target could not be fully removed.
    pub error: Option<String>,
}

/// Remove the chosen targets, in [`ALL_TARGETS`] order regardless of the order
/// they were selected in — the service has to go first so nothing is holding
/// the binaries open.
///
/// Best-effort per target: one failure does not abort the rest, and every
/// outcome is reported, so the UI can never claim a clean uninstall it did not
/// achieve.
pub fn remove(chosen: &[Target], home: &Path) -> Vec<Removed> {
    let mut ordered: Vec<Target> =
        ALL_TARGETS.into_iter().filter(|t| chosen.contains(t)).collect();
    ordered.dedup();
    ordered.into_iter().map(|t| remove_one(t, home)).collect()
}

fn remove_one(target: Target, home: &Path) -> Removed {
    let mut lines = Vec::new();
    let mut error = None;

    match target {
        Target::Service => {
            // Ask systemd to stop and forget it before the unit file goes, so
            // it does not linger as a failed unit until the next daemon-reload.
            if crate::daemon::have_systemctl() {
                match crate::daemon::systemctl("disable") {
                    Ok(_) => lines.push("service disabled".to_string()),
                    // Not installed / not enabled is a perfectly fine state to
                    // be uninstalling from.
                    Err(e) => lines.push(format!("disable: {e}")),
                }
                let _ = crate::daemon::stop();
            }
            let unit = crate::daemon::unit_path();
            match remove_path(&unit) {
                Ok(true) => lines.push(format!("removed {}", unit.display())),
                Ok(false) => lines.push("no unit file to remove".to_string()),
                Err(e) => error = Some(e),
            }
            if crate::daemon::have_systemctl() {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
            }
        }
        Target::Binaries => {
            for name in BINARIES {
                let path = home.join(".local/bin").join(name);
                match remove_path(&path) {
                    Ok(true) => lines.push(format!("removed {}", path.display())),
                    Ok(false) => {}
                    Err(e) => error = Some(e),
                }
            }
            if lines.is_empty() {
                lines.push("no binaries found".to_string());
            }
        }
        Target::AgentHooks => {
            let mut touched = false;
            for kind in crate::agents::ALL_AGENTS {
                match crate::agents::uninstall(kind, home) {
                    Ok(report) if report.changed => {
                        touched = true;
                        lines.push(format!("{}: hooks removed", kind.slug()));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error = Some(e.clone());
                        lines.push(format!("{}: {e}", kind.slug()));
                    }
                }
            }
            if !touched && error.is_none() {
                lines.push("no agent hooks were installed".to_string());
            }
        }
        Target::Config | Target::Cache | Target::StockBackup => {
            let dir = match target {
                Target::Config => home.join(".config/openmicro"),
                Target::Cache => home.join(".cache/openmicro"),
                _ => home.join(".local/share/openmicro"),
            };
            match remove_path(&dir) {
                Ok(true) => lines.push(format!("removed {}", dir.display())),
                Ok(false) => lines.push("nothing to remove".to_string()),
                Err(e) => error = Some(e),
            }
        }
    }

    Removed { target, lines, error }
}

/// Remove a file or directory. `Ok(false)` means it was not there to begin
/// with, which is a success for our purposes, not an error.
fn remove_path(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map(|_| true).map_err(|e| format!("cannot remove {}: {e}", path.display()))
}

/// One-line summary of a completed uninstall.
pub fn summarise(results: &[Removed]) -> String {
    let failed = results.iter().filter(|r| r.error.is_some()).count();
    match (results.len(), failed) {
        (0, _) => "Nothing selected.".to_string(),
        (n, 0) => format!("Removed {n} item(s)."),
        (n, f) => format!("Removed {} of {n} item(s); {f} failed.", n - f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch `$HOME` that cleans itself up.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("openmicro-uninstall-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            TempHome(base)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn touch(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            p
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_the_stock_backup_is_irreversible_and_unselected_by_default() {
        for t in ALL_TARGETS {
            assert_eq!(
                t.is_irreversible(),
                t == Target::StockBackup,
                "{t:?} irreversibility"
            );
            assert_eq!(t.default_selected(), !t.is_irreversible(), "{t:?} default");
        }
    }

    #[test]
    fn target_ids_round_trip_and_are_unique() {
        let mut ids: Vec<&str> = ALL_TARGETS.iter().map(|t| t.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "target ids must be unique");
        for t in ALL_TARGETS {
            assert_eq!(Target::from_id(t.id()), Some(t));
        }
        assert_eq!(Target::from_id("nope"), None);
    }

    #[test]
    fn survey_reports_absent_targets_as_not_present() {
        let home = TempHome::new("empty");
        for found in survey(home.path()) {
            if found.target == Target::Service {
                continue; // depends on the real machine's unit file
            }
            assert!(!found.present, "{:?} should be absent: {found:?}", found.target);
        }
    }

    #[test]
    fn survey_finds_installed_binaries_and_directories() {
        let home = TempHome::new("full");
        home.touch(".local/bin/openmicro");
        home.touch(".local/bin/openmicrod");
        home.touch(".config/openmicro/config.toml");
        home.touch(".cache/openmicro/firmware/openmicro-fw.bin");

        let found = survey(home.path());
        let bins = found.iter().find(|f| f.target == Target::Binaries).unwrap();
        assert!(bins.present);
        assert_eq!(bins.paths.len(), 2, "only the two that exist");
        assert!(bins.detail.contains('2'), "{}", bins.detail);

        assert!(found.iter().find(|f| f.target == Target::Config).unwrap().present);
        assert!(found.iter().find(|f| f.target == Target::Cache).unwrap().present);
    }

    #[test]
    fn remove_deletes_the_chosen_targets_and_leaves_the_rest() {
        let home = TempHome::new("remove");
        home.touch(".local/bin/openmicro");
        home.touch(".config/openmicro/config.toml");
        home.touch(".cache/openmicro/firmware/openmicro-fw.bin");

        let results = remove(&[Target::Binaries, Target::Cache], home.path());
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");

        assert!(!home.path().join(".local/bin/openmicro").exists());
        assert!(!home.path().join(".cache/openmicro").exists());
        // Not selected, so it must survive.
        assert!(home.path().join(".config/openmicro/config.toml").exists());
    }

    #[test]
    fn remove_runs_in_a_safe_order_regardless_of_selection_order() {
        // The service must be handled first so nothing holds the binaries open.
        let home = TempHome::new("order");
        let results = remove(&[Target::Cache, Target::Service, Target::Binaries], home.path());
        let order: Vec<Target> = results.iter().map(|r| r.target).collect();
        assert_eq!(order, vec![Target::Service, Target::Binaries, Target::Cache]);
    }

    #[test]
    fn removing_something_absent_is_not_an_error() {
        let home = TempHome::new("absent");
        let results = remove(&[Target::Config, Target::Cache], home.path());
        assert!(results.iter().all(|r| r.error.is_none()), "{results:?}");
        assert!(results[0].lines.iter().any(|l| l.contains("nothing to remove")));
    }

    #[test]
    fn removing_agent_hooks_takes_them_back_out_of_the_config() {
        let home = TempHome::new("hooks");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), r#"{"model":"opus"}"#).unwrap();
        crate::agents::install(crate::agents::AgentKind::Claude, home.path()).unwrap();

        let results = remove(&[Target::AgentHooks], home.path());
        assert!(results[0].error.is_none(), "{results:?}");
        assert!(results[0].lines.iter().any(|l| l.contains("claude")), "{results:?}");

        let after = std::fs::read_to_string(home.path().join(".claude/settings.json")).unwrap();
        assert!(!after.contains("openmicro-hook"), "{after}");
        assert!(after.contains("opus"), "the user's own settings survive: {after}");
    }

    #[test]
    fn summaries_never_claim_a_clean_run_when_something_failed() {
        let ok = Removed { target: Target::Cache, lines: vec![], error: None };
        let bad = Removed {
            target: Target::Config,
            lines: vec![],
            error: Some("permission denied".to_string()),
        };
        assert_eq!(summarise(&[]), "Nothing selected.");
        assert_eq!(summarise(std::slice::from_ref(&ok)), "Removed 1 item(s).");
        let mixed = summarise(&[ok, bad]);
        assert!(mixed.contains("1 failed"), "{mixed}");
    }
}
