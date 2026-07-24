//! The first-run setup wizard: from "is anything plugged in?" to a flashed
//! device with every installed coding agent wired up.
//!
//! The flow the wizard implements:
//!
//! 1. **Warning** — custom firmware is not what the vendor intends; explain
//!    that, and that going back is only possible from a backup.
//! 2. **Detect** — probe USB and BLE with a spinner until something is found.
//! 3. If the device already runs OpenMicro firmware, jump straight to step 6.
//! 4. Otherwise: if it is only reachable over Bluetooth, ask for a cable; then
//!    ask for the boot button, polling until the device shows up in ROM
//!    bootloader mode.
//! 5. **Firmware** — back up the stock image, build or download OpenMicro, flash.
//! 6. **Agents** — detect installed coding agents and install their hooks.
//!
//! Everything that decides *what screen to show* is a pure function of a
//! [`Probe`] here, so the whole branching table is unit-tested without
//! hardware; the rendering lives in `ui` and the side effects in `main`.

use std::path::PathBuf;

use crate::agents::{AgentKind, AgentRow};
use crate::firmware::{Release, Sources};
use crate::probe::{Connection, FirmwareKind, Probe};

/// Marker file (under `$HOME`) written once setup has been completed or
/// explicitly skipped, so the wizard only takes over the TUI on first run.
pub const SETUP_MARKER_REL: &str = ".config/openmicro/setup-done";

/// Frames of the "working…" spinner, advanced once per input-poll tick.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The warning shown before anything is touched. Deliberately explicit: this is
/// a one-way door unless the user takes a backup first.
pub const WARNING: &[&str] = &[
    "OpenMicro replaces the firmware on your Creator Micro 2 with its own build.",
    "",
    "• This is NOT an intended use of the device. The vendor does not support it,",
    "  does not publish its firmware, and it may void any warranty you have.",
    "• Going back to the stock firmware IS possible — but only from a full backup",
    "  taken from your own device BEFORE flashing. This wizard can take that",
    "  backup for you, and `openmicro restore` writes it back.",
    "• Without a backup there is no published stock image to return to.",
    "• Flashing needs a USB cable and the boot button; a failed flash is",
    "  recoverable by re-entering bootloader mode, but do not unplug mid-write.",
];

/// Which screen the wizard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Custom-firmware warning; needs an explicit acceptance.
    Warning,
    /// Looking for the device (spinner).
    Detect,
    /// Device is cabled but running its firmware: ask for the boot button.
    NeedBootMode,
    /// Device is only reachable over Bluetooth: ask for a cable first.
    NeedCable,
    /// In bootloader mode: choose backup / build / download / flash.
    Firmware,
    /// Pick which published firmware version to download.
    Releases,
    /// Flash finished; tell the user to reset the device.
    Flashed,
    /// Pick which installed coding agents to wire up.
    Agents,
    /// Everything done.
    Done,
}

impl Stage {
    /// True while the wizard is polling for a hardware change, i.e. the screen
    /// should show a spinner and the probe should keep running.
    pub fn is_waiting(self) -> bool {
        matches!(self, Stage::Detect | Stage::NeedBootMode | Stage::NeedCable)
    }

    /// True when the screen should keep probing in the background.
    ///
    /// The waiting screens plus the firmware menu: unplugging the device while
    /// the menu is open must grey out "Flash" instead of leaving a stale, then
    /// failing, action. [`auto_stage`] still refuses to *move* off a menu, so
    /// polling here only refreshes what it shows.
    pub fn polls(self) -> bool {
        self.is_waiting() || self == Stage::Firmware
    }
}

/// The screen a [`Probe`] implies, given the one we are on.
///
/// Only the waiting stages react to hardware; the rest are driven by the user
/// so that a passing USB glitch can never yank a menu out from under them.
pub fn auto_stage(current: Stage, probe: Probe) -> Stage {
    if !current.is_waiting() {
        return current;
    }
    match probe.firmware() {
        // Ready to flash — that is what every waiting stage is waiting for.
        FirmwareKind::Bootloader => Stage::Firmware,
        // Already running our firmware: nothing to flash, go wire the agents.
        FirmwareKind::OpenMicro => Stage::Agents,
        // Something is there but not ours: the next step depends on whether we
        // can reach it over a cable.
        FirmwareKind::Stock | FirmwareKind::Unknown => match probe.connection() {
            Connection::Cable => Stage::NeedBootMode,
            Connection::Ble => Stage::NeedCable,
            // Nothing found at all: keep looking, unless we had already moved
            // past detection (the device was unplugged — go back to looking).
            Connection::None => Stage::Detect,
        },
    }
}

/// An action offered on the firmware screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Dump the current (stock) flash so a restore is possible later.
    BackupStock,
    /// Build the firmware from the `firmware/` crate.
    Build,
    /// Download a prebuilt release image.
    Download,
    /// Write the resolved image to the device.
    Flash,
}

/// One row of the firmware action menu.
#[derive(Debug, Clone)]
pub struct ActionItem {
    pub action: Action,
    pub label: String,
    /// Whether it can be run right now.
    pub available: bool,
    /// Why not, or extra context when it can.
    pub note: String,
}

/// Build the firmware action menu from the current environment.
///
/// Pure (all the I/O is done by the caller and passed in) so the availability
/// rules — in particular "you cannot flash without an image" and "you cannot
/// back up what is not in bootloader mode" — are unit-tested.
pub fn actions(
    sources: &Sources,
    probe: Probe,
    backup: Option<&PathBuf>,
    image: Option<&PathBuf>,
) -> Vec<ActionItem> {
    let ready = probe.bootloader_ready();

    let backup_note = match backup {
        Some(p) => format!("already saved: {}", p.display()),
        None if ready => "recommended — this is the only way back to stock".to_string(),
        None => "needs the device in bootloader mode".to_string(),
    };

    let build_note = sources
        .build_blocker()
        .unwrap_or_else(|| "compiles firmware/ with the Xtensa toolchain".to_string());

    let download_note = match (sources.can_download(), sources.forced) {
        (false, _) => "curl not found".to_string(),
        (true, true) => format!("from {}", sources.url),
        (true, false) => match &sources.cached_version {
            Some(v) => format!("choose a published version (cached: {v})"),
            None => "choose from the published versions".to_string(),
        },
    };

    let flash_note = match (image, ready) {
        (Some(p), true) => format!("write {}", p.display()),
        (Some(_), false) => "needs the device in bootloader mode".to_string(),
        (None, _) => "no firmware image yet — build or download one first".to_string(),
    };

    vec![
        ActionItem {
            action: Action::BackupStock,
            label: "Back up the stock firmware".to_string(),
            available: ready && backup.is_none(),
            note: backup_note,
        },
        ActionItem {
            action: Action::Build,
            label: "Build firmware from source".to_string(),
            available: sources.can_build(),
            note: build_note,
        },
        ActionItem {
            action: Action::Download,
            label: "Download prebuilt firmware".to_string(),
            available: sources.can_download(),
            note: download_note,
        },
        ActionItem {
            action: Action::Flash,
            label: "Flash the device".to_string(),
            available: ready && image.is_some(),
            note: flash_note,
        },
    ]
}

/// Work handed to the background thread so the UI never blocks.
#[derive(Debug, Clone)]
pub enum Job {
    Probe,
    Backup,
    Build,
    /// Fetch the published release list, to populate the version picker.
    FetchReleases,
    /// Download one specific release's firmware asset.
    DownloadRelease(Release),
    Flash,
    InstallAgents(Vec<AgentKind>),
}

impl Job {
    /// Text shown next to the spinner while this job runs.
    pub fn label(&self) -> String {
        match self {
            Job::Probe => "looking for the device".to_string(),
            Job::Backup => "reading the stock firmware (this takes a minute)".to_string(),
            Job::Build => "building firmware".to_string(),
            Job::FetchReleases => "fetching the list of firmware versions".to_string(),
            Job::DownloadRelease(r) => format!("downloading firmware {}", r.tag),
            Job::Flash => "flashing".to_string(),
            Job::InstallAgents(a) => format!("installing hooks for {} agent(s)", a.len()),
        }
    }
}

/// A message from the background worker back to the UI thread.
#[derive(Debug)]
pub enum JobMsg {
    /// A fresh device observation.
    Probed(Probe),
    /// The published release list (or why it could not be fetched).
    Releases(Result<Vec<Release>, String>),
    /// A job finished: `Ok` carries its output lines, `Err` the failure text.
    Finished { job: Job, result: Result<Vec<String>, String> },
}

/// Everything the wizard screens render from.
pub struct Wizard {
    pub stage: Stage,
    /// Latest device observation (`None` until the first probe returns).
    pub probe: Option<Probe>,
    /// A job currently running in the background, if any.
    pub busy: Option<Job>,
    /// Animated spinner frame.
    pub tick: usize,
    /// Rolling output/log lines from jobs.
    pub log: Vec<String>,
    /// Firmware action menu, and the cursor into it.
    pub actions: Vec<ActionItem>,
    pub action_sel: usize,
    /// Agent picker rows, and the cursor into them.
    pub agents: Vec<AgentRow>,
    pub agent_sel: usize,
    /// Published firmware versions, and the cursor into them. Empty until the
    /// release list has been fetched.
    pub releases: Vec<Release>,
    pub release_sel: usize,
    /// Resolved firmware image, when one exists.
    pub image: Option<PathBuf>,
    /// Existing stock-firmware backup, when one exists.
    pub backup: Option<PathBuf>,
    /// Firmware source availability.
    pub sources: Sources,
}

/// Cap on retained log lines, so a long build cannot grow the UI state without
/// bound. The tail is what matters (errors come last).
const LOG_LIMIT: usize = 400;

impl Wizard {
    pub fn new() -> Wizard {
        let mut w = Wizard {
            stage: Stage::Warning,
            probe: None,
            busy: None,
            tick: 0,
            log: Vec::new(),
            actions: Vec::new(),
            action_sel: 0,
            agents: Vec::new(),
            agent_sel: 0,
            releases: Vec::new(),
            release_sel: 0,
            image: None,
            backup: None,
            sources: Sources::detect(),
        };
        w.refresh_env();
        w
    }

    /// Re-read everything that lives on disk: firmware sources, the resolved
    /// image, the stock backup, and the agent rows. Called on entry to a screen
    /// and after any job that could have changed them.
    pub fn refresh_env(&mut self) {
        self.sources = Sources::detect();
        self.image = crate::flash::resolve_image(None).ok();
        let backup = crate::flash::backup_path();
        self.backup = backup.is_file().then_some(backup);
        self.agents = crate::agents::detect(&crate::agents::home());
        if self.agent_sel >= self.agents.len() {
            self.agent_sel = self.agents.len().saturating_sub(1);
        }
        self.rebuild_actions();
    }

    /// Recompute the firmware action menu from the current probe + environment.
    pub fn rebuild_actions(&mut self) {
        self.actions = actions(
            &self.sources,
            self.probe.unwrap_or_default(),
            self.backup.as_ref(),
            self.image.as_ref(),
        );
        if self.action_sel >= self.actions.len() {
            self.action_sel = self.actions.len().saturating_sub(1);
        }
    }

    /// Fold in a new probe result and let it move the stage.
    pub fn on_probe(&mut self, probe: Probe) {
        self.probe = Some(probe);
        let next = auto_stage(self.stage, probe);
        if next != self.stage {
            self.stage = next;
            self.refresh_env();
        } else {
            self.rebuild_actions();
        }
    }

    /// Append job output, keeping only the most recent [`LOG_LIMIT`] lines.
    pub fn push_log<I: IntoIterator<Item = String>>(&mut self, lines: I) {
        self.log.extend(lines);
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }

    /// The currently highlighted firmware action, if the menu is non-empty.
    pub fn selected_action(&self) -> Option<&ActionItem> {
        self.actions.get(self.action_sel)
    }

    /// Move the firmware-menu cursor.
    pub fn action_move(&mut self, delta: i32) {
        self.action_sel = move_cursor(self.action_sel, self.actions.len(), delta);
    }

    /// Move the agent-picker cursor.
    pub fn agent_move(&mut self, delta: i32) {
        self.agent_sel = move_cursor(self.agent_sel, self.agents.len(), delta);
    }

    /// Move the version-picker cursor.
    pub fn release_move(&mut self, delta: i32) {
        self.release_sel = move_cursor(self.release_sel, self.releases.len(), delta);
    }

    /// The highlighted firmware version, if the list is non-empty.
    pub fn selected_release(&self) -> Option<&Release> {
        self.releases.get(self.release_sel)
    }

    /// Fold in a fetched release list: on success, open the picker with the
    /// cursor on the version [`pick_release`](crate::firmware::pick_release)
    /// would have chosen, so pressing Enter straight away does the sensible
    /// thing. On failure, stay put and log why.
    pub fn on_releases(&mut self, result: Result<Vec<Release>, String>) {
        match result {
            Ok(releases) if releases.is_empty() => {
                self.push_log([
                    "no firmware releases published yet — build from source instead.".to_string(),
                ]);
            }
            Ok(releases) => {
                let default = crate::firmware::pick_release(&releases, None)
                    .ok()
                    .and_then(|r| releases.iter().position(|x| x.tag == r.tag))
                    .unwrap_or(0);
                self.releases = releases;
                self.release_sel = default;
                self.stage = Stage::Releases;
            }
            Err(e) => self.push_log(e.lines().map(|l| l.to_string())),
        }
    }

    /// Toggle the highlighted agent, refusing to select ones that cannot be
    /// installed (unsupported mechanism, or a config we must not touch).
    pub fn toggle_agent(&mut self) {
        if let Some(row) = self.agents.get_mut(self.agent_sel) {
            if is_installable(row) {
                row.selected = !row.selected;
            }
        }
    }

    /// Every agent currently ticked in the picker.
    pub fn chosen_agents(&self) -> Vec<AgentKind> {
        self.agents
            .iter()
            .filter(|r| r.selected && is_installable(r))
            .map(|r| r.kind)
            .collect()
    }

    /// Advance the spinner.
    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    /// The current spinner glyph.
    pub fn spinner(&self) -> char {
        SPINNER[self.tick % SPINNER.len()]
    }
}

impl Default for Wizard {
    fn default() -> Self {
        Wizard::new()
    }
}

/// Whether an agent row can have hooks installed into it. False only when the
/// agent's own config blocks a safe merge.
pub fn is_installable(row: &AgentRow) -> bool {
    !matches!(row.status, crate::agents::HookStatus::Blocked(_))
}

/// Move a cursor by `delta` within `len`, clamping at both ends. A zero-length
/// list keeps the cursor at 0.
fn move_cursor(cur: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let max = (len - 1) as i32;
    (cur as i32 + delta).clamp(0, max) as usize
}

/// Absolute path of the first-run marker file.
pub fn setup_marker() -> PathBuf {
    crate::agents::home().join(SETUP_MARKER_REL)
}

/// True when the wizard should take over on launch (no marker yet).
pub fn setup_needed() -> bool {
    !setup_marker().is_file()
}

/// Record that setup has been completed (or deliberately skipped), so the next
/// launch goes straight to the dashboard. Best-effort: failing to write the
/// marker only means the wizard offers itself again.
pub fn mark_setup_done() {
    let path = setup_marker();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, "openmicro setup completed\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::DeviceState;
    use crate::probe::BleState;

    fn probe(usb: DeviceState, ble: BleState) -> Probe {
        Probe { usb, ble }
    }

    #[test]
    fn detect_waits_while_nothing_is_connected() {
        let p = probe(DeviceState::Absent, BleState::Absent);
        assert_eq!(auto_stage(Stage::Detect, p), Stage::Detect);
    }

    #[test]
    fn detect_jumps_to_agents_when_our_firmware_is_already_running() {
        let p = probe(DeviceState::Absent, BleState::OpenMicro);
        assert_eq!(auto_stage(Stage::Detect, p), Stage::Agents);
    }

    #[test]
    fn detect_asks_for_a_cable_when_only_ble_sees_a_stock_device() {
        let p = probe(DeviceState::Absent, BleState::StockLike);
        assert_eq!(auto_stage(Stage::Detect, p), Stage::NeedCable);
    }

    #[test]
    fn detect_asks_for_boot_mode_when_cabled_but_running() {
        let p = probe(DeviceState::NormalDevice, BleState::Unavailable);
        assert_eq!(auto_stage(Stage::Detect, p), Stage::NeedBootMode);
    }

    #[test]
    fn bootloader_mode_opens_the_firmware_menu_from_every_waiting_stage() {
        let p = probe(DeviceState::Bootloader, BleState::Unavailable);
        for stage in [Stage::Detect, Stage::NeedCable, Stage::NeedBootMode] {
            assert_eq!(auto_stage(stage, p), Stage::Firmware, "{stage:?}");
        }
    }

    #[test]
    fn plugging_a_cable_in_switches_from_cable_prompt_to_boot_prompt() {
        let p = probe(DeviceState::NormalDevice, BleState::Unavailable);
        assert_eq!(auto_stage(Stage::NeedCable, p), Stage::NeedBootMode);
    }

    #[test]
    fn unplugging_returns_to_detection() {
        let p = probe(DeviceState::Absent, BleState::Absent);
        assert_eq!(auto_stage(Stage::NeedBootMode, p), Stage::Detect);
    }

    #[test]
    fn non_waiting_stages_ignore_hardware_changes() {
        // A menu must not be yanked away by a passing USB event.
        let p = probe(DeviceState::Bootloader, BleState::Unavailable);
        for stage in [Stage::Warning, Stage::Firmware, Stage::Flashed, Stage::Agents, Stage::Done] {
            assert_eq!(auto_stage(stage, p), stage, "{stage:?}");
        }
    }

    #[test]
    fn waiting_stages_are_exactly_the_polling_screens() {
        assert!(Stage::Detect.is_waiting());
        assert!(Stage::NeedCable.is_waiting());
        assert!(Stage::NeedBootMode.is_waiting());
        assert!(!Stage::Warning.is_waiting());
        assert!(!Stage::Firmware.is_waiting());
        assert!(!Stage::Agents.is_waiting());
    }

    #[test]
    fn the_firmware_menu_polls_but_never_moves_itself() {
        // It must refresh (device unplugged -> "Flash" greys out) without the
        // menu jumping to another screen under the user's cursor.
        assert!(Stage::Firmware.polls());
        assert!(!Stage::Firmware.is_waiting());
        for p in [
            probe(DeviceState::Absent, BleState::Absent),
            probe(DeviceState::Absent, BleState::OpenMicro),
            probe(DeviceState::Bootloader, BleState::Unavailable),
        ] {
            assert_eq!(auto_stage(Stage::Firmware, p), Stage::Firmware);
        }
        // Screens with nothing to react to do not spend a BLE scan every tick.
        assert!(!Stage::Warning.polls());
        assert!(!Stage::Agents.polls());
        assert!(!Stage::Done.polls());
    }

    /// A `Sources` with everything unavailable, for menu-availability tests.
    fn barren_sources() -> Sources {
        Sources {
            toolchain: crate::firmware::Toolchain::Missing,
            firmware_dir: None,
            existing: None,
            url: "https://example/releases".to_string(),
            forced: false,
            cached_version: None,
            have_curl: false,
        }
    }

    #[test]
    fn flash_is_unavailable_without_an_image() {
        let items = actions(
            &barren_sources(),
            probe(DeviceState::Bootloader, BleState::Unavailable),
            None,
            None,
        );
        let flash = items.iter().find(|i| i.action == Action::Flash).unwrap();
        assert!(!flash.available);
        assert!(flash.note.contains("build or download"), "{}", flash.note);
    }

    #[test]
    fn flash_is_unavailable_without_bootloader_mode() {
        let image = PathBuf::from("/tmp/fw.bin");
        let items = actions(
            &barren_sources(),
            probe(DeviceState::NormalDevice, BleState::Unavailable),
            None,
            Some(&image),
        );
        let flash = items.iter().find(|i| i.action == Action::Flash).unwrap();
        assert!(!flash.available);
        assert!(flash.note.contains("bootloader"), "{}", flash.note);
    }

    #[test]
    fn flash_is_available_with_an_image_in_bootloader_mode() {
        let image = PathBuf::from("/tmp/fw.bin");
        let items = actions(
            &barren_sources(),
            probe(DeviceState::Bootloader, BleState::Unavailable),
            None,
            Some(&image),
        );
        let flash = items.iter().find(|i| i.action == Action::Flash).unwrap();
        assert!(flash.available);
        assert!(flash.note.contains("/tmp/fw.bin"));
    }

    #[test]
    fn backup_is_offered_once_and_then_reported_as_done() {
        let boot = probe(DeviceState::Bootloader, BleState::Unavailable);
        let first = actions(&barren_sources(), boot, None, None);
        let b = first.iter().find(|i| i.action == Action::BackupStock).unwrap();
        assert!(b.available);
        assert!(b.note.contains("only way back"), "{}", b.note);

        let existing = PathBuf::from("/home/u/stock.bin");
        let second = actions(&barren_sources(), boot, Some(&existing), None);
        let b = second.iter().find(|i| i.action == Action::BackupStock).unwrap();
        assert!(!b.available, "an existing backup must not be overwritten by accident");
        assert!(b.note.contains("/home/u/stock.bin"));
    }

    #[test]
    fn build_and_download_report_their_blockers() {
        let items = actions(
            &barren_sources(),
            probe(DeviceState::Bootloader, BleState::Unavailable),
            None,
            None,
        );
        let build = items.iter().find(|i| i.action == Action::Build).unwrap();
        assert!(!build.available);
        assert!(build.note.contains("espup"), "{}", build.note);

        let dl = items.iter().find(|i| i.action == Action::Download).unwrap();
        assert!(!dl.available);
        assert!(dl.note.contains("curl"), "{}", dl.note);
    }

    #[test]
    fn download_offers_a_version_choice_when_curl_is_available() {
        let mut s = barren_sources();
        s.have_curl = true;
        let items = actions(&s, Probe::default(), None, None);
        let dl = items.iter().find(|i| i.action == Action::Download).unwrap();
        assert!(dl.available);
        assert!(dl.note.contains("published versions"), "{}", dl.note);

        // A cached download says which version is already sitting there.
        s.cached_version = Some("v1.2.0".to_string());
        let items = actions(&s, Probe::default(), None, None);
        let dl = items.iter().find(|i| i.action == Action::Download).unwrap();
        assert!(dl.note.contains("v1.2.0"), "{}", dl.note);

        // A forced URL bypasses the picker, so it names the URL instead.
        s.forced = true;
        let items = actions(&s, Probe::default(), None, None);
        let dl = items.iter().find(|i| i.action == Action::Download).unwrap();
        assert!(dl.note.contains("https://example/releases"), "{}", dl.note);
    }

    #[test]
    fn fetched_releases_open_the_picker_on_the_default_version() {
        let mut w = Wizard::new();
        let releases = crate::firmware::parse_releases(
            r#"[{"tag_name":"v2.0.0-rc1","prerelease":true,"draft":false,"published_at":"",
                 "assets":[{"name":"openmicro-fw.bin","size":1,
                            "browser_download_url":"https://x/rc"}]},
                {"tag_name":"v1.0.0","prerelease":false,"draft":false,"published_at":"",
                 "assets":[{"name":"openmicro-fw.bin","size":1,
                            "browser_download_url":"https://x/stable"}]}]"#,
        )
        .unwrap();

        w.on_releases(Ok(releases));

        assert_eq!(w.stage, Stage::Releases);
        // The cursor starts on the newest *stable* build, not simply the first
        // row, so Enter straight away installs the sensible thing.
        assert_eq!(w.selected_release().unwrap().tag, "v1.0.0");
    }

    #[test]
    fn an_empty_or_failed_release_list_does_not_open_the_picker() {
        let mut w = Wizard::new();
        w.stage = Stage::Firmware;
        w.on_releases(Ok(Vec::new()));
        assert_eq!(w.stage, Stage::Firmware);
        assert!(w.log.iter().any(|l| l.contains("no firmware releases")), "{:?}", w.log);

        w.log.clear();
        w.on_releases(Err("rate limited".to_string()));
        assert_eq!(w.stage, Stage::Firmware);
        assert!(w.log.iter().any(|l| l.contains("rate limited")), "{:?}", w.log);
    }

    #[test]
    fn the_version_picker_never_moves_itself() {
        assert!(!Stage::Releases.polls());
        assert!(!Stage::Releases.is_waiting());
        let p = probe(DeviceState::Absent, BleState::Absent);
        assert_eq!(auto_stage(Stage::Releases, p), Stage::Releases);
    }

    #[test]
    fn cursor_clamps_at_both_ends() {
        assert_eq!(move_cursor(0, 4, -1), 0);
        assert_eq!(move_cursor(3, 4, 1), 3);
        assert_eq!(move_cursor(1, 4, 1), 2);
        assert_eq!(move_cursor(0, 0, 1), 0, "empty list keeps the cursor at 0");
    }

    #[test]
    fn spinner_advances_and_wraps() {
        let mut w = Wizard::new();
        let first = w.spinner();
        w.tick();
        assert_ne!(first, w.spinner());
        for _ in 0..SPINNER.len() - 1 {
            w.tick();
        }
        assert_eq!(first, w.spinner(), "spinner wraps back around");
    }

    #[test]
    fn log_is_bounded_and_keeps_the_tail() {
        let mut w = Wizard::new();
        w.push_log((0..LOG_LIMIT + 50).map(|i| i.to_string()));
        assert_eq!(w.log.len(), LOG_LIMIT);
        assert_eq!(w.log.last().unwrap(), &(LOG_LIMIT + 49).to_string());
    }

    #[test]
    fn agents_with_a_blocked_config_cannot_be_selected() {
        // A config we must not touch (invalid JSON, a conflicting `notify`) is
        // not something the user can tick their way past.
        let mut w = Wizard::new();
        let Some(idx) = w
            .agents
            .iter()
            .position(|r| matches!(r.status, crate::agents::HookStatus::Blocked(_)))
        else {
            // No agent on this machine has a broken config: assert the rule
            // directly instead.
            let mut row = w.agents[0].clone();
            row.status = crate::agents::HookStatus::Blocked("bad json".into());
            assert!(!is_installable(&row));
            return;
        };
        w.agent_sel = idx;
        w.toggle_agent();
        assert!(!w.agents[idx].selected);
        assert!(!w.chosen_agents().contains(&w.agents[idx].kind));
    }

    #[test]
    fn probe_result_moves_the_stage_and_refreshes_the_menu() {
        let mut w = Wizard::new();
        w.stage = Stage::Detect;
        w.on_probe(probe(DeviceState::Bootloader, BleState::Unavailable));
        assert_eq!(w.stage, Stage::Firmware);
        assert!(!w.actions.is_empty());
    }

    #[test]
    fn job_labels_are_human_readable() {
        assert!(Job::Flash.label().contains("flash"));
        assert!(Job::InstallAgents(vec![AgentKind::Claude]).label().contains("1 agent"));
    }

    #[test]
    fn warning_mentions_reverting_and_that_it_is_unsupported() {
        let text = WARNING.join(" ");
        assert!(text.contains("NOT an intended use"));
        assert!(text.contains("openmicro restore"));
        assert!(text.contains("BEFORE flashing"));
    }
}
