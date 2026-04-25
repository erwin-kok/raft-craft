use crossterm::event::KeyCode;

use crate::ui::app::{App, InputMode};

// ── UiAction ──────────────────────────────────────────────────────────────────

/// What the user wants to do, decoded from a keypress.
///
/// The event loop in `renderer.rs` applies these to `Cluster` and `App`.
/// Keeping decoding separate from application means neither module needs to
/// know about the other's types.
#[derive(Debug)]
pub enum UiAction {
    /// User pressed q/Q.
    Quit,
    /// Pause or resume the simulated clock.
    TogglePause,
    /// Advance the clock by one step (only meaningful when paused).
    StepClock,
    /// Kill the currently selected node.
    KillNode,
    /// Restart the currently selected node.
    RestartNode,
    /// Cycle node selection forward.
    SelectNext,
    /// Cycle node selection backward.
    SelectPrev,
    /// Isolate the selected node from all other nodes.
    IsolateNode,
    /// Restore all links to the selected node.
    UnisolateNode,
    /// Increase global packet drop rate by a fixed step.
    IncDropRate,
    /// Decrease global packet drop rate by a fixed step.
    DecDropRate,
    /// Increase global latency by a fixed step.
    IncLatency,
    /// Decrease global latency by a fixed step.
    DecLatency,
    /// Increase clock speed (more sim-ms per real tick).
    IncSpeed,
    /// Decrease clock speed (fewer sim-ms per real tick).
    DecSpeed,
    /// Increase 10x clock speed (more sim-ms per real tick).
    Inc10Speed,
    /// Decrease 10x clock speed (fewer sim-ms per real tick).
    Dec10Speed,
    /// Submit the completed SET command (key + value both filled).
    SubmitCommand { key: String, value: String },
    /// No-op — key was consumed for text input, no cluster action needed.
    Handled,
}

// ── decode ────────────────────────────────────────────────────────────────────

/// Translate a raw keypress into a `UiAction`, updating text-input state in
/// `App` as a side-effect when in input mode.
///
/// Returns `None` if the key is completely ignored.
pub fn decode(code: KeyCode, app: &mut App) -> Option<UiAction> {
    match app.input_mode {
        InputMode::EnteringKey => Some(decode_entering_key(code, app)),
        InputMode::EnteringValue => Some(decode_entering_value(code, app)),
        InputMode::Normal => decode_normal(code, app),
    }
}

fn decode_entering_key(code: KeyCode, app: &mut App) -> UiAction {
    match code {
        KeyCode::Char(c) => {
            app.input_key.push(c);
            UiAction::Handled
        }
        KeyCode::Backspace => {
            app.input_key.pop();
            UiAction::Handled
        }
        KeyCode::Enter | KeyCode::Tab => {
            if !app.input_key.is_empty() {
                app.input_mode = InputMode::EnteringValue;
            }
            UiAction::Handled
        }
        KeyCode::Esc => {
            app.cancel_input();
            UiAction::Handled
        }
        _ => UiAction::Handled,
    }
}

fn decode_entering_value(code: KeyCode, app: &mut App) -> UiAction {
    match code {
        KeyCode::Char(c) => {
            app.input_val.push(c);
            UiAction::Handled
        }
        KeyCode::Backspace => {
            app.input_val.pop();
            UiAction::Handled
        }
        KeyCode::Enter => {
            let key = app.input_key.clone();
            let val = app.input_val.clone();
            app.cancel_input();
            UiAction::SubmitCommand { key, value: val }
        }
        KeyCode::Esc => {
            app.cancel_input();
            UiAction::Handled
        }
        _ => UiAction::Handled,
    }
}

fn decode_normal(code: KeyCode, app: &mut App) -> Option<UiAction> {
    let action = match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => UiAction::Quit,
        KeyCode::Char('p') => UiAction::TogglePause,
        KeyCode::Right => UiAction::StepClock,
        KeyCode::Char('k') => UiAction::KillNode,
        KeyCode::Char('r') => UiAction::RestartNode,
        KeyCode::Char('P') => UiAction::IsolateNode,
        KeyCode::Char('H') => UiAction::UnisolateNode,
        KeyCode::Char('d') => UiAction::IncDropRate,
        KeyCode::Char('D') => UiAction::DecDropRate,
        KeyCode::Char('l') => UiAction::IncLatency,
        KeyCode::Char('L') => UiAction::DecLatency,
        KeyCode::Char('+') => UiAction::IncSpeed,
        KeyCode::Char('-') => UiAction::DecSpeed,
        KeyCode::PageUp => UiAction::Inc10Speed,
        KeyCode::PageDown => UiAction::Dec10Speed,
        KeyCode::Down => UiAction::SelectNext,
        KeyCode::Up => UiAction::SelectPrev,
        KeyCode::Char('c') => {
            app.start_command();
            UiAction::Handled
        }
        _ => return None,
    };
    Some(action)
}
