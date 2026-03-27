pub mod connection;
pub mod local_forward;
pub mod remote_forward;

pub use connection::ConnectionForm;
pub use local_forward::LocalForwardForm;
pub use remote_forward::RemoteForwardForm;

/// A single text input field with cursor tracking.
#[derive(Debug, Clone)]
pub struct TextField {
    pub label: &'static str,
    pub value: String,
    pub cursor: usize,
}

impl TextField {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            cursor: 0,
        }
    }

    pub fn input_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.value.remove(self.cursor);
        }
    }

    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        if self.cursor < self.value.len() {
            self.cursor += 1;
        }
    }
}
