#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

/// Unix socket local forward bridge.
///
/// Same pattern as [`super::tcp::TcpLocalForwardBridge`] but listens on a Unix
/// domain socket instead of a TCP port.
#[cfg(unix)]
pub struct SocketLocalForwardBridge {
    socket_path: PathBuf,
    relay_name: String,
    port_name: String,
}

#[cfg(unix)]
impl SocketLocalForwardBridge {
    pub fn new(socket_path: impl Into<PathBuf>, relay_name: String, port_name: String) -> Self {
        Self {
            socket_path: socket_path.into(),
            relay_name,
            port_name,
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn relay_name(&self) -> &str {
        &self.relay_name
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Run the local forward bridge accept loop.
    ///
    /// Binds a Unix socket listener and for each incoming connection:
    /// 1. Opens a relay connection via the HybridConnectionClient
    /// 2. Sends preamble (stream mode + port name)
    /// 3. Validates preamble response
    /// 4. Runs bidirectional stream pump
    pub async fn run(
        &self,
        client: Arc<azure_relay::HybridConnectionClient>,
        shutdown: Arc<tokio::sync::Notify>,
    ) -> anyhow::Result<()> {
        use tokio::net::UnixListener;

        // Remove stale socket file if it exists
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(path = %self.socket_path.display(), relay = %self.relay_name, "Unix socket local forward listening");

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _addr) = result?;
                    let client = client.clone();
                    let port_name = self.port_name.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_local_socket_connection(stream, client, &port_name).await {
                            tracing::warn!(error = %e, "Unix socket local forward connection failed");
                        }
                    });
                }
                _ = shutdown.notified() => {
                    tracing::info!(path = %self.socket_path.display(), "Unix socket local forward shutting down");
                    break;
                }
            }
        }
        // Clean up socket file
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }
}

#[cfg(unix)]
async fn handle_local_socket_connection(
    unix_stream: tokio::net::UnixStream,
    client: Arc<azure_relay::HybridConnectionClient>,
    port_name: &str,
) -> anyhow::Result<()> {
    let mut relay_stream = client.create_connection().await?;

    use crate::preamble::{self, ConnectionMode, PreambleRequest};
    let request = PreambleRequest {
        mode: ConnectionMode::Stream,
        port_name: port_name.to_string(),
    };
    preamble::write_request(&mut relay_stream, &request).await?;
    preamble::read_response(&mut relay_stream).await?;

    let (a_to_b, b_to_a) = crate::stream_pump::run(unix_stream, relay_stream).await?;
    tracing::debug!(sent = a_to_b, received = b_to_a, "Unix socket local forward connection completed");
    Ok(())
}

/// Unix socket remote forwarder.
///
/// Same pattern as [`super::tcp::TcpRemoteForwarder`] but connects to a Unix
/// domain socket instead of a TCP address.
#[cfg(unix)]
pub struct SocketRemoteForwarder {
    socket_path: PathBuf,
    port_name: String,
}

#[cfg(unix)]
impl SocketRemoteForwarder {
    pub fn new(socket_path: impl Into<PathBuf>, port_name: String) -> Self {
        Self {
            socket_path: socket_path.into(),
            port_name,
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Handle an incoming relay connection by forwarding to the target Unix socket.
    ///
    /// The dispatcher has already read the preamble and validated the port name
    /// and mode before calling this method.
    pub async fn handle_connection(
        &self,
        mut relay_stream: azure_relay::HybridConnectionStream,
    ) -> anyhow::Result<()> {
        let unix_stream = tokio::net::UnixStream::connect(&self.socket_path).await?;
        tracing::debug!(path = %self.socket_path.display(), "Connected to Unix socket target");

        use crate::preamble::{self, ConnectionMode};
        preamble::write_response_ok(&mut relay_stream, ConnectionMode::Stream).await?;

        let (a_to_b, b_to_a) = crate::stream_pump::run(unix_stream, relay_stream).await?;
        tracing::debug!(
            path = %self.socket_path.display(),
            sent = a_to_b, received = b_to_a,
            "Unix socket remote forward connection completed"
        );
        Ok(())
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn socket_local_forward_bridge_new() {
        let bridge =
            SocketLocalForwardBridge::new("/tmp/test.sock", "relay1".into(), "sock".into());
        assert_eq!(bridge.socket_path(), Path::new("/tmp/test.sock"));
        assert_eq!(bridge.relay_name(), "relay1");
        assert_eq!(bridge.port_name(), "sock");
    }

    #[test]
    fn socket_remote_forwarder_new() {
        let fwd = SocketRemoteForwarder::new("/var/run/app.sock", "sock".into());
        assert_eq!(fwd.socket_path(), Path::new("/var/run/app.sock"));
        assert_eq!(fwd.port_name(), "sock");
    }
}
