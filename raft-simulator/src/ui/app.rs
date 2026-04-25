/// All mutable UI state.  No I/O, no cluster interaction — purely data.
pub struct App {
    /// Index into the sorted node-id list; used by overview and log view.
    pub selected: usize,
    pub input_mode: InputMode,
    /// Key being typed for a SET command.
    pub input_key: String,
    /// Value being typed for a SET command.
    pub input_val: String,
    /// One-line status shown in the controls bar.
    pub status: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            selected: 0,
            input_mode: InputMode::Normal,
            input_key: String::new(),
            input_val: String::new(),
            status: "Ready — press ? for help".into(),
        }
    }
}

impl App {
    /// Advance to the next node in the cluster list (wraps around).
    pub fn select_next(&mut self, total: usize) {
        if total > 0 {
            self.selected = (self.selected + 1) % total;
        }
    }

    /// Go back to the previous node (wraps around).
    pub fn select_prev(&mut self, total: usize) {
        if total > 0 {
            self.selected = (self.selected + total - 1) % total;
        }
    }

    /// Begin key-entry mode for a new SET command.
    pub fn start_command(&mut self) {
        self.input_mode = InputMode::EnteringKey;
        self.input_key.clear();
        self.input_val.clear();
    }

    /// Cancel any in-progress command input.
    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::Normal;
        self.input_key.clear();
        self.input_val.clear();
    }
}

// ── InputMode ─────────────────────────────────────────────────────────────────

/// Which field the user is currently typing into.
#[derive(Debug, PartialEq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    /// User is typing the key for a SET command.
    EnteringKey,
    /// Key confirmed; user is typing the value.
    EnteringValue,
}
