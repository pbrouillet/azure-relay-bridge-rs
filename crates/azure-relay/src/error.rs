/// Errors returned by the Azure Relay Hybrid Connections client library.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// A general relay communication error.
    #[error("Relay communication error: {message}")]
    Communication {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Authorization failed (bad SAS key, expired token, etc.).
    #[error("Authorization failed: {0}")]
    AuthorizationFailed(String),

    /// The specified Hybrid Connection endpoint does not exist.
    #[error("Endpoint not found: {0}")]
    EndpointNotFound(String),

    /// The connection string is invalid or missing required fields.
    #[error("Invalid connection string: {0}")]
    InvalidConnectionString(String),

    /// The operation timed out.
    #[error("Operation timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// Invalid argument provided.
    #[error("Invalid argument '{name}': {message}")]
    InvalidArgument { name: String, message: String },

    /// WebSocket error.
    #[error("WebSocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),

    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl RelayError {
    pub fn invalid_argument(name: impl Into<String>, message: impl Into<String>) -> Self {
        RelayError::InvalidArgument {
            name: name.into(),
            message: message.into(),
        }
    }

    pub fn communication(message: impl Into<String>) -> Self {
        RelayError::Communication {
            message: message.into(),
            source: None,
        }
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for RelayError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        RelayError::WebSocket(Box::new(err))
    }
}

/// Result type alias for relay operations.
pub type Result<T> = std::result::Result<T, RelayError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_communication_variant() {
        let err = RelayError::Communication {
            message: "connection lost".into(),
            source: None,
        };
        match &err {
            RelayError::Communication { message, source } => {
                assert_eq!(message, "connection lost");
                assert!(source.is_none());
            }
            _ => panic!("expected Communication variant"),
        }
    }

    #[test]
    fn test_authorization_failed_variant() {
        let err = RelayError::AuthorizationFailed("bad token".into());
        match &err {
            RelayError::AuthorizationFailed(msg) => assert_eq!(msg, "bad token"),
            _ => panic!("expected AuthorizationFailed variant"),
        }
    }

    #[test]
    fn test_endpoint_not_found_variant() {
        let err = RelayError::EndpointNotFound("my-hc".into());
        match &err {
            RelayError::EndpointNotFound(ep) => assert_eq!(ep, "my-hc"),
            _ => panic!("expected EndpointNotFound variant"),
        }
    }

    #[test]
    fn test_invalid_connection_string_variant() {
        let err = RelayError::InvalidConnectionString("missing endpoint".into());
        match &err {
            RelayError::InvalidConnectionString(msg) => assert_eq!(msg, "missing endpoint"),
            _ => panic!("expected InvalidConnectionString variant"),
        }
    }

    #[test]
    fn test_timeout_variant() {
        let dur = Duration::from_secs(30);
        let err = RelayError::Timeout(dur);
        match &err {
            RelayError::Timeout(d) => assert_eq!(*d, dur),
            _ => panic!("expected Timeout variant"),
        }
    }

    #[test]
    fn test_invalid_argument_variant() {
        let err = RelayError::InvalidArgument {
            name: "port".into(),
            message: "must be positive".into(),
        };
        match &err {
            RelayError::InvalidArgument { name, message } => {
                assert_eq!(name, "port");
                assert_eq!(message, "must be positive");
            }
            _ => panic!("expected InvalidArgument variant"),
        }
    }

    #[test]
    fn test_websocket_variant() {
        let ws_err = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let err = RelayError::WebSocket(Box::new(ws_err));
        assert!(matches!(&err, RelayError::WebSocket(_)));
    }

    #[test]
    fn test_io_variant() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broken");
        let err = RelayError::Io(io_err);
        assert!(matches!(&err, RelayError::Io(_)));
    }

    #[test]
    fn test_invalid_argument_helper() {
        let err = RelayError::invalid_argument("host", "cannot be empty");
        match &err {
            RelayError::InvalidArgument { name, message } => {
                assert_eq!(name, "host");
                assert_eq!(message, "cannot be empty");
            }
            _ => panic!("expected InvalidArgument variant"),
        }
    }

    #[test]
    fn test_communication_helper() {
        let err = RelayError::communication("something went wrong");
        match &err {
            RelayError::Communication { message, source } => {
                assert_eq!(message, "something went wrong");
                assert!(source.is_none());
            }
            _ => panic!("expected Communication variant"),
        }
    }

    #[test]
    fn test_display_communication() {
        let err = RelayError::communication("link down");
        assert_eq!(err.to_string(), "Relay communication error: link down");
    }

    #[test]
    fn test_display_authorization_failed() {
        let err = RelayError::AuthorizationFailed("expired".into());
        assert_eq!(err.to_string(), "Authorization failed: expired");
    }

    #[test]
    fn test_display_endpoint_not_found() {
        let err = RelayError::EndpointNotFound("hc1".into());
        assert_eq!(err.to_string(), "Endpoint not found: hc1");
    }

    #[test]
    fn test_display_invalid_connection_string() {
        let err = RelayError::InvalidConnectionString("bad".into());
        assert_eq!(err.to_string(), "Invalid connection string: bad");
    }

    #[test]
    fn test_display_timeout() {
        let err = RelayError::Timeout(Duration::from_secs(5));
        assert_eq!(err.to_string(), "Operation timed out after 5s");
    }

    #[test]
    fn test_display_invalid_argument() {
        let err = RelayError::invalid_argument("port", "negative");
        assert_eq!(err.to_string(), "Invalid argument 'port': negative");
    }

    #[test]
    fn test_display_websocket() {
        let ws_err = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let err = RelayError::WebSocket(Box::new(ws_err));
        let display = err.to_string();
        assert!(display.starts_with("WebSocket error:"), "got: {display}");
    }

    #[test]
    fn test_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = RelayError::Io(io_err);
        assert_eq!(err.to_string(), "file missing");
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out");
        let err: RelayError = io_err.into();
        assert!(matches!(err, RelayError::Io(_)));
    }

    #[test]
    fn test_from_tungstenite_error() {
        let ws_err = tokio_tungstenite::tungstenite::Error::ConnectionClosed;
        let err: RelayError = ws_err.into();
        assert!(matches!(err, RelayError::WebSocket(_)));
    }

    #[test]
    fn test_result_type_alias() {
        let ok: Result<i32> = Ok(42);
        assert_eq!(ok.unwrap(), 42);

        let fail: Result<i32> = Err(RelayError::communication("boom"));
        assert!(fail.is_err());
    }
}
