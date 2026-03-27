use super::TextField;

const FIELD_COUNT: usize = 5;

/// Form for Azure Relay connection settings (shared between client and server).
pub struct ConnectionForm {
    pub endpoint: TextField,
    pub connection_string: TextField,
    pub sas_key_name: TextField,
    pub sas_key: TextField,
    pub log_level: TextField,
    pub active_field: usize,
}

impl ConnectionForm {
    pub fn new() -> Self {
        Self {
            endpoint: TextField::new("Endpoint URI"),
            connection_string: TextField::new("Connection String"),
            sas_key_name: TextField::new("SAS Key Name"),
            sas_key: TextField::new("SAS Key"),
            log_level: TextField::new("Log Level"),
            active_field: 0,
        }
    }

    pub fn fields(&self) -> [&TextField; FIELD_COUNT] {
        [
            &self.endpoint,
            &self.connection_string,
            &self.sas_key_name,
            &self.sas_key,
            &self.log_level,
        ]
    }

    pub fn fields_mut(&mut self) -> [&mut TextField; FIELD_COUNT] {
        [
            &mut self.endpoint,
            &mut self.connection_string,
            &mut self.sas_key_name,
            &mut self.sas_key,
            &mut self.log_level,
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
}
