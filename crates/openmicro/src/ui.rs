use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::client::SnapshotDto;

pub fn status_label(connected: bool) -> &'static str {
    if connected {
        "[\u{25cf} connected]"
    } else {
        "[\u{25cb} disconnected — retrying]"
    }
}

pub fn render(frame: &mut Frame, snap: &SnapshotDto, connected: bool) {
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

    let title = format!(" OpenMicro — agents  {} ", status_label(connected));
    let table = Table::new(rows, [Constraint::Length(4), Constraint::Length(12), Constraint::Min(10)])
        .header(Row::new(vec!["slot", "agent", "state"]).style(Style::default().add_modifier(Modifier::UNDERLINED)))
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(table, frame.area());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_reflects_connection_state() {
        assert_eq!(status_label(true), "[\u{25cf} connected]");
        assert_eq!(status_label(false), "[\u{25cb} disconnected — retrying]");
    }
}
