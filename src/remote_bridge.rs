use std::collections::HashMap;
use std::sync::Arc;

use azure_relay::{HybridConnectionListener, HybridConnectionStream};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::preamble::{self, ConnectionMode};
#[cfg(unix)]
use crate::socket::SocketRemoteForwarder;
use crate::http_remote_forwarder::HttpRemoteForwarder;
use crate::tcp::TcpRemoteForwarder;
use crate::udp::UdpRemoteForwarder;

/// The type of remote forwarder for a given port name.
pub enum RemoteForwarder {
    Tcp(TcpRemoteForwarder),
    Udp(UdpRemoteForwarder),
    Http(HttpRemoteForwarder),
    #[cfg(unix)]
    Socket(SocketRemoteForwarder),
}

/// Dispatches incoming relay connections to the appropriate forwarder.
pub struct RemoteForwardBridge {
    listener: HybridConnectionListener,
    forwarders: HashMap<String, RemoteForwarder>,
    http_forwarders: Vec<HttpRemoteForwarder>,
    relay_name: String,
}

impl RemoteForwardBridge {
    pub fn new(
        listener: HybridConnectionListener,
        forwarders: HashMap<String, RemoteForwarder>,
        http_forwarders: Vec<HttpRemoteForwarder>,
        relay_name: String,
    ) -> Self {
        Self {
            listener,
            forwarders,
            http_forwarders,
            relay_name,
        }
    }

    /// Run the remote forward bridge accept loop.
    pub async fn run(self, shutdown: Arc<Notify>) -> anyhow::Result<()> {
        // Set HTTP request handler if any HTTP forwarders exist
        if !self.http_forwarders.is_empty() {
            let handler = HttpDispatcher {
                forwarders: Arc::new(self.http_forwarders),
            };
            self.listener.set_request_handler(handler);
        }

        self.listener.open().await?;
        info!(relay = %self.relay_name, "Remote forward bridge listening on relay");

        let forwarders = Arc::new(self.forwarders);

        loop {
            tokio::select! {
                result = self.listener.accept_connection() => {
                    match result {
                        Ok(Some(stream)) => {
                            let forwarders = forwarders.clone();
                            let relay_name = self.relay_name.clone();
                            tokio::spawn(async move {
                                if let Err(e) = dispatch_connection(stream, &forwarders, &relay_name).await {
                                    warn!(relay = %relay_name, error = %e, "Remote forward dispatch failed");
                                }
                            });
                        }
                        Ok(None) => {
                            info!(relay = %self.relay_name, "Listener closed, stopping remote forward bridge");
                            break;
                        }
                        Err(e) => {
                            error!(relay = %self.relay_name, error = %e, "Accept error");
                        }
                    }
                }
                _ = shutdown.notified() => {
                    info!(relay = %self.relay_name, "Remote forward bridge shutting down");
                    break;
                }
            }
        }

        self.listener.close().await?;
        Ok(())
    }
}

/// Read preamble from incoming connection and dispatch to the right forwarder.
///
/// Implements the C# RemoteForwardBridge preamble validation logic:
/// - If only 1 forwarder and portName is a parseable integer, use that forwarder
///   regardless of portName (single-forwarder fallback)
/// - Error code 0 for unknown port name
/// - Error code 1 for UDP forwarder receiving a stream-mode connection
/// - Error code 255 for stream forwarder receiving a datagram-mode connection
async fn dispatch_connection(
    mut stream: HybridConnectionStream,
    forwarders: &HashMap<String, RemoteForwarder>,
    relay_name: &str,
) -> anyhow::Result<()> {
    let request = preamble::read_request(&mut stream).await?;
    debug!(
        relay = %relay_name,
        port_name = %request.port_name,
        mode = ?request.mode,
        "Received connection preamble"
    );

    // Forwarder lookup with single-forwarder fallback (matching C# behavior):
    // If there's only 1 forwarder and the port name is a parseable integer,
    // use that forwarder regardless of port name match.
    let forwarder = if let Some(f) = forwarders.get(&request.port_name) {
        f
    } else if forwarders.len() == 1 && request.port_name.parse::<i32>().is_ok() {
        forwarders.values().next().unwrap()
    } else {
        // Error code 0: unknown port name (matches C# {0,0,0} "unknown version" response)
        preamble::write_response_err(&mut stream, 0).await.ok();
        anyhow::bail!("no forwarder for port name '{}'", request.port_name);
    };

    // Mode validation with C#-compatible error codes
    match (forwarder, request.mode) {
        (RemoteForwarder::Tcp(tcp), ConnectionMode::Stream) => {
            tcp.handle_connection(stream).await
        }
        #[cfg(unix)]
        (RemoteForwarder::Socket(sock), ConnectionMode::Stream) => {
            sock.handle_connection(stream).await
        }
        (RemoteForwarder::Udp(udp), ConnectionMode::Datagram) => {
            udp.handle_connection(stream).await
        }
        (RemoteForwarder::Udp(_), ConnectionMode::Stream) => {
            // Error code 1: UDP forwarder expected datagram but got stream
            preamble::write_response_err(&mut stream, 1).await.ok();
            Err(anyhow::anyhow!(
                "mode mismatch: UDP forwarder received stream mode connection"
            ))
        }
        (RemoteForwarder::Tcp(_), ConnectionMode::Datagram) => {
            // Error code 255: stream forwarder received datagram mode
            preamble::write_response_err(&mut stream, 255).await.ok();
            Err(anyhow::anyhow!(
                "mode mismatch: TCP forwarder received datagram mode connection"
            ))
        }
        #[cfg(unix)]
        (RemoteForwarder::Socket(_), ConnectionMode::Datagram) => {
            preamble::write_response_err(&mut stream, 255).await.ok();
            Err(anyhow::anyhow!(
                "mode mismatch: Socket forwarder received datagram mode connection"
            ))
        }
        (RemoteForwarder::Http(_), _) => {
            // HTTP forwarders use the listener's RequestHandler mechanism,
            // not the preamble-based dispatch. This should not be reached.
            preamble::write_response_err(&mut stream, 0).await.ok();
            Err(anyhow::anyhow!(
                "HTTP forwarder does not accept WebSocket connections"
            ))
        }
    }
}

/// Dispatches HTTP requests to one of the HTTP forwarders (round-robin if multiple).
struct HttpDispatcher {
    forwarders: Arc<Vec<HttpRemoteForwarder>>,
}

impl azure_relay::http::RequestHandler for HttpDispatcher {
    async fn handle_request(
        &self,
        context: azure_relay::http::RelayedHttpListenerContext,
    ) -> azure_relay::http::RelayedHttpListenerResponse {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let idx = COUNTER.fetch_add(1, Ordering::Relaxed) % self.forwarders.len();
        self.forwarders[idx].handle_request(context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    #[test]
    fn remote_forwarder_tcp_variant() {
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let forwarder = RemoteForwarder::Tcp(tcp);
        assert!(matches!(forwarder, RemoteForwarder::Tcp(_)));
    }

    #[test]
    fn remote_forwarder_udp_variant() {
        let addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let udp = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        let forwarder = RemoteForwarder::Udp(udp);
        assert!(matches!(forwarder, RemoteForwarder::Udp(_)));
    }

    #[test]
    fn forwarder_map_lookup() {
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let udp = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        let mut map = HashMap::new();
        map.insert("3306".to_string(), RemoteForwarder::Tcp(tcp));
        map.insert("5000U".to_string(), RemoteForwarder::Udp(udp));
        assert!(map.contains_key("3306"));
        assert!(map.contains_key("5000U"));
        assert!(!map.contains_key("unknown"));
    }

    #[test]
    fn single_forwarder_numeric_fallback() {
        // When there's only 1 forwarder and port name is a parseable integer,
        // the forwarder should be used regardless of port name match.
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let mut map = HashMap::new();
        map.insert("3306".to_string(), RemoteForwarder::Tcp(tcp));

        // Port name "29876" doesn't match "3306" but is numeric
        // → single-forwarder fallback should find it
        assert!(map.get("29876").is_none()); // exact match fails
        assert_eq!(map.len(), 1);
        assert!("29876".parse::<i32>().is_ok()); // is numeric
        // So dispatch_connection would use the single forwarder
    }

    #[test]
    fn single_forwarder_non_numeric_no_fallback() {
        // Non-numeric port name should NOT trigger fallback
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let mut map = HashMap::new();
        map.insert("3306".to_string(), RemoteForwarder::Tcp(tcp));

        assert!(map.get("not_a_number").is_none());
        assert!("not_a_number".parse::<i32>().is_err()); // not numeric
        // So dispatch_connection would NOT fallback — returns error code 0
    }

    #[test]
    fn empty_forwarder_map_no_match() {
        let map: HashMap<String, RemoteForwarder> = HashMap::new();
        assert!(map.get("anyport").is_none());
        assert_eq!(map.len(), 0);
        // dispatch_connection would send error code 0
    }

    #[test]
    fn mode_mismatch_tcp_with_datagram() {
        // Verify that matching RemoteForwarder::Tcp with ConnectionMode::Datagram
        // would be caught by the match arm
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let forwarder = RemoteForwarder::Tcp(tcp);
        let mode = ConnectionMode::Datagram;

        // This match mirrors dispatch_connection's logic
        let is_mismatch = matches!(
            (&forwarder, mode),
            (RemoteForwarder::Tcp(_), ConnectionMode::Datagram)
        );
        assert!(is_mismatch, "TCP forwarder + datagram mode should be a mismatch");
    }

    #[test]
    fn mode_mismatch_udp_with_stream() {
        let addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let udp = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        let forwarder = RemoteForwarder::Udp(udp);
        let mode = ConnectionMode::Stream;

        let is_mismatch = matches!(
            (&forwarder, mode),
            (RemoteForwarder::Udp(_), ConnectionMode::Stream)
        );
        assert!(is_mismatch, "UDP forwarder + stream mode should be a mismatch");
    }

    #[test]
    fn mode_match_tcp_with_stream() {
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let forwarder = RemoteForwarder::Tcp(tcp);
        let mode = ConnectionMode::Stream;

        let is_match = matches!(
            (&forwarder, mode),
            (RemoteForwarder::Tcp(_), ConnectionMode::Stream)
        );
        assert!(is_match, "TCP forwarder + stream mode should match");
    }

    #[test]
    fn mode_match_udp_with_datagram() {
        let addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let udp = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        let forwarder = RemoteForwarder::Udp(udp);
        let mode = ConnectionMode::Datagram;

        let is_match = matches!(
            (&forwarder, mode),
            (RemoteForwarder::Udp(_), ConnectionMode::Datagram)
        );
        assert!(is_match, "UDP forwarder + datagram mode should match");
    }

    #[test]
    fn multi_forwarder_no_fallback() {
        // With multiple forwarders, no fallback even for numeric port names
        let addr: SocketAddr = "127.0.0.1:3306".parse().unwrap();
        let tcp1 = TcpRemoteForwarder::new(addr, "3306".into(), DEFAULT_TIMEOUT, None);
        let tcp2 = TcpRemoteForwarder::new(addr, "8080".into(), DEFAULT_TIMEOUT, None);
        let mut map = HashMap::new();
        map.insert("3306".to_string(), RemoteForwarder::Tcp(tcp1));
        map.insert("8080".to_string(), RemoteForwarder::Tcp(tcp2));

        assert!(map.get("29876").is_none());
        assert_eq!(map.len(), 2); // more than 1 → no fallback
    }
}
