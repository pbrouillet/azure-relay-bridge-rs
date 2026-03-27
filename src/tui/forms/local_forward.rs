use super::TextField;

const FIELD_COUNT: usize = 4;

/// Form for a LocalForward (client-side) forwarding entry.
pub struct LocalForwardForm {
    pub relay_name: TextField,
    pub bind_address: TextField,
    pub bind_port: TextField,
    pub port_name: TextField,
    pub active_field: usize,
}

impl LocalForwardForm {
    pub fn new() -> Self {
        Self {
            relay_name: TextField::new("Relay Name"),
            bind_address: TextField::new("Bind Address"),
            bind_port: TextField::new("Bind Port"),
            port_name: TextField::new("Port Name"),
            active_field: 0,
        }
    }

    pub fn fields(&self) -> [&TextField; FIELD_COUNT] {
        [
            &self.relay_name,
            &self.bind_address,
            &self.bind_port,
            &self.port_name,
        ]
    }

    pub fn fields_mut(&mut self) -> [&mut TextField; FIELD_COUNT] {
        [
            &mut self.relay_name,
            &mut self.bind_address,
            &mut self.bind_port,
            &mut self.port_name,
        ]
    }

    fn active_field_mut(&mut self) -> &mut TextField {
        let idx = self.active_field;
        self.fields_mut()[idx]
    }

    pub fn next_field(&mut self) {
        self.active_field = (self.active_field + 1) % FIELD_COUNT;
    }

    pub fn prev_field(&mut self) {
        self.active_field = if self.active_field == 0 {
            FIELD_COUNT - 1
        } else {
            self.active_field - 1
        };
    }

    pub fn input_char(&mut self, c: char) {
        self.active_field_mut().input_char(c);
    }

    pub fn backspace(&mut self) {
        self.active_field_mut().backspace();
    }

    pub fn cursor_left(&mut self) {
        self.active_field_mut().cursor_left();
    }

    pub fn cursor_right(&mut self) {
        self.active_field_mut().cursor_right();
    }

    pub fn summary(&self) -> String {
        let port = if self.bind_port.value.is_empty() {
            "?".to_string()
        } else {
            self.bind_port.value.clone()
        };
        let relay = if self.relay_name.value.is_empty() {
            "<unnamed>"
        } else {
            &self.relay_name.value
        };
        format!("{}:{} → {}", self.bind_address.value.as_str(), port, relay)
    }
}
