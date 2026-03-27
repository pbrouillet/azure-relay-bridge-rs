use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use azure_relay::HybridConnectionClient;
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::preamble::{self, ConnectionMode, PreambleRequest};

/// TCP socket buffer size matching C# TcpClient.SendBufferSize / ReceiveBufferSize.
const TCP_BUFFER_SIZE: usize = 65536;

/// Configure TCP socket options to match C# TcpClient defaults:
/// NoDelay = true, SendBufferSize = 65536, ReceiveBufferSize = 65536.
fn configure_tcp_socket(stream: &tokio::net::TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let sock_ref = socket2::SockRef::from(stream);
    sock_ref.set_send_buffer_size(TCP_BUFFER_SIZE)?;
    sock_ref.set_recv_buffer_size(TCP_BUFFER_SIZE)?;
    Ok(())
}

/// Local-forward bridge for TCP.
///
/// Listens on a local TCP port; for each accepted connection it opens a Hybrid
/// Connection via the relay, performs the preamble handshake (stream mode), and
/// runs a bidirectional stream pump between the local TCP socket and the relay.
pub struct TcpLocalForwardBridge {
    bind_addr: SocketAddr,
    relay_name: String,
    port_name: String,
    gateway_ports: bool,
    connect_timeout: Duration,
}

impl TcpLocalForwardBridge {
    pub fn new(
        bind_addr: SocketAddr,
        relay_name: String,
        port_name: String,
        gateway_ports: bool,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            bind_addr,
            relay_name,
            port_name,
            gateway_ports,
            connect_timeout,
        }
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn relay_name(&self) -> &str {
        &self.relay_name
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Run the local forward bridge accept loop.
    ///
    /// Binds a TCP listener and for each incoming connection:
    /// 1. Opens a relay connection via the HybridConnectionClient
    /// 2. Sends preamble (stream mode + port name)
    /// 3. Validates preamble response
    /// 4. Runs bidirectional stream pump
    pub async fn run(
        &self,
        client: Arc<HybridConnectionClient>,
        shutdown: Arc<Notify>,
    ) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        tracing::info!(addr = %self.bind_addr, relay = %self.relay_name, "TCP local forward listening");

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (tcp_stream, peer_addr) = result?;
                    configure_tcp_socket(&tcp_stream)?;
                    let client = client.clone();
                    let port_name = self.port_name.clone();
                    let connect_timeout = self.connect_timeout;

                    tokio::spawn(async move {
                        if let Err(e) = handle_local_tcp_connection(tcp_stream, client, &port_name, connect_timeout).await {
                            tracing::warn!(peer = %peer_addr, error = %e, "TCP local forward connection failed");
                        }
                    });
                }
                _ = shutdown.notified() => {
                    tracing::info!(addr = %self.bind_addr, "TCP local forward shutting down");
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Handle a single local TCP connection by bridging it to the relay.
async fn handle_local_tcp_connection(
    tcp_stream: tokio::net::TcpStream,
    client: Arc<HybridConnectionClient>,
    port_name: &str,
    connect_timeout: Duration,
) -> anyhow::Result<()> {
    configure_tcp_socket(&tcp_stream)?;
    let mut relay_stream = tokio::time::timeout(connect_timeout, client.create_connection())
        .await
        .map_err(|_| anyhow::anyhow!("relay connection timed out after {:?}", connect_timeout))??;

    let request = PreambleRequest {
        mode: ConnectionMode::Stream,
        port_name: port_name.to_string(),
    };
    preamble::write_request(&mut relay_stream, &request).await?;
    preamble::read_response(&mut relay_stream).await?;

    let (a_to_b, b_to_a) = crate::stream_pump::run(tcp_stream, relay_stream).await?;
    tracing::debug!(sent = a_to_b, received = b_to_a, "TCP local forward connection completed");

    Ok(())
}

/// Remote forwarder for TCP.
///
/// Handles incoming relay connections: reads the preamble request, validates
/// the port name and stream mode, opens a `TcpStream` to the configured target,
/// sends a success preamble response, and runs a bidirectional stream pump.
pub struct TcpRemoteForwarder {
    target_addr: SocketAddr,
    port_name: String,
    connect_timeout: Duration,
    bind_address: Option<SocketAddr>,
}

impl TcpRemoteForwarder {
    pub fn new(
        target_addr: SocketAddr,
        port_name: String,
        connect_timeout: Duration,
        bind_address: Option<SocketAddr>,
    ) -> Self {
        Self {
            target_addr,
            port_name,
            connect_timeout,
            bind_address,
        }
    }

    pub fn target_addr(&self) -> SocketAddr {
        self.target_addr
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Handle an incoming relay connection by forwarding to the target TCP endpoint.
    ///
    /// The dispatcher has already read the preamble and validated the port name
    /// and mode before calling this method.
    pub async fn handle_connection(
        &self,
        mut relay_stream: azure_relay::HybridConnectionStream,
    ) -> anyhow::Result<()> {
        let target_addr = self.target_addr;
        let bind_address = self.bind_address;
        let tcp_stream = tokio::time::timeout(self.connect_timeout, async move {
            if let Some(bind_addr) = bind_address {
                // Create socket matching the *target* address family so connect() succeeds.
                let socket = if target_addr.is_ipv4() {
                    tokio::net::TcpSocket::new_v4()?
                } else {
                    tokio::net::TcpSocket::new_v6()?
                };
                socket.bind(std::net::SocketAddr::new(bind_addr.ip(), 0))?;
                socket.connect(target_addr).await
            } else {
                tokio::net::TcpStream::connect(target_addr).await
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TCP connect to {} timed out after {:?}",
                self.target_addr,
                self.connect_timeout
            )
        })??;
        configure_tcp_socket(&tcp_stream)?;
        tracing::debug!(target = %self.target_addr, "Connected to TCP target");

        preamble::write_response_ok(&mut relay_stream, ConnectionMode::Stream).await?;

        let (a_to_b, b_to_a) = crate::stream_pump::run(tcp_stream, relay_stream).await?;
        tracing::debug!(
            target = %self.target_addr,
            sent = a_to_b,
            received = b_to_a,
            "TCP remote forward connection completed"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_local_forward_bridge_new() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let bridge = TcpLocalForwardBridge::new(addr, "relay1".into(), "8080".into(), false, Duration::from_secs(60));
        assert_eq!(bridge.bind_addr(), addr);
        assert_eq!(bridge.relay_name(), "relay1");
        assert_eq!(bridge.port_name(), "8080");
    }

    #[test]
    fn tcp_local_forward_bridge_gateway_ports() {
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        let bridge = TcpLocalForwardBridge::new(addr, "relay1".into(), "8080".into(), true, Duration::from_secs(60));
        assert!(bridge.gateway_ports);
    }

    #[test]
    fn tcp_remote_forwarder_new() {
        let addr: SocketAddr = "10.0.0.1:3306".parse().unwrap();
        let fwd = TcpRemoteForwarder::new(addr, "3306".into(), Duration::from_secs(60), None);
        assert_eq!(fwd.target_addr(), addr);
        assert_eq!(fwd.port_name(), "3306");
        assert!(fwd.bind_address.is_none());
    }

    #[test]
    fn tcp_remote_forwarder_with_bind_address() {
        let addr: SocketAddr = "10.0.0.1:3306".parse().unwrap();
        let bind: SocketAddr = "192.168.1.1:0".parse().unwrap();
        let fwd = TcpRemoteForwarder::new(addr, "3306".into(), Duration::from_secs(60), Some(bind));
        assert_eq!(fwd.target_addr(), addr);
        assert_eq!(fwd.port_name(), "3306");
        assert_eq!(fwd.bind_address, Some(bind));
    }

    #[test]
    fn tcp_remote_forwarder_bind_address_none() {
        let addr: SocketAddr = "10.0.0.1:3306".parse().unwrap();
        let fwd = TcpRemoteForwarder::new(addr, "3306".into(), Duration::from_secs(60), None);
        assert!(fwd.bind_address.is_none());
    }

    #[tokio::test]
    async fn tcp_remote_forwarder_bind_then_connect_loopback() {
        // Verify that bind-then-connect works with loopback addresses
        // by creating a TcpSocket, binding to 127.0.0.1:0, and connecting
        // to a local listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = listener.local_addr().unwrap();

        let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind(bind_addr).unwrap();
        let stream = socket.connect(target_addr).await.unwrap();

        // Verify the connection was established from the bind IP
        assert_eq!(stream.local_addr().unwrap().ip(), bind_addr.ip());

        // Accept on listener side
        let (server_stream, peer_addr) = listener.accept().await.unwrap();
        assert_eq!(peer_addr.ip(), bind_addr.ip());
        drop(stream);
        drop(server_stream);
    }

    #[tokio::test]
    async fn tcp_nodelay_is_set() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect = tokio::net::TcpStream::connect(addr);
        let accept = listener.accept();
        let (client, server) = tokio::join!(connect, accept);
        let client = client.unwrap();
        let (server, _) = server.unwrap();
        // Verify configure_tcp_socket sets nodelay and buffer sizes
        configure_tcp_socket(&client).unwrap();
        configure_tcp_socket(&server).unwrap();
        assert!(client.nodelay().unwrap());
        assert!(server.nodelay().unwrap());
        let client_ref = socket2::SockRef::from(&client);
        let server_ref = socket2::SockRef::from(&server);
        // Kernels may round buffer sizes up; verify at least the requested size
        assert!(client_ref.send_buffer_size().unwrap() >= TCP_BUFFER_SIZE);
        assert!(client_ref.recv_buffer_size().unwrap() >= TCP_BUFFER_SIZE);
        assert!(server_ref.send_buffer_size().unwrap() >= TCP_BUFFER_SIZE);
        assert!(server_ref.recv_buffer_size().unwrap() >= TCP_BUFFER_SIZE);
    }
}
