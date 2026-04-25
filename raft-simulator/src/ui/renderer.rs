use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use raft_core::protocol::command::Command;
use raft_runtime::MetricsSnapshot;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
};

use crate::{
    cluster::Cluster,
    ui::{
        app::App,
        input::{UiAction, decode},
        widgets::{
            cluster_overview::ClusterOverview, controls::Controls, log_view::LogView,
            message_log::MessageLog,
        },
    },
};

/// Step sizes for network fault controls.
const DROP_RATE_STEP: f64 = 0.05; // 5 % per keypress
const LATENCY_STEP_MS: u64 = 10; // 10 sim-ms per keypress
const CLOCK_STEP_MS: u64 = 1000; // 10 sim-μs per manual step
const SPEED_STEP_MS: u64 = 10; // sim-μs per keypress
const FRAME_MS: u64 = 50; // ~20 fps

// ── Entry point ───────────────────────────────────────────────────────────────

/// Initialise the terminal, run the event loop, and restore the terminal on exit.
pub fn run(cluster: &mut Cluster) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::default();
    let result = event_loop(&mut terminal, cluster, &mut app);

    // Always restore the terminal, even if the loop returned an error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

// ── Event loop ────────────────────────────────────────────────────────────────

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    cluster: &mut Cluster,
    app: &mut App,
) -> io::Result<()> {
    let frame_dur = Duration::from_millis(FRAME_MS);

    loop {
        let frame_start = Instant::now();

        draw(terminal, cluster, app)?;

        let elapsed = frame_start.elapsed();
        let timeout = frame_dur.saturating_sub(elapsed);

        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(action) = decode(key.code, app)
            && apply(action, cluster, app) == LoopControl::Break
        {
            break;
        }
    }

    Ok(())
}

// ── Draw ──────────────────────────────────────────────────────────────────────

fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    cluster: &Cluster,
    app: &App,
) -> io::Result<()> {
    // Collect one consistent snapshot for the whole frame.
    let node_ids = cluster.node_ids().to_vec();

    let snapshots: Vec<(u64, Option<MetricsSnapshot>)> = node_ids
        .iter()
        .map(|&id| {
            let snap = cluster
                .nodes
                .get(&id)
                .filter(|h| h.alive)
                .map(|h| h.metrics.snapshot());
            (id, snap)
        })
        .collect();

    let selected_snap = snapshots.get(app.selected).and_then(|(_, s)| s.clone());

    let all_snaps: Vec<MetricsSnapshot> = snapshots.iter().filter_map(|(_, s)| s.clone()).collect();

    terminal.draw(|f| {
        let area = f.area();

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(area);

        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(outer[0]);

        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(main[0]);

        ClusterOverview::new(&snapshots, app.selected).render(f, left[0]);
        LogView::new(selected_snap.as_ref()).render(f, left[1]);
        MessageLog::new(&all_snaps).render(f, main[1]);
        Controls::new(
            cluster.clock.now_μs(),
            cluster.clock.is_paused(),
            cluster.clock.speed_μs(),
            cluster.drop_rate(),
            cluster.latency_ms(),
            &app.input_mode,
            &app.input_key,
            &app.input_val,
            &app.status,
        )
        .render(f, outer[1]);
    })?;

    Ok(())
}

// ── Action application ────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum LoopControl {
    Continue,
    Break,
}

/// Apply a decoded `UiAction` to `Cluster` and `App`.
/// Returns `Break` only when the user has asked to quit.
fn apply(action: UiAction, cluster: &mut Cluster, app: &mut App) -> LoopControl {
    let node_ids = cluster.node_ids().to_vec();
    let n = node_ids.len().max(1);

    match action {
        UiAction::Quit => return LoopControl::Break,

        UiAction::TogglePause => {
            if cluster.clock.is_paused() {
                cluster.clock.resume();
                app.status = "Clock resumed.".into();
            } else {
                cluster.clock.pause();
                app.status = "Clock paused.".into();
            }
        }

        UiAction::StepClock => {
            cluster.clock.tick(CLOCK_STEP_MS);
            app.status = format!(
                "Stepped to t={:2}s",
                cluster.clock.now_μs() as f64 / 1_000_000.0
            );
        }

        UiAction::KillNode => {
            if let Some(&id) = node_ids.get(app.selected) {
                cluster.kill_node(id);
                app.status = format!("Node {id} killed.");
            }
        }

        UiAction::RestartNode => {
            if let Some(&id) = node_ids.get(app.selected) {
                cluster.restart_node(id);
                app.status = format!("Node {id} restarted.");
            }
        }

        UiAction::SelectNext => app.select_next(n),
        UiAction::SelectPrev => app.select_prev(n),

        UiAction::IsolateNode => {
            if let Some(&id) = node_ids.get(app.selected) {
                cluster.isolate(id);
                app.status = format!("Node {id} isolated from all peers.");
            }
        }

        UiAction::UnisolateNode => {
            if let Some(&id) = node_ids.get(app.selected) {
                cluster.unisolate(id);
                app.status = format!("Node {id} reconnected to all peers.");
            }
        }

        UiAction::IncDropRate => {
            let new = (cluster.drop_rate() + DROP_RATE_STEP).min(1.0);
            cluster.set_drop_rate(new);
            app.status = format!("Drop rate: {:.0}%", new * 100.0);
        }

        UiAction::DecDropRate => {
            let new = (cluster.drop_rate() - DROP_RATE_STEP).max(0.0);
            cluster.set_drop_rate(new);
            app.status = format!("Drop rate: {:.0}%", new * 100.0);
        }

        UiAction::IncLatency => {
            let new = cluster.latency_ms() + LATENCY_STEP_MS;
            cluster.set_latency_ms(new);
            app.status = format!("Latency: {new}ms");
        }

        UiAction::DecLatency => {
            let new = cluster.latency_ms().saturating_sub(LATENCY_STEP_MS);
            cluster.set_latency_ms(new);
            app.status = format!("Latency: {new}ms");
        }

        UiAction::IncSpeed => {
            let new = (cluster.speed_μs() + SPEED_STEP_MS).min(10_000);
            cluster.set_speed_μs(new);
            app.status = format!("Clock speed: {new} sim-μs/tick");
        }

        UiAction::DecSpeed => {
            let new = cluster.speed_μs().saturating_sub(SPEED_STEP_MS).max(1);
            cluster.set_speed_μs(new);
            app.status = format!("Clock speed: {new} sim-μs/tick");
        }

        UiAction::Inc10Speed => {
            let new = (cluster.speed_μs() + SPEED_STEP_MS * 10).min(10_000);
            cluster.set_speed_μs(new);
            app.status = format!("Clock speed: {new} sim-μs/tick");
        }

        UiAction::Dec10Speed => {
            let new = cluster.speed_μs().saturating_sub(SPEED_STEP_MS * 10).max(1);
            cluster.set_speed_μs(new);
            app.status = format!("Clock speed: {new} sim-μs/tick");
        }

        UiAction::SubmitCommand { key, value } => {
            // Try the selected node first; fall back to any alive node.
            let target = node_ids
                .get(app.selected)
                .copied()
                .filter(|&id| cluster.nodes.get(&id).map(|h| h.alive).unwrap_or(false))
                .or_else(|| {
                    node_ids
                        .iter()
                        .find(|&&id| cluster.nodes.get(&id).map(|h| h.alive).unwrap_or(false))
                        .copied()
                });

            match target {
                Some(id) => {
                    let cmd = Command::set(key.clone(), value.clone());
                    match cluster.submit(id, cmd) {
                        Some(_) => app.status = format!("SET {key}={value} → node {id}"),
                        None => app.status = format!("Node {id} channel full."),
                    }
                }
                None => app.status = "No alive nodes to send command to.".into(),
            }
        }

        UiAction::Handled => {}
    }

    LoopControl::Continue
}
