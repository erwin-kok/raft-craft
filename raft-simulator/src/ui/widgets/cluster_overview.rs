use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table},
};

use raft_core::{Role, protocol::types::NodeId};
use raft_runtime::MetricsSnapshot;

pub struct ClusterOverview<'a> {
    snapshots: &'a [(NodeId, Option<MetricsSnapshot>)],
    selected: usize,
}

impl<'a> ClusterOverview<'a> {
    /// `snapshots` is a slice of `(node_id, Some(snapshot))` for live nodes
    /// and `(node_id, None)` for dead ones.
    pub fn new(snapshots: &'a [(NodeId, Option<MetricsSnapshot>)], selected: usize) -> Self {
        Self {
            snapshots,
            selected,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let header = Row::new(
            ["Node", "Status", "Role", "Term", "Commit", "Applied", "Log"]
                .iter()
                .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD))),
        )
        .height(1);

        let widths = [
            ratatui::layout::Constraint::Length(5),
            ratatui::layout::Constraint::Length(7),
            ratatui::layout::Constraint::Length(10),
            ratatui::layout::Constraint::Length(5),
            ratatui::layout::Constraint::Length(7),
            ratatui::layout::Constraint::Length(8),
            ratatui::layout::Constraint::Length(4),
        ];

        let rows: Vec<Row> = self
            .snapshots
            .iter()
            .enumerate()
            .map(|(i, (id, snap))| {
                let selected_style = if i == self.selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                match snap {
                    None => Row::new(vec![
                        Cell::from(format!("{id}")),
                        Cell::from("DEAD").style(Style::default().fg(Color::Red)),
                        Cell::from("—"),
                        Cell::from("—"),
                        Cell::from("—"),
                        Cell::from("—"),
                        Cell::from("—"),
                    ])
                    .style(selected_style),

                    Some(s) => {
                        let (role_str, role_style) = match s.role {
                            Role::Leader => (
                                "Leader",
                                Style::default()
                                    .fg(Color::Green)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Role::Candidate => ("Candidate", Style::default().fg(Color::Yellow)),
                            Role::Follower => ("Follower", Style::default().fg(Color::Cyan)),
                        };
                        Row::new(vec![
                            Cell::from(format!("{id}")),
                            Cell::from("alive").style(Style::default().fg(Color::Green)),
                            Cell::from(role_str).style(role_style),
                            Cell::from(format!("{}", s.term)),
                            Cell::from(format!("{}", s.commit_index)),
                            Cell::from(format!("{}", s.last_applied)),
                            Cell::from(format!("{}", s.log_length)),
                        ])
                        .style(selected_style)
                    }
                }
            })
            .collect();

        let table = Table::new(rows, widths).header(header).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Cluster  [↑ / ↓ = cycle node] "),
        );

        f.render_widget(table, area);
    }
}
