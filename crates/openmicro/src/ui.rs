use openmicro_proto::{AgentState, Rgb};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap};

use crate::client::{SnapshotDto, PALETTE};
use crate::flash::{self, ChecklistItem};

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
        let esptool = flash::esptool_path();
        let device = flash::classify_usb(&flash::detect_usb());
        self.items = flash::checklist(&image, esptool.as_deref(), device);
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
        "  [c] config  [f] flash  [q] quit"
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
    fn battery_label_formats() {
        assert_eq!(battery_label(Some(84), false), "bat 84%");
        assert_eq!(battery_label(Some(84), true), "bat 84%+");
        assert_eq!(battery_label(None, false), "bat \u{2014}");
    }
}
