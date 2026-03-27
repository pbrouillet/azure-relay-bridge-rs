//! Integration tests requiring a live Azure Relay namespace.
//!
//! These tests are `#[ignore]`d by default. Run them with:
//! ```sh
//! RELAY_CONNECTION_STRING="Endpoint=sb://...;..." cargo test -p azure-relay --ignored
//! ```
//!
//! Prerequisites:
//! - Set `RELAY_CONNECTION_STRING` env var with an Azure Relay namespace connection string
//! - Create two Hybrid Connections in the namespace:
//!   - `authenticated` (requires auth, the default)
//!   - `unauthenticated` (client auth disabled)

use azure_relay::{HybridConnectionClient, HybridConnectionListener, RelayConnectionStringBuilder};
use std::sync::OnceLock;

/// Returns the connection string from the RELAY_CONNECTION_STRING env var.
/// Panics if not set (tests are #[ignore]d so this only fires when explicitly run).
pub fn connection_string() -> &'static str {
    static CS: OnceLock<String> = OnceLock::new();
    CS.get_or_init(|| {
        std::env::var("RELAY_CONNECTION_STRING")
            .expect("RELAY_CONNECTION_STRING env var must be set to run integration tests")
    })
}

/// Returns a connection string with the given entity path appended.
pub fn connection_string_with_entity(entity: &str) -> String {
    let base = connection_string();
    if base.contains("EntityPath=") {
        // Replace existing EntityPath
        let mut builder = RelayConnectionStringBuilder::from_connection_string(base).unwrap();
        builder.set_entity_path(entity);
        builder.to_string()
    } else {
        format!("{};EntityPath={}", base, entity)
    }
}

/// The name of the authenticated hybrid connection.
pub const AUTHENTICATED_ENTITY: &str = "authenticated";

/// The name of the unauthenticated hybrid connection.
pub const UNAUTHENTICATED_ENTITY: &str = "unauthenticated";

pub mod runtime_tests;
pub mod websocket_tests;
pub mod http_request_tests;
