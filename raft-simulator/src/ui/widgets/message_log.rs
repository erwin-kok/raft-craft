use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use raft_runtime::{MessageEvent, MetricsSnapshot};

pub struct MessageLog<'a> {
    snapshots: &'a [MetricsSnapshot],
}

impl<'a> MessageLog<'a> {
    pub fn new(snapshots: &'a [MetricsSnapshot]) -> Self {
        Self { snapshots }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Collect all message events from all nodes, newest first.
        let mut events: Vec<&MessageEvent> = self
            .snapshots
            .iter()
            .flat_map(|s| s.message_log.iter())
            .collect();
        events.sort_by(|a, b| b.tick.cmp(&a.tick));
        events.truncate(200);

        let items: Vec<ListItem> = events
            .iter()
            .map(|e| {
                let label_style = label_color(&e.label);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("t={:>6}s ", e.tick as f64 / 1_000_000.0),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("{}->{} ", e.from, e.to),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(e.label.clone(), label_style),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Messages  (newest first) "),
        );
        f.render_widget(list, area);
    }
}

fn label_color(label: &str) -> Style {
    if label.starts_with("RequestVote(") {
        Style::default().fg(Color::Yellow)
    } else if label.starts_with("VoteResp") {
        Style::default().fg(Color::Magenta)
    } else if label.starts_with("AppendEntries(") {
        Style::default().fg(Color::Cyan)
    } else {
        // AppendEntriesResponse
        Style::default().fg(Color::Green)
    }
}
