use openmicro_proto::{AgentState, Rgb};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap};

use crate::agents::HookStatus;
use crate::client::{SnapshotDto, PALETTE};
use crate::flash::{self, ChecklistItem};
use crate::onboarding::{Stage, Wizard, WARNING};
use crate::probe::{BleState, Connection, FirmwareKind, Probe};

/// Upper bound on `sleep_minutes`: 1440 minutes = 24 hours. Mirrors the same
/// clamp in `openmicrod::engine` so a stuck key (or a bogus seeded value) can
/// never drive the idle-sleep threshold arbitrarily high.
pub const MAX_SLEEP_MINUTES: u32 = 1440;

/// The four editable states, in panel row order.
pub const PANEL_STATES: [AgentState; 4] = [
    AgentState::Idle,
    AgentState::Thinking,
    AgentState::Working,
    AgentState::AwaitingApproval,
];

/// Row index of the sleep-minutes control: after brightness (0) and the four
/// per-state color rows (1..=4).
pub const SLEEP_ROW: usize = PANEL_STATES.len() + 1;

/// Local-echo state for the config panel (brightness + per-state color index
/// + idle-sleep minutes).
pub struct ConfigUiState {
    pub open: bool,
    pub selected: usize, // 0 = brightness, 1..=4 = PANEL_STATES, 5 = sleep
    pub brightness: u8,
    /// Preset index used as the starting point for further ←/→ cycling per
    /// state. Only meaningful as a *cycling cursor*; the swatch actually
    /// shown is `colors[i]` (see [`Self::seed_from_snapshot`]).
    pub color_idx: [usize; 4],
    /// Displayed per-state color (panel row order, matching `PANEL_STATES`).
    /// Seeded from the daemon's live config, so it can be the device's true
    /// current color even when that color isn't one of the `PALETTE` presets.
    pub colors: [Rgb; 4],
    pub sleep_minutes: u32,
}

impl ConfigUiState {
    pub fn new(brightness: u8) -> Self {
        Self {
            open: false,
            selected: 0,
            brightness,
            color_idx: [0; 4],
            colors: [PALETTE[0]; 4],
            sleep_minutes: 3,
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.selected < SLEEP_ROW {
            self.selected += 1;
        }
    }

    /// Seed the panel's displayed brightness/colors/sleep from the daemon's
    /// latest snapshot, replacing whatever hardcoded UI defaults (or stale
    /// prior state) were showing. Called when the panel transitions from
    /// closed to open, so the baseline for the first adjustment is the real
    /// live config rather than a UI-side constant (final-branch review Fix 1:
    /// opening `[c]` used to show the wrong values and the first bump would
    /// clobber live state with them).
    pub fn seed_from_snapshot(&mut self, snap: &SnapshotDto) {
        self.brightness = snap.brightness;
        self.sleep_minutes = snap.sleep_minutes.min(MAX_SLEEP_MINUTES);
        for (i, state) in PANEL_STATES.iter().enumerate() {
            let rgb = snap.colors.for_state(*state);
            self.colors[i] = rgb;
            // Best-effort: if the live color happens to match a palette
            // preset exactly, seed the cycling cursor there too, so the next
            // ←/→ press continues from it instead of jumping back to preset 0.
            // When it doesn't match any preset, leave the cursor as-is — the
            // swatch still shows the true live color; the first press then
            // starts cycling presets from wherever the cursor was.
            if let Some(idx) = PALETTE.iter().position(|p| *p == rgb) {
                self.color_idx[i] = idx;
            }
        }
    }
}

/// State for the guided firmware installer screen (toggled by `f`).
///
/// The checklist and `ready` flag are recomputed by [`InstallerUiState::refresh`]
/// (which does the filesystem / sysfs / PATH lookups) on open and on the `r`
/// key, so rendering stays cheap. `output` holds captured flash status lines.
#[derive(Default)]
pub struct InstallerUiState {
    pub open: bool,
    pub items: Vec<ChecklistItem>,
    pub ready: bool,
    pub output: Vec<String>,
}

impl InstallerUiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute the checklist from the live environment: firmware image,
    /// esptool discovery, and USB device state. Side-effecting (I/O), so it is
    /// called on open / refresh rather than per frame.
    pub fn refresh(&mut self) {
        let image = flash::resolve_image(None);
        let flasher = match &image {
            Ok(p) => flash::pick_flasher(p),
            // Without an image we cannot know which tool the write will need;
            // report whichever is installed rather than a misleading "missing".
            Err(_) => flash::esptool_path()
                .map(flash::Flasher::Esptool)
                .or_else(|| flash::espflash_path().map(flash::Flasher::Espflash))
                .ok_or_else(|| {
                    "no flash tool found — install one: `pip install esptool` or \
                     `cargo install espflash`."
                        .to_string()
                }),
        };
        let device = flash::classify_usb(&flash::detect_usb());
        self.items = flash::checklist(&image, &flasher, device);
        self.ready = flash::ready(&self.items);
    }
}

/// Compact battery label for the dashboard title. `None` renders as an em dash.
pub fn battery_label(pct: Option<u8>, charging: bool) -> String {
    match pct {
        Some(p) => format!("bat {}%{}", p, if charging { "+" } else { "" }),
        None => "bat \u{2014}".to_string(),
    }
}

pub fn status_label(connected: bool) -> &'static str {
    if connected {
        "[\u{25cf} connected]"
    } else {
        "[\u{25cb} disconnected — retrying]"
    }
}

pub fn render(
    frame: &mut Frame,
    snap: &SnapshotDto,
    connected: bool,
    cfg: &ConfigUiState,
    installer: &InstallerUiState,
) {
    let owner = snap.owner.clone().unwrap_or_default();
    let rows: Vec<Row> = snap
        .sessions
        .iter()
        .map(|s| {
            let id = format!("{}:{}", s.agent, s.session);
            let mut row = Row::new(vec![
                s.slot.map(|i| i.to_string()).unwrap_or_default(),
                s.agent.clone(),
                s.state.clone(),
            ]);
            if id == owner {
                row = row.style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
            }
            row
        })
        .collect();

    let hint = if cfg.open || installer.open {
        ""
    } else {
        "  [c] config  [f] flash  [s] setup  [q] quit"
    };
    let bat = battery_label(snap.battery, snap.charging);
    let title =
        format!(" OpenMicro — agents  {}  {}{} ", status_label(connected), bat, hint);
    let table = Table::new(rows, [Constraint::Length(4), Constraint::Length(12), Constraint::Min(10)])
        .header(Row::new(vec!["slot", "agent", "state"]).style(Style::default().add_modifier(Modifier::UNDERLINED)))
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, frame.area());

    if cfg.open {
        render_config(frame, cfg);
    }
    if installer.open {
        render_installer(frame, installer);
    }
}

/// Render the guided firmware installer overlay: the prerequisite checklist
/// (✓/✗ per row), a flash action when ready, any captured output, and the key
/// hints. Honest by construction: the "Press Enter to flash" action only
/// appears when all prerequisites pass; otherwise it shows what to fix.
fn render_installer(frame: &mut Frame, installer: &InstallerUiState) {
    let area = frame.area();
    let w = 72u16.min(area.width.saturating_sub(2));
    let h = 18u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "Flash firmware — Creator Micro 2 (ESP32-S3)",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(""));

    for (ok, text) in &installer.items {
        let (mark, color) = if *ok {
            ("\u{2713}", Color::Green) // ✓
        } else {
            ("\u{2717}", Color::Red) // ✗
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), Style::default().fg(color)),
            Span::raw(text.clone()),
        ]));
    }

    lines.push(Line::from(""));
    if installer.ready {
        lines.push(Line::styled(
            "\u{25b6} Press Enter to flash",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    } else {
        lines.push(Line::styled(
            "Resolve the items above (\u{2717}) before flashing.",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if !installer.output.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "output:",
            Style::default().add_modifier(Modifier::UNDERLINED),
        ));
        for l in &installer.output {
            lines.push(Line::raw(l.clone()));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" installer  [r] refresh  [Enter] flash  [f/Esc] close ");
    let para = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(Clear, rect);
    frame.render_widget(para, rect);
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------

/// Title shown for each wizard screen.
pub fn stage_title(stage: Stage) -> &'static str {
    match stage {
        Stage::Warning => "Before you start",
        Stage::Detect => "Looking for your device",
        Stage::NeedCable => "Connect a USB cable",
        Stage::NeedBootMode => "Enter bootloader mode",
        Stage::Firmware => "Firmware",
        Stage::Flashed => "Firmware written",
        Stage::Agents => "Coding agents",
        Stage::Done => "All set",
    }
}

/// Key hints shown at the bottom of each wizard screen.
pub fn stage_keys(stage: Stage, busy: bool) -> &'static str {
    if busy {
        return " running — please wait  [q] quit ";
    }
    match stage {
        Stage::Warning => " [y] I understand, continue   [s] skip setup   [q] quit ",
        Stage::Detect | Stage::NeedCable | Stage::NeedBootMode => {
            " [r] re-check now   [a] skip to agent setup   [s] skip setup   [q] quit "
        }
        Stage::Firmware => {
            " [↑↓] choose   [Enter] run   [r] re-check   [a] agent setup   [s] skip   [q] quit "
        }
        Stage::Flashed => " [Enter] continue to agent setup   [s] skip   [q] quit ",
        Stage::Agents => {
            " [↑↓] move   [space] toggle   [Enter] install selected   [n] skip   [q] quit "
        }
        Stage::Done => " [Enter] open the dashboard   [q] quit ",
    }
}

/// Human summary of what the last probe saw.
pub fn probe_summary(probe: Option<Probe>) -> String {
    let Some(p) = probe else {
        return "scanning…".to_string();
    };
    let what = match p.firmware() {
        FirmwareKind::Bootloader => "in bootloader mode, ready to flash",
        FirmwareKind::OpenMicro => "running OpenMicro firmware",
        FirmwareKind::Stock => "running the stock firmware",
        FirmwareKind::Unknown => "not found",
    };
    if !p.any_device() {
        let hint = if p.ble == BleState::Unavailable {
            " (no Bluetooth adapter available — USB only)"
        } else {
            ""
        };
        return format!("device: {what}{hint}");
    }
    let where_ = match p.connection() {
        Connection::Cable => "USB",
        Connection::Ble => "Bluetooth",
        Connection::None => "nowhere",
    };
    format!("device: seen over {where_}, {what}")
}

/// Render the full-screen setup wizard.
pub fn render_wizard(frame: &mut Frame, w: &Wizard) {
    let area = frame.area();
    let show_log = !w.log.is_empty();
    let log_height = if show_log { 9 } else { 0 };
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(log_height),
        Constraint::Length(3),
    ])
    .split(area);

    // Header: stage title + what we last saw.
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", stage_title(w.stage)),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(probe_summary(w.probe), Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" OpenMicro setup "));
    frame.render_widget(header, chunks[0]);

    // Body.
    let body = Paragraph::new(wizard_body(w))
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(body, chunks[1]);

    // Job output, most recent lines last.
    if show_log {
        let take = (log_height as usize).saturating_sub(2);
        let start = w.log.len().saturating_sub(take);
        let lines: Vec<Line> = w.log[start..].iter().map(|l| Line::raw(l.clone())).collect();
        let log =
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" output "));
        frame.render_widget(log, chunks[2]);
    }

    // Footer: spinner while busy, plus the key hints.
    let mut footer: Vec<Span> = Vec::new();
    if let Some(job) = &w.busy {
        footer.push(Span::styled(
            format!(" {} {} ", w.spinner(), job.label()),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }
    footer.push(Span::styled(
        stage_keys(w.stage, w.busy.is_some()),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(footer)).block(Block::default().borders(Borders::ALL)),
        chunks[3],
    );
}

/// The body lines for the current wizard screen.
fn wizard_body(w: &Wizard) -> Vec<Line<'static>> {
    match w.stage {
        Stage::Warning => warning_body(),
        Stage::Detect => waiting_body(
            w,
            "Looking for a Creator Micro 2 over USB and Bluetooth.",
            &[
                "Plug it in with a data-capable USB cable (some cables are charge-only),",
                "or wake it so it advertises over Bluetooth.",
            ],
        ),
        Stage::NeedCable => waiting_body(
            w,
            "Your device is reachable over Bluetooth only.",
            &[
                "Firmware can only be written over USB, so connect the device with a",
                "data-capable USB cable now.",
                "",
                "Then hold the power/boot button while plugging the cable in, and keep",
                "holding until this screen updates — that enters bootloader mode.",
            ],
        ),
        Stage::NeedBootMode => waiting_body(
            w,
            "The device is cabled, but it is running its firmware.",
            &[
                "It uses native USB-Serial-JTAG with no auto-reset, so bootloader mode",
                "has to be entered by hand:",
                "",
                "  1. Unplug the USB cable.",
                "  2. Hold the power/boot button down.",
                "  3. Plug the cable back in while still holding.",
                "  4. Release once this screen updates.",
            ],
        ),
        Stage::Firmware => firmware_body(w),
        Stage::Flashed => vec![
            Line::styled(
                "Firmware written successfully.",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw("Reset the device (unplug and replug it, without holding the button)."),
            Line::raw("It should come back up advertising as an OpenMicro device."),
            Line::raw(""),
            Line::raw("Next: set transport = \"ble\" in ~/.config/openmicro/config.toml and"),
            Line::raw("start the daemon with `openmicro service enable`."),
        ],
        Stage::Agents => agents_body(w),
        Stage::Done => vec![
            Line::styled(
                "Setup complete.",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw("Start the daemon if it is not running yet:  openmicro service enable"),
            Line::raw("Re-run this wizard at any time with:        openmicro setup"),
            Line::raw("Go back to the stock firmware (if you took a backup): openmicro restore"),
        ],
    }
}

/// The custom-firmware warning screen.
fn warning_body() -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            "⚠  Custom firmware warning",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    lines.extend(WARNING.iter().map(|l| Line::raw(l.to_string())));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Press y to accept and continue, or q to quit.",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    lines
}

/// A screen that is polling for a hardware change: headline, instructions, and
/// a live spinner so it is obvious the wizard is still watching.
fn waiting_body(w: &Wizard, headline: &str, detail: &[&str]) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(headline.to_string(), Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(""),
    ];
    lines.extend(detail.iter().map(|l| Line::raw(l.to_string())));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!("{} watching for a change…", w.spinner()),
        Style::default().fg(Color::Cyan),
    ));
    lines
}

/// The firmware action menu.
fn firmware_body(w: &Wizard) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            "The device is in bootloader mode. Choose what to do:".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    for (i, item) in w.actions.iter().enumerate() {
        let selected = i == w.action_sel;
        let marker = if selected { "> " } else { "  " };
        let style = match (selected, item.available) {
            (true, true) => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            (true, false) => Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
            (false, true) => Style::default(),
            (false, false) => Style::default().fg(Color::DarkGray),
        };
        let mark = if item.available { "•" } else { "✗" };
        lines.push(Line::styled(format!("{marker}{mark} {}", item.label), style));
        lines.push(Line::styled(
            format!("      {}", item.note),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines
}

/// The agent picker.
fn agents_body(w: &Wizard) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        "Wire your coding agents to the macropad:".to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if crate::agents::hook_binary().is_none() {
        lines.push(Line::styled(
            "⚠  openmicro-hook is not on PATH — hooks install fine but stay inert until it is.",
            Style::default().fg(Color::Red),
        ));
    }
    lines.push(Line::raw(""));

    for (i, row) in w.agents.iter().enumerate() {
        let selected = i == w.agent_sel;
        let marker = if selected { "> " } else { "  " };
        let installable = crate::onboarding::is_installable(row);
        let check = match (&row.status, row.selected) {
            (HookStatus::Installed, _) => "[✓]",
            (_, true) => "[x]",
            _ if installable => "[ ]",
            _ => "[-]",
        };
        // Keep every row to ONE line: a long reason (the unsupported/blocked
        // explanations are sentences) is shown under the highlighted row
        // instead of wrapping the list into an unreadable block.
        let (status_text, status_color) = match &row.status {
            HookStatus::Installed => ("hooks installed", Color::Green),
            HookStatus::Missing if row.present => ("detected, hooks missing", Color::Yellow),
            HookStatus::Missing => ("not installed on this machine", Color::DarkGray),
            HookStatus::Blocked(_) => ("cannot install — see below", Color::Red),
            HookStatus::Unsupported(_) => ("unsupported by this agent", Color::DarkGray),
        };
        let name_style = if selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{check} {:<14}", row.kind.label()), name_style),
            Span::styled(status_text.to_string(), Style::default().fg(status_color)),
        ]));
        // Under the highlighted row: either the exact file we would write — so
        // nobody has to guess what "install hooks" touches — or the full reason
        // this agent cannot be installed.
        if selected {
            let detail = match &row.status {
                HookStatus::Unsupported(why) => (*why).to_string(),
                HookStatus::Blocked(why) => format!("blocked: {why}"),
                _ => row
                    .config_path
                    .as_ref()
                    .map(|p| format!("writes {}", p.display()))
                    .unwrap_or_else(|| "no config file to write".to_string()),
            };
            lines.push(Line::styled(
                format!("      {detail}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Existing settings are merged, not replaced, and the previous file is backed up.",
        Style::default().fg(Color::DarkGray),
    ));
    lines
}

fn state_label(s: AgentState) -> &'static str {
    match s {
        AgentState::Idle => "idle",
        AgentState::Thinking => "thinking",
        AgentState::Working => "working",
        AgentState::AwaitingApproval => "awaiting_approval",
    }
}

fn render_config(frame: &mut Frame, cfg: &ConfigUiState) {
    let area = frame.area();
    let w = 44u16.min(area.width.saturating_sub(2));
    let h = 10u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };

    let mut lines: Vec<Line> = Vec::new();
    // Brightness row (index 0).
    let sel = |i: usize| if cfg.selected == i {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let marker = |i: usize| if cfg.selected == i { "> " } else { "  " };
    lines.push(Line::styled(
        format!("{}Brightness: {}", marker(0), cfg.brightness),
        sel(0),
    ));
    for (i, st) in PANEL_STATES.iter().enumerate() {
        let rgb: Rgb = cfg.colors[i];
        let swatch = Span::styled(
            "  ",
            Style::default().bg(Color::Rgb(rgb.r, rgb.g, rgb.b)),
        );
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}{:<18} ", marker(i + 1), state_label(*st)),
                sel(i + 1),
            ),
            swatch,
            Span::styled(format!(" {},{},{}", rgb.r, rgb.g, rgb.b), sel(i + 1)),
        ]));
    }
    // Sleep row (index SLEEP_ROW): idle minutes before LED sleep (0 disables).
    lines.push(Line::styled(
        format!("{}Sleep (min): {}", marker(SLEEP_ROW), cfg.sleep_minutes),
        sel(SLEEP_ROW),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" config  [↑↓] row  [←→] adjust  [c/Esc] close ");
    let para = Paragraph::new(lines).block(block);
    frame.render_widget(Clear, rect);
    frame.render_widget(para, rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_reflects_connection_state() {
        assert_eq!(status_label(true), "[\u{25cf} connected]");
        assert_eq!(status_label(false), "[\u{25cb} disconnected — retrying]");
    }

    #[test]
    fn seed_from_snapshot_replaces_ui_defaults() {
        // Fix 1: opening the config panel used to show hardcoded UI defaults
        // (brightness 200, sleep 3, PALETTE[0] colors) instead of the daemon's
        // real config, so the first adjustment clobbered live state. Seeding
        // from the snapshot must pull in the real values.
        let mut cfg = ConfigUiState::new(200);
        let mut snap = SnapshotDto { brightness: 77, sleep_minutes: 42, ..Default::default() };
        snap.colors.working = Rgb { r: 9, g: 8, b: 7 }; // not a PALETTE preset
        snap.colors.idle = PALETTE[2]; // matches a preset exactly

        cfg.seed_from_snapshot(&snap);

        assert_eq!(cfg.brightness, 77);
        assert_eq!(cfg.sleep_minutes, 42);
        // Working (panel row 3, PANEL_STATES[2]) shows the true live color even
        // though it doesn't match any preset.
        assert_eq!(cfg.colors[2], Rgb { r: 9, g: 8, b: 7 });
        // Idle (panel row 1, PANEL_STATES[0]) matches PALETTE[2] exactly, so the
        // preset index used for further ←/→ cycling is seeded to match.
        assert_eq!(cfg.colors[0], PALETTE[2]);
        assert_eq!(cfg.color_idx[0], 2);
    }

    #[test]
    fn seed_from_snapshot_clamps_sleep_minutes() {
        // Defense in depth alongside the TUI adjust()/engine clamp: even a
        // daemon reporting a stale/out-of-range value never shows unbounded.
        let mut cfg = ConfigUiState::new(200);
        let snap = SnapshotDto { sleep_minutes: 999_999, ..Default::default() };
        cfg.seed_from_snapshot(&snap);
        assert_eq!(cfg.sleep_minutes, 1440);
    }

    #[test]
    fn config_selection_clamps() {
        let mut c = ConfigUiState::new(200);
        assert_eq!(c.selected, 0);
        c.select_prev();
        assert_eq!(c.selected, 0); // clamped at top
        for _ in 0..10 {
            c.select_next();
        }
        assert_eq!(c.selected, SLEEP_ROW); // clamped at bottom (sleep row = 5)
        c.select_prev();
        assert_eq!(c.selected, SLEEP_ROW - 1);
    }

    #[test]
    fn installer_refresh_populates_checklist() {
        // refresh() reads the live environment; on this machine the firmware
        // image is not built and the device is not in bootloader mode, so the
        // checklist has three rows and is not ready — but never crashes.
        let mut inst = InstallerUiState::new();
        inst.refresh();
        assert_eq!(inst.items.len(), 3, "three checklist rows");
        assert!(!inst.ready, "not ready without a built image + bootloader device");
        // esptool is installed here, so row 2 (esptool) should be satisfied.
        assert!(inst.items[1].0, "esptool row should be ✓: {:?}", inst.items[1]);
        assert!(!inst.items[0].0, "firmware image row should be ✗ (not built)");
    }

    #[test]
    fn render_installer_open_does_not_panic() {
        // Confirm the installer render path compiles and runs on a test buffer.
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        let snap = SnapshotDto::default();
        let cfg = ConfigUiState::new(200);
        let mut inst = InstallerUiState::new();
        inst.open = true;
        inst.refresh();
        terminal
            .draw(|f| render(f, &snap, false, &cfg, &inst))
            .unwrap();
    }

    #[test]
    fn every_wizard_stage_renders_without_panicking() {
        // Layout arithmetic (saturating_sub on small terminals, the optional
        // log pane) is the easiest thing to get wrong here, so draw every
        // screen — with and without output — on a real test buffer.
        use ratatui::backend::TestBackend;
        for size in [(90u16, 30u16), (40, 12)] {
            for stage in ALL_STAGES {
                for with_log in [false, true] {
                    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                    let mut w = Wizard::new();
                    w.stage = stage;
                    w.probe = Some(Probe::default());
                    if with_log {
                        w.push_log((0..20).map(|i| format!("line {i}")));
                        w.busy = Some(crate::onboarding::Job::Flash);
                    }
                    terminal
                        .draw(|f| render_wizard(f, &w))
                        .unwrap_or_else(|e| panic!("{stage:?} at {size:?}: {e}"));
                }
            }
        }
    }

    /// Every wizard screen, for exhaustive rendering tests.
    const ALL_STAGES: [Stage; 8] = [
        Stage::Warning,
        Stage::Detect,
        Stage::NeedCable,
        Stage::NeedBootMode,
        Stage::Firmware,
        Stage::Flashed,
        Stage::Agents,
        Stage::Done,
    ];

    /// Render one wizard stage to an off-screen buffer and return it as text.
    fn draw_stage(stage: Stage, probe: Option<Probe>) -> String {
        use ratatui::backend::TestBackend;
        const W: usize = 100;
        let mut terminal = Terminal::new(TestBackend::new(W as u16, 34)).unwrap();
        let mut w = Wizard::new();
        w.stage = stage;
        w.probe = probe;
        // The action menu's availability depends on the probe, so it has to be
        // rebuilt after one is set (the live path does this via `on_probe`).
        w.rebuild_actions();
        terminal.draw(|f| render_wizard(f, &w)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(W)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn each_wizard_screen_says_what_the_user_must_do() {
        use crate::flash::DeviceState;

        let warning = draw_stage(Stage::Warning, None);
        assert!(warning.contains("NOT an intended use"), "{warning}");
        assert!(warning.contains("openmicro restore"), "{warning}");

        let cable = draw_stage(
            Stage::NeedCable,
            Some(Probe { usb: DeviceState::Absent, ble: BleState::StockLike }),
        );
        assert!(cable.contains("USB"), "{cable}");
        assert!(cable.to_lowercase().contains("bootloader"), "{cable}");

        let boot = draw_stage(
            Stage::NeedBootMode,
            Some(Probe { usb: DeviceState::NormalDevice, ble: BleState::Unavailable }),
        );
        assert!(boot.contains("Hold the power/boot button"), "{boot}");

        let firmware = draw_stage(
            Stage::Firmware,
            Some(Probe { usb: DeviceState::Bootloader, ble: BleState::Unavailable }),
        );
        for expected in ["Back up the stock firmware", "Build firmware", "Download", "Flash"] {
            assert!(firmware.contains(expected), "missing {expected:?}:\n{firmware}");
        }

        let agents = draw_stage(Stage::Agents, None);
        for kind in crate::agents::ALL_AGENTS {
            assert!(agents.contains(kind.label()), "missing {}:\n{agents}", kind.label());
        }
    }

    #[test]
    fn every_agent_row_stays_on_one_line() {
        // Long explanations (the unsupported/blocked reasons are sentences)
        // must not wrap the picker into an unreadable block; they belong under
        // the highlighted row instead.
        let text = draw_stage(Stage::Agents, None);
        for line in text.lines() {
            if line.contains("T3 Code") {
                assert!(line.contains("unsupported by this agent"), "{line}");
                assert!(!line.contains("drives other agents"), "long reason inline: {line}");
            }
        }
    }

    #[test]
    fn stage_titles_and_keys_are_present_for_every_stage() {
        for stage in ALL_STAGES {
            assert!(!stage_title(stage).is_empty(), "{stage:?}");
            assert!(stage_keys(stage, false).contains("[q]"), "{stage:?}");
        }
        assert!(stage_keys(Stage::Firmware, true).contains("please wait"));
    }

    #[test]
    fn probe_summary_distinguishes_transport_and_firmware() {
        use crate::flash::DeviceState;
        assert_eq!(probe_summary(None), "scanning…");

        let ble = Probe { usb: DeviceState::Absent, ble: BleState::OpenMicro };
        let s = probe_summary(Some(ble));
        assert!(s.contains("Bluetooth") && s.contains("OpenMicro"), "{s}");

        let boot = Probe { usb: DeviceState::Bootloader, ble: BleState::Unavailable };
        let s = probe_summary(Some(boot));
        assert!(s.contains("USB") && s.contains("bootloader"), "{s}");

        // No Bluetooth stack must not read as "no device seen over Bluetooth".
        let none = Probe { usb: DeviceState::Absent, ble: BleState::Unavailable };
        let s = probe_summary(Some(none));
        assert!(s.contains("no Bluetooth adapter"), "{s}");
    }

    #[test]
    fn battery_label_formats() {
        assert_eq!(battery_label(Some(84), false), "bat 84%");
        assert_eq!(battery_label(Some(84), true), "bat 84%+");
        assert_eq!(battery_label(None, false), "bat \u{2014}");
    }
}
