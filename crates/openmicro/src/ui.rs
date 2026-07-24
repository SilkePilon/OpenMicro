use openmicro_proto::{AgentState, Rgb};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table};

use crate::client::{SnapshotDto, PALETTE};

/// The four editable states, in panel row order.
pub const PANEL_STATES: [AgentState; 4] = [
    AgentState::Idle,
    AgentState::Thinking,
    AgentState::Working,
    AgentState::AwaitingApproval,
];

/// Local-echo state for the config panel (brightness + per-state color index).
pub struct ConfigUiState {
    pub open: bool,
    pub selected: usize, // 0 = brightness, 1..=4 = PANEL_STATES
    pub brightness: u8,
    pub color_idx: [usize; 4],
}

impl ConfigUiState {
    pub fn new(brightness: u8) -> Self {
        Self { open: false, selected: 0, brightness, color_idx: [0; 4] }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self) {
        if self.selected < PANEL_STATES.len() {
            self.selected += 1;
        }
    }
}

pub fn status_label(connected: bool) -> &'static str {
    if connected {
        "[\u{25cf} connected]"
    } else {
        "[\u{25cb} disconnected — retrying]"
    }
}

pub fn render(frame: &mut Frame, snap: &SnapshotDto, connected: bool, cfg: &ConfigUiState) {
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

    let hint = if cfg.open { "" } else { "  [c] config  [q] quit" };
    let title = format!(" OpenMicro — agents  {}{} ", status_label(connected), hint);
    let table = Table::new(rows, [Constraint::Length(4), Constraint::Length(12), Constraint::Min(10)])
        .header(Row::new(vec!["slot", "agent", "state"]).style(Style::default().add_modifier(Modifier::UNDERLINED)))
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, frame.area());

    if cfg.open {
        render_config(frame, cfg);
    }
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
    let h = 9u16.min(area.height.saturating_sub(2));
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
        let rgb: Rgb = PALETTE[cfg.color_idx[i]];
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
    fn config_selection_clamps() {
        let mut c = ConfigUiState::new(200);
        assert_eq!(c.selected, 0);
        c.select_prev();
        assert_eq!(c.selected, 0); // clamped at top
        for _ in 0..10 {
            c.select_next();
        }
        assert_eq!(c.selected, PANEL_STATES.len()); // clamped at bottom (4)
        c.select_prev();
        assert_eq!(c.selected, PANEL_STATES.len() - 1);
    }
}
