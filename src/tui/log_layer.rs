use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Shared sender handle. The runner sets/clears this when starting/stopping.
pub type SharedLogSender = Arc<Mutex<Option<mpsc::UnboundedSender<String>>>>;

/// A tracing `Layer` that formats events and sends them through a shared
/// mpsc channel for display in the TUI log panel.
pub struct ChannelLayer {
    sender: SharedLogSender,
}

impl ChannelLayer {
    pub fn new(sender: SharedLogSender) -> Self {
        Self { sender }
    }
}

impl<S: Subscriber> Layer<S> for ChannelLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let guard = self.sender.lock().unwrap();
        if let Some(ref tx) = *guard {
            let metadata = event.metadata();
            let level = metadata.level();
            let target = metadata.target();

            let mut visitor = MessageVisitor::default();
            event.record(&mut visitor);

            let msg = if visitor.message.is_empty() {
                format!("{level} {target}")
            } else {
                format!("{level} {target}: {}", visitor.message)
            };

            let _ = tx.send(msg);
        }
    }
}

/// Visitor that extracts the `message` field from a tracing event.
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else if self.message.is_empty() {
            // Accumulate other fields as key=value
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message
                .push_str(&format!("{}={:?}", field.name(), value));
        } else {
            self.message
                .push_str(&format!(" {}={:?}", field.name(), value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message
                .push_str(&format!("{}={}", field.name(), value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !self.message.is_empty() {
            self.message.push(' ');
        }
        self.message
            .push_str(&format!("{}={}", field.name(), value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !self.message.is_empty() {
            self.message.push(' ');
        }
        self.message
            .push_str(&format!("{}={}", field.name(), value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !self.message.is_empty() {
            self.message.push(' ');
        }
        self.message
            .push_str(&format!("{}={}", field.name(), value));
    }
}
