use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Duration, Instant};

use azure_relay::HybridConnectionClient;

/// Maximum UDP datagram size.
const MAX_DATAGRAM_SIZE: usize = 65535;

/// Idle timeout for UDP routes (4 minutes, matching C#).
const UDP_ROUTE_IDLE_TIMEOUT: Duration = Duration::from_secs(240);

/// Send timeout for UDP route writes (matching C# CancellationTokenSource(TimeSpan.FromSeconds(1))).
const UDP_SEND_TIMEOUT: Duration = Duration::from_secs(1);

/// Write a length-prefixed datagram to a stream.
pub async fn write_datagram<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    data: &[u8],
) -> std::io::Result<()> {
    if data.len() > MAX_DATAGRAM_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("datagram too large: {} bytes", data.len()),
        ));
    }
    let len = (data.len() as u16).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed datagram from a stream.
/// Returns None on EOF.
pub async fn read_datagram<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 2];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data).await?;
    Ok(Some(data))
}

/// A route for a specific UDP client, with its own relay connection.
struct UdpRoute {
    /// Sender half for writing datagrams to the relay.
    relay_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    /// Last activity timestamp for idle timeout.
    last_activity: Arc<Mutex<Instant>>,
}

/// UDP local forward bridge.
/// Binds a local UDP port, maintains a route table mapping client endpoints
/// to relay connections, and forwards datagrams using length-prefix framing.
pub struct UdpLocalForwardBridge {
    bind_addr: SocketAddr,
    relay_name: String,
    port_name: String,
}

impl UdpLocalForwardBridge {
    pub fn new(bind_addr: SocketAddr, relay_name: String, port_name: String) -> Self {
        Self {
            bind_addr,
            relay_name,
            port_name,
        }
    }

    /// Run the UDP local forward bridge.
    ///
    /// Binds a UDP socket and maintains a route table mapping client endpoints
    /// to relay connections. Each unique client gets its own relay connection
    /// with datagram framing.
    pub async fn run(
        &self,
        client: Arc<HybridConnectionClient>,
        shutdown: Arc<Notify>,
    ) -> anyhow::Result<()> {
        let socket = Arc::new(UdpSocket::bind(self.bind_addr).await?);
        tracing::info!(addr = %self.bind_addr, relay = %self.relay_name, "UDP local forward listening");

        let routes: Arc<Mutex<HashMap<SocketAddr, UdpRoute>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let mut buf = vec![0u8; MAX_DATAGRAM_SIZE];

        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, peer_addr) = result?;
                    let data = buf[..len].to_vec();

                    let mut routes_lock = routes.lock().await;

                    // Check if route exists and is still active
                    let route_active = routes_lock.get(&peer_addr)
                        .map(|r| !r.relay_tx.is_closed())
                        .unwrap_or(false);

                    if !route_active {
                        // Remove stale route if present
                        routes_lock.remove(&peer_addr);

                        // Create new route
                        match create_udp_route(
                            client.clone(),
                            &self.port_name,
                            socket.clone(),
                            peer_addr,
                        ).await {
                            Ok(route) => {
                                routes_lock.insert(peer_addr, route);
                            }
                            Err(e) => {
                                tracing::warn!(peer = %peer_addr, error = %e, "Failed to create UDP route");
                                continue;
                            }
                        }
                    }

                    // Send datagram through the route
                    if let Some(route) = routes_lock.get(&peer_addr) {
                        *route.last_activity.lock().await = Instant::now();
                        if route.relay_tx.send(data).await.is_err() {
                            tracing::debug!(peer = %peer_addr, "UDP route channel closed");
                            routes_lock.remove(&peer_addr);
                        }
                    }
                }
                _ = shutdown.notified() => {
                    tracing::info!(addr = %self.bind_addr, "UDP local forward shutting down");
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Create a new UDP route: open a relay connection, send preamble, spawn
/// send/receive tasks with datagram framing.
async fn create_udp_route(
    client: Arc<HybridConnectionClient>,
    port_name: &str,
    socket: Arc<UdpSocket>,
    peer_addr: SocketAddr,
) -> anyhow::Result<UdpRoute> {
    use crate::preamble::{self, ConnectionMode, PreambleRequest};

    // Open relay connection
    let mut relay_stream = client.create_connection().await?;

    // Send preamble (datagram mode)
    let request = PreambleRequest {
        mode: ConnectionMode::Datagram,
        port_name: port_name.to_string(),
    };
    preamble::write_request(&mut relay_stream, &request).await?;
    preamble::read_response(&mut relay_stream).await?;

    // Split into read/write halves
    let (relay_read, relay_write) = tokio::io::split(relay_stream);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let last_activity = Arc::new(Mutex::new(Instant::now()));

    // Spawn sender: channel → relay (with datagram framing)
    let last_act_send = last_activity.clone();
    tokio::spawn(async move {
        let mut writer = relay_write;
        while let Some(data) = rx.recv().await {
            *last_act_send.lock().await = Instant::now();
            match tokio::time::timeout(
                UDP_SEND_TIMEOUT,
                write_datagram(&mut writer, &data),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "UDP route send failed");
                    break;
                }
                Err(_) => {
                    tracing::debug!("UDP route send timed out (1s)");
                    break;
                }
            }
        }
    });

    // Spawn receiver: relay → UDP socket (with datagram framing)
    let last_act_recv = last_activity.clone();
    tokio::spawn(async move {
        let mut reader = relay_read;
        loop {
            match tokio::time::timeout(UDP_ROUTE_IDLE_TIMEOUT, read_datagram(&mut reader)).await {
                Ok(Ok(Some(data))) => {
                    *last_act_recv.lock().await = Instant::now();
                    if let Err(e) = socket.send_to(&data, peer_addr).await {
                        tracing::debug!(error = %e, "UDP route recv send_to failed");
                        break;
                    }
                }
                Ok(Ok(None)) => break, // EOF
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "UDP route read failed");
                    break;
                }
                Err(_) => {
                    tracing::debug!(peer = %peer_addr, "UDP route idle timeout");
                    break;
                }
            }
        }
    });

    Ok(UdpRoute {
        relay_tx: tx,
        last_activity,
    })
}

/// UDP remote forwarder.
/// Accepts relay connections with datagram mode, forwards to target UDP endpoint.
pub struct UdpRemoteForwarder {
    target_addr: SocketAddr,
    port_name: String,
    bind_address: Option<SocketAddr>,
}

impl UdpRemoteForwarder {
    pub fn new(target_addr: SocketAddr, port_name: String, bind_address: Option<SocketAddr>) -> Self {
        Self {
            target_addr,
            port_name,
            bind_address,
        }
    }

    /// Handle an incoming relay connection in datagram mode.
    pub async fn handle_connection(
        &self,
        mut relay_stream: azure_relay::HybridConnectionStream,
    ) -> anyhow::Result<()> {
        use crate::preamble::{self, ConnectionMode};

        // Send success preamble response
        preamble::write_response_ok(&mut relay_stream, ConnectionMode::Datagram).await?;

        // Bind an ephemeral UDP socket for forwarding
        let bind_addr = self.bind_address
            .map(|a| std::net::SocketAddr::new(a.ip(), 0))
            .unwrap_or_else(|| {
                // Match the target's address family so connect() succeeds.
                if self.target_addr.is_ipv4() {
                    "0.0.0.0:0".parse().unwrap()
                } else {
                    "[::]:0".parse().unwrap()
                }
            });
        let socket = UdpSocket::bind(bind_addr).await?;
        socket.connect(self.target_addr).await?;
        tracing::debug!(target = %self.target_addr, "UDP remote forwarder connected");

        let (relay_read, relay_write) = tokio::io::split(relay_stream);
        let socket = Arc::new(socket);

        // Spawn relay → UDP task
        let socket_send = socket.clone();
        let send_handle = tokio::spawn(async move {
            let mut reader = relay_read;
            loop {
                match tokio::time::timeout(UDP_ROUTE_IDLE_TIMEOUT, read_datagram(&mut reader)).await
                {
                    Ok(Ok(Some(data))) => {
                        if let Err(e) = socket_send.send(&data).await {
                            tracing::debug!(error = %e, "UDP remote send failed");
                            break;
                        }
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "UDP remote read failed");
                        break;
                    }
                    Err(_) => {
                        tracing::debug!("UDP remote forwarder idle timeout");
                        break;
                    }
                }
            }
        });

        // Spawn UDP → relay task
        let socket_recv = socket.clone();
        let recv_handle = tokio::spawn(async move {
            let mut writer = relay_write;
            let mut buf = vec![0u8; MAX_DATAGRAM_SIZE];
            loop {
                match tokio::time::timeout(UDP_ROUTE_IDLE_TIMEOUT, socket_recv.recv(&mut buf)).await
                {
                    Ok(Ok(n)) => {
                        if let Err(e) = write_datagram(&mut writer, &buf[..n]).await {
                            tracing::debug!(error = %e, "UDP remote relay write failed");
                            break;
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "UDP remote recv failed");
                        break;
                    }
                    Err(_) => {
                        tracing::debug!("UDP remote forwarder idle timeout");
                        break;
                    }
                }
            }
        });

        // Wait for either task to finish
        tokio::select! {
            _ = send_handle => {}
            _ = recv_handle => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn write_read_datagram_round_trip() {
        let data = b"Hello UDP!";
        let mut buf = Vec::new();
        write_datagram(&mut buf, data).await.unwrap();

        // Verify format: 2-byte BE length + payload
        assert_eq!(buf.len(), 2 + data.len());
        assert_eq!(&buf[..2], &(data.len() as u16).to_be_bytes());
        assert_eq!(&buf[2..], data);

        let mut cursor = Cursor::new(&buf);
        let read = read_datagram(&mut cursor).await.unwrap().unwrap();
        assert_eq!(read, data);
    }

    #[tokio::test]
    async fn write_read_empty_datagram() {
        let mut buf = Vec::new();
        write_datagram(&mut buf, b"").await.unwrap();
        assert_eq!(&buf, &[0, 0]);

        let mut cursor = Cursor::new(&buf);
        let read = read_datagram(&mut cursor).await.unwrap().unwrap();
        assert!(read.is_empty());
    }

    #[tokio::test]
    async fn write_read_max_size_datagram() {
        let data = vec![0xAB; 65535];
        let mut buf = Vec::new();
        write_datagram(&mut buf, &data).await.unwrap();
        assert_eq!(buf.len(), 2 + 65535);
        assert_eq!(&buf[..2], &[0xFF, 0xFF]); // 65535 in BE

        let mut cursor = Cursor::new(&buf);
        let read = read_datagram(&mut cursor).await.unwrap().unwrap();
        assert_eq!(read, data);
    }

    #[tokio::test]
    async fn write_datagram_too_large_fails() {
        let data = vec![0; 65536];
        let mut buf = Vec::new();
        let result = write_datagram(&mut buf, &data).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_datagram_eof_returns_none() {
        let buf: &[u8] = &[];
        let mut cursor = Cursor::new(buf);
        let result = read_datagram(&mut cursor).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_multiple_datagrams() {
        let mut buf = Vec::new();
        write_datagram(&mut buf, b"first").await.unwrap();
        write_datagram(&mut buf, b"second").await.unwrap();
        write_datagram(&mut buf, b"third").await.unwrap();

        let mut cursor = Cursor::new(&buf);
        assert_eq!(
            read_datagram(&mut cursor).await.unwrap().unwrap(),
            b"first"
        );
        assert_eq!(
            read_datagram(&mut cursor).await.unwrap().unwrap(),
            b"second"
        );
        assert_eq!(
            read_datagram(&mut cursor).await.unwrap().unwrap(),
            b"third"
        );
        assert!(read_datagram(&mut cursor).await.unwrap().is_none());
    }

    // C# compatibility: verify known byte sequences
    #[tokio::test]
    async fn csharp_compat_datagram_framing() {
        // C# writes a 5-byte datagram "Hello": [0x00, 0x05, 0x48, 0x65, 0x6C, 0x6C, 0x6F]
        let csharp_bytes: &[u8] = &[0x00, 0x05, 0x48, 0x65, 0x6C, 0x6C, 0x6F];
        let mut cursor = Cursor::new(csharp_bytes);
        let data = read_datagram(&mut cursor).await.unwrap().unwrap();
        assert_eq!(data, b"Hello");
    }

    #[test]
    fn udp_route_idle_timeout_is_4_min() {
        assert_eq!(UDP_ROUTE_IDLE_TIMEOUT, Duration::from_secs(240));
    }

    #[test]
    fn udp_local_forward_bridge_new() {
        let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let bridge = UdpLocalForwardBridge::new(addr, "relay1".into(), "9999U".into());
        assert_eq!(bridge.bind_addr, addr);
        assert_eq!(bridge.relay_name, "relay1");
        assert_eq!(bridge.port_name, "9999U");
    }

    #[test]
    fn udp_remote_forwarder_new() {
        let addr: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let fwd = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        assert_eq!(fwd.target_addr, addr);
        assert_eq!(fwd.port_name, "5000U");
        assert!(fwd.bind_address.is_none());
    }

    #[test]
    fn udp_remote_forwarder_bind_address_none() {
        let addr: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let fwd = UdpRemoteForwarder::new(addr, "5000U".into(), None);
        assert!(fwd.bind_address.is_none());
    }

    #[tokio::test]
    async fn udp_bind_to_specific_address() {
        // Verify that binding a UDP socket to a specific IP works
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let socket = tokio::net::UdpSocket::bind(bind_addr).await.unwrap();
        let local = socket.local_addr().unwrap();
        assert_eq!(local.ip(), bind_addr.ip());
        assert_ne!(local.port(), 0); // OS assigned a port
    }

    #[test]
    fn udp_remote_forwarder_with_bind_address() {
        let addr: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        let bind: SocketAddr = "192.168.1.1:0".parse().unwrap();
        let fwd = UdpRemoteForwarder::new(addr, "5000U".into(), Some(bind));
        assert_eq!(fwd.target_addr, addr);
        assert_eq!(fwd.port_name, "5000U");
        assert_eq!(fwd.bind_address, Some(bind));
    }
}
