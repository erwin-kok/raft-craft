use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::ui::app::InputMode;

pub struct Controls<'a> {
    clock_μs: u64,
    paused: bool,
    speed_μs: u64,
    drop_rate: f64,
    latency_ms: u64,
    input_mode: &'a InputMode,
    input_key: &'a str,
    input_val: &'a str,
    status: &'a str,
}

impl<'a> Controls<'a> {
    pub fn new(
        clock_μs: u64,
        paused: bool,
        speed_μs: u64,
        drop_rate: f64,
        latency_ms: u64,
        input_mode: &'a InputMode,
        input_key: &'a str,
        input_val: &'a str,
        status: &'a str,
    ) -> Self {
        Self {
            clock_μs,
            paused,
            speed_μs,
            drop_rate,
            latency_ms,
            input_mode,
            input_key,
            input_val,
            status,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let line = match self.input_mode {
            InputMode::EnteringKey => Line::from(vec![
                Span::raw("  SET key: "),
                Span::styled(
                    format!("{}█", self.input_key),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    "  (Enter/Tab = confirm key   Esc = cancel)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),

            InputMode::EnteringValue => Line::from(vec![
                Span::raw(format!("  SET {}=", self.input_key)),
                Span::styled(
                    format!("{}█", self.input_val),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    "  (Enter = submit   Esc = cancel)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),

            InputMode::Normal => {
                let (clock_label, clock_color) = if self.paused {
                    ("PAUSED", Color::Yellow)
                } else {
                    ("RUNNING", Color::Green)
                };

                // Network fault summary shown in red when non-zero.
                let fault_str = match (self.drop_rate > 0.0, self.latency_ms > 0) {
                    (true, true) => format!(
                        "  drop={:.0}%  lat={}ms",
                        self.drop_rate * 100.0,
                        self.latency_ms
                    ),
                    (true, false) => format!("  drop={:.0}%", self.drop_rate * 100.0),
                    (false, true) => format!("  lat={}ms", self.latency_ms),
                    (false, false) => String::new(),
                };

                Line::from(vec![
                    Span::styled(
                        format!(
                            "  t={:.2}s step={:.2}μs [{}]",
                            self.clock_μs as f64 / 1_000_000.0,
                            self.speed_μs as f64 / 1_000.0,
                            clock_label
                        ),
                        Style::default()
                            .fg(clock_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(fault_str, Style::default().fg(Color::Red)),
                    Span::raw("  "),
                    Span::styled(self.status, Style::default().fg(Color::LightBlue)),
                    Span::styled(
                        // Must stay in sync with the match arms in input.rs.
                        "  | p=pause  \u{2192}=step  k=kill  r=restart  c=command  \
                         P=partition  H=heal  d/D=drop\u{00b1}  l/L=latency\u{00b1}  ↑/↓=cycle  ?=help  q=quit",
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            }
        };

        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::TOP)),
            area,
        );
    }
}
