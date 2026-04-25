use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use raft_runtime::MetricsSnapshot;

pub struct LogView<'a> {
    snap: Option<&'a MetricsSnapshot>,
}

impl<'a> LogView<'a> {
    pub fn new(snap: Option<&'a MetricsSnapshot>) -> Self {
        Self { snap }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let Some(snap) = self.snap else {
            let list = List::new(vec![ListItem::new("  (node dead)")])
                .block(Block::default().borders(Borders::ALL).title(" Log "));
            f.render_widget(list, area);
            return;
        };

        let commit = snap.commit_index;
        let applied = snap.last_applied;
        let length = snap.log_length;
        let id = snap.node_id;

        let mut items: Vec<ListItem> = Vec::new();

        if length == 0 {
            items.push(ListItem::new(Line::from(Span::styled(
                "  (empty log)",
                Style::default().fg(Color::DarkGray),
            ))));
        } else {
            for idx in 1..=length {
                let is_applied = idx <= applied;
                let is_committed = idx <= commit;

                let (marker, color) = match (is_committed, is_applied) {
                    (true, true) => ("✓", Color::Green),
                    (true, false) => ("○", Color::Yellow),
                    _ => ("·", Color::DarkGray),
                };

                items.push(ListItem::new(Line::from(vec![Span::styled(
                    format!("  [{marker}] {idx:>4}"),
                    Style::default().fg(color),
                )])));
            }
        }

        let title = format!(" Log — node {id}  commit={commit}  applied={applied} ");
        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(list, area);
    }
}
