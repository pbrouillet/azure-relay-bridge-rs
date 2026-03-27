#[cfg(feature = "azure-identity")]
pub mod aad_token_provider;
pub mod client;
pub mod connection_string;
pub mod error;
#[allow(dead_code)] // pub(crate) methods used when listener HTTP handling is wired up
pub mod http;
pub mod listener;
pub mod protocol;
pub mod stream;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod token_provider;

#[cfg(feature = "azure-identity")]
pub use aad_token_provider::{AadTokenProvider, TokenKind};
pub use client::HybridConnectionClient;
pub use connection_string::{AuthenticationType, RelayConnectionStringBuilder};
pub use error::{RelayError, Result};
pub use http::{
    RelayedHttpListenerContext, RelayedHttpListenerRequest, RelayedHttpListenerResponse,
    RequestHandler,
};
pub use listener::{ConnectionStatus, HybridConnectionListener};
pub use stream::{HybridConnectionStream, WriteMode};
