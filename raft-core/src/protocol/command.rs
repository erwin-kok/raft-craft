#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    value: u64,
}

impl Command {
    pub fn new(value: u64) -> Self {
        Self { value }
    }
}
