use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use azure_relay::{HybridConnectionClient, HybridConnectionListener};

use crate::config::{Config, LocalForwardBinding, RemoteForwardBinding};

/// The top-level bridge host, managing local and remote forwarding.
pub struct Host {
    config: Config,
    shutdown: Arc<Notify>,
    handles: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl Host {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            shutdown: Arc::new(Notify::new()),
            handles: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Build a connection string for a given relay name, using the config's
    /// global credentials or a per-forward override.
    fn build_connection_string(
        &self,
        relay_name: &str,
        override_cs: Option<&str>,
    ) -> anyhow::Result<String> {
        if let Some(cs) = override_cs {
            if cs.to_lowercase().contains("entitypath=") {
                return Ok(cs.to_string());
            }
            return Ok(format!("{};EntityPath={}", cs, relay_name));
        }

        if let Some(ref cs) = self.config.azure_relay_connection_string {
            let base = if cs.to_lowercase().contains("entitypath=") {
                cs.clone()
            } else {
                format!("{};EntityPath={}", cs, relay_name)
            };
            return Ok(base);
        }

        if let Some(ref endpoint) = self.config.azure_relay_endpoint {
            let mut cs = format!("Endpoint={};EntityPath={}", endpoint, relay_name);
            if let Some(ref key_name) = self.config.azure_relay_shared_access_key_name {
                cs.push_str(&format!(";SharedAccessKeyName={}", key_name));
            }
            if let Some(ref key) = self.config.azure_relay_shared_access_key {
                cs.push_str(&format!(";SharedAccessKey={}", key));
            }
            if let Some(ref sig) = self.config.azure_relay_shared_access_signature {
                cs.push_str(&format!(";SharedAccessSignature={}", sig));
            }
            return Ok(cs);
        }

        anyhow::bail!("No connection string or endpoint configured")
    }

    /// Start all configured forwarders.
    ///
    /// This sets up:
    /// - Local forward bridges (TCP/UDP/Unix socket listeners → relay)
    /// - Remote forward bridges (relay → TCP/UDP/Unix socket targets)
    pub async fn start(&self) -> anyhow::Result<()> {
        info!(
            local_forwards = self.config.local_forward.len(),
            remote_forwards = self.config.remote_forward.len(),
            "Starting host"
        );

        let mut handles = self.handles.lock().await;
        let gateway_ports = self.config.gateway_ports.unwrap_or(false);
        let exit_on_forward_failure = self.config.exit_on_forward_failure.unwrap_or(true);
        let connect_timeout = std::time::Duration::from_secs(
            self.config.connect_timeout.unwrap_or(60) as u64,
        );
        let keep_alive = self.config.keep_alive_interval
            .map(|secs| Duration::from_secs(secs as u64));
        let address_family = self.config.address_family.as_deref();
        let max_attempts = self.config.connection_attempts.unwrap_or(1) as usize;

        // --- Local forward bridges ---
        for lf in &self.config.local_forward {
            let cs = match self.build_connection_string(
                &lf.relay_name,
                lf.connection_string.as_deref(),
            ) {
                Ok(cs) => cs,
                Err(e) => {
                    if exit_on_forward_failure {
                        return Err(e);
                    }
                    warn!(relay = %lf.relay_name, error = %e, "Skipping local forward (connection string failed)");
                    continue;
                }
            };

            let bindings = if lf.bindings.is_empty() {
                vec![LocalForwardBinding {
                    bind_address: lf.bind_address.clone(),
                    bind_port: lf.bind_port.unwrap_or(0),
                    port_name: lf.port_name.clone(),
                    bind_local_socket: lf.bind_local_socket.clone(),
                    no_authentication: lf.no_authentication.unwrap_or(false),
                    ..Default::default()
                }]
            } else {
                lf.bindings.clone()
            };

            'bindings: for binding in &bindings {
                if let Some(ref socket_path) = binding.bind_local_socket {
                    #[cfg(unix)]
                    {
                        let port_name = binding
                            .port_name
                            .clone()
                            .unwrap_or_else(|| "sock".to_string());
                        let bridge = crate::socket::SocketLocalForwardBridge::new(
                            socket_path.clone(),
                            lf.relay_name.clone(),
                            port_name,
                        );
                        let client = 'retry: {
                            let mut last_err = None;
                            for attempt in 1..=max_attempts {
                                match HybridConnectionClient::from_connection_string(&cs) {
                                    Ok(mut c) => {
                                        if let Some(ka) = keep_alive {
                                            c.set_keep_alive_interval(ka);
                                        }
                                        break 'retry Arc::new(c);
                                    }
                                    Err(e) if attempt < max_attempts => {
                                        warn!(
                                            relay = %lf.relay_name,
                                            attempt = attempt,
                                            max = max_attempts,
                                            error = %e,
                                            "Unix socket client creation failed, retrying in 1s..."
                                        );
                                        tokio::time::sleep(Duration::from_secs(1)).await;
                                        last_err = Some(e);
                                    }
                                    Err(e) => {
                                        last_err = Some(e);
                                    }
                                }
                            }
                            let e = last_err.unwrap();
                            if exit_on_forward_failure {
                                return Err(anyhow::anyhow!("failed to create client: {}", e));
                            }
                            warn!(error = %e, "Skipping Unix socket local forward (client creation failed)");
                            continue 'bindings;
                        };
                        let shutdown = self.shutdown.clone();
                        handles.push(tokio::spawn(async move {
                            if let Err(e) = bridge.run(client, shutdown).await {
                                error!(error = %e, "Unix socket local forward bridge failed");
                            }
                        }));
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = socket_path;
                        warn!("Unix socket forwarding not supported on this platform");
                    }
                    continue;
                }

                let port = binding.bind_port;
                if port == 0 {
                    warn!(relay = %lf.relay_name, "Skipping binding with port 0");
                    continue;
                }

                let port_name = binding
                    .port_name
                    .clone()
                    .unwrap_or_else(|| {
                        if port < 0 {
                            format!("{}U", port.unsigned_abs())
                        } else {
                            port.to_string()
                        }
                    });

                let bind_host = if gateway_ports {
                    binding.bind_address.as_deref().unwrap_or("0.0.0.0")
                } else {
                    binding.bind_address.as_deref().unwrap_or("127.0.0.1")
                };

                let abs_port = port.unsigned_abs() as u16;
                let bind_addr = resolve_address(bind_host, abs_port).await?;

                // Address family filtering
                if !is_address_family_allowed(bind_addr, address_family) {
                    warn!(
                        addr = %bind_addr,
                        family = ?address_family,
                        "Skipping bind address (address family mismatch)"
                    );
                    continue;
                }

                if port < 0 {
                    // UDP
                    let bridge = crate::udp::UdpLocalForwardBridge::new(
                        bind_addr,
                        lf.relay_name.clone(),
                        port_name,
                    );
                    let client = 'retry: {
                        let mut last_err = None;
                        for attempt in 1..=max_attempts {
                            match HybridConnectionClient::from_connection_string(&cs) {
                                Ok(mut c) => {
                                    if let Some(ka) = keep_alive {
                                        c.set_keep_alive_interval(ka);
                                    }
                                    break 'retry Arc::new(c);
                                }
                                Err(e) if attempt < max_attempts => {
                                    warn!(
                                        relay = %lf.relay_name,
                                        attempt = attempt,
                                        max = max_attempts,
                                        error = %e,
                                        "UDP client creation failed, retrying in 1s..."
                                    );
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    last_err = Some(e);
                                }
                                Err(e) => {
                                    last_err = Some(e);
                                }
                            }
                        }
                        let e = last_err.unwrap();
                        if exit_on_forward_failure {
                            return Err(anyhow::anyhow!("failed to create client: {}", e));
                        }
                        warn!(error = %e, "Skipping UDP local forward (client creation failed)");
                        continue 'bindings;
                    };
                    let shutdown = self.shutdown.clone();
                    handles.push(tokio::spawn(async move {
                        if let Err(e) = bridge.run(client, shutdown).await {
                            error!(error = %e, "UDP local forward bridge failed");
                        }
                    }));
                } else {
                    // TCP
                    let bridge = crate::tcp::TcpLocalForwardBridge::new(
                        bind_addr,
                        lf.relay_name.clone(),
                        port_name,
                        gateway_ports,
                        connect_timeout,
                    );
                    let client = 'retry: {
                        let mut last_err = None;
                        for attempt in 1..=max_attempts {
                            match HybridConnectionClient::from_connection_string(&cs) {
                                Ok(mut c) => {
                                    if let Some(ka) = keep_alive {
                                        c.set_keep_alive_interval(ka);
                                    }
                                    break 'retry Arc::new(c);
                                }
                                Err(e) if attempt < max_attempts => {
                                    warn!(
                                        relay = %lf.relay_name,
                                        attempt = attempt,
                                        max = max_attempts,
                                        error = %e,
                                        "TCP client creation failed, retrying in 1s..."
                                    );
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    last_err = Some(e);
                                }
                                Err(e) => {
                                    last_err = Some(e);
                                }
                            }
                        }
                        let e = last_err.unwrap();
                        if exit_on_forward_failure {
                            return Err(anyhow::anyhow!("failed to create client: {}", e));
                        }
                        warn!(error = %e, "Skipping TCP local forward (client creation failed)");
                        continue 'bindings;
                    };
                    let shutdown = self.shutdown.clone();
                    handles.push(tokio::spawn(async move {
                        if let Err(e) = bridge.run(client, shutdown).await {
                            error!(error = %e, "TCP local forward bridge failed");
                        }
                    }));
                }
            }
        }

        // --- Remote forward bridges ---
        // Group by relay_name since multiple bindings can share a listener.
        let mut remote_groups: HashMap<String, (String, Vec<RemoteForwardBinding>)> =
            HashMap::new();

        for rf in &self.config.remote_forward {
            let cs = match self.build_connection_string(
                &rf.relay_name,
                rf.connection_string.as_deref(),
            ) {
                Ok(cs) => cs,
                Err(e) => {
                    if exit_on_forward_failure {
                        return Err(e);
                    }
                    warn!(relay = %rf.relay_name, error = %e, "Skipping remote forward (connection string failed)");
                    continue;
                }
            };

            let bindings = if rf.bindings.is_empty() {
                vec![RemoteForwardBinding {
                    host: rf.host.clone(),
                    host_port: rf.host_port.unwrap_or(0),
                    port_name: rf.port_name.clone(),
                    local_socket: rf.local_socket.clone(),
                    http: rf.http.unwrap_or(false),
                    ..Default::default()
                }]
            } else {
                rf.bindings.clone()
            };

            let entry = remote_groups
                .entry(rf.relay_name.clone())
                .or_insert_with(|| (cs, Vec::new()));
            entry.1.extend(bindings);
        }

        'remote_groups: for (relay_name, (cs, bindings)) in remote_groups {
            let listener = 'retry: {
                let mut last_err = None;
                for attempt in 1..=max_attempts {
                    // Note: HybridConnectionListener sets keep_alive_interval at
                    // construction time (DEFAULT_KEEP_ALIVE_INTERVAL) and has no
                    // public setter. A builder pattern or setter would be needed to
                    // override it from config.
                    match HybridConnectionListener::from_connection_string(&cs) {
                        Ok(l) => break 'retry l,
                        Err(e) if attempt < max_attempts => {
                            warn!(
                                relay = %relay_name,
                                attempt = attempt,
                                max = max_attempts,
                                error = %e,
                                "Listener creation failed, retrying in 1s..."
                            );
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            last_err = Some(e);
                        }
                        Err(e) => {
                            last_err = Some(e);
                        }
                    }
                }
                let e = last_err.unwrap();
                if exit_on_forward_failure {
                    return Err(anyhow::anyhow!(
                        "failed to create listener for '{}': {}",
                        relay_name,
                        e
                    ));
                }
                warn!(relay = %relay_name, error = %e, "Skipping remote forward (listener creation failed)");
                continue 'remote_groups;
            };
            let mut forwarders = HashMap::new();
            let mut http_forwarders: Vec<crate::http_remote_forwarder::HttpRemoteForwarder> = Vec::new();

            for binding in &bindings {
                let port = binding.host_port;
                let port_name = binding
                    .port_name
                    .clone()
                    .unwrap_or_else(|| {
                        if port < 0 {
                            format!("{}U", port.unsigned_abs())
                        } else {
                            port.to_string()
                        }
                    });

                if binding.http {
                    let host = binding.host.as_deref().unwrap_or("localhost");
                    let abs_port = port.unsigned_abs() as u16;
                    let https = port_name.to_lowercase().starts_with("https")
                        || binding
                            .path
                            .as_deref()
                            .map_or(false, |p| p.starts_with("https"));
                    let config = crate::http_forward::HttpForwardConfig {
                        host: host.to_string(),
                        port: abs_port,
                        https,
                        insecure: binding.insecure,
                        path_prefix: binding.path.clone(),
                    };
                    match crate::http_remote_forwarder::HttpRemoteForwarder::new(
                        config,
                        relay_name.clone(),
                        port_name.clone(),
                    ) {
                        Ok(fwd) => http_forwarders.push(fwd),
                        Err(e) => {
                            warn!(relay = %relay_name, error = %e, "Failed to create HTTP forwarder");
                            if exit_on_forward_failure {
                                return Err(e);
                            }
                        }
                    }
                    continue;
                }

                if let Some(ref socket_path) = binding.local_socket {
                    #[cfg(unix)]
                    {
                        let fwd = crate::socket::SocketRemoteForwarder::new(
                            socket_path.clone(),
                            port_name.clone(),
                        );
                        forwarders.insert(
                            port_name,
                            crate::remote_bridge::RemoteForwarder::Socket(fwd),
                        );
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = socket_path;
                        warn!("Unix socket remote forwarding not supported on this platform");
                    }
                    continue;
                }

                if port == 0 {
                    warn!(relay = %relay_name, "Skipping remote binding with port 0");
                    continue;
                }

                let host = binding.host.as_deref().unwrap_or("localhost");
                let abs_port = port.unsigned_abs() as u16;
                let target_addr = resolve_address(host, abs_port).await?;

                // Address family filtering
                if !is_address_family_allowed(target_addr, address_family) {
                    warn!(
                        addr = %target_addr,
                        family = ?address_family,
                        "Skipping target address (address family mismatch)"
                    );
                    continue;
                }

                let bind_address = if let Some(ref ba) = self.config.bind_address {
                    Some(resolve_address(ba, 0).await?)
                } else {
                    None
                };

                if port < 0 {
                    let fwd = crate::udp::UdpRemoteForwarder::new(
                        target_addr,
                        port_name.clone(),
                        bind_address,
                    );
                    forwarders.insert(
                        port_name,
                        crate::remote_bridge::RemoteForwarder::Udp(fwd),
                    );
                } else {
                    let fwd = crate::tcp::TcpRemoteForwarder::new(
                        target_addr,
                        port_name.clone(),
                        connect_timeout,
                        bind_address,
                    );
                    forwarders.insert(
                        port_name,
                        crate::remote_bridge::RemoteForwarder::Tcp(fwd),
                    );
                }
            }

            if forwarders.is_empty() && http_forwarders.is_empty() {
                warn!(relay = %relay_name, "No forwarders configured, skipping listener");
                continue;
            }

            let bridge = crate::remote_bridge::RemoteForwardBridge::new(
                listener,
                forwarders,
                http_forwarders,
                relay_name.clone(),
            );
            let shutdown = self.shutdown.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = bridge.run(shutdown).await {
                    error!(relay = %relay_name, error = %e, "Remote forward bridge failed");
                }
            }));
        }

        info!(tasks = handles.len(), "All bridges started");
        Ok(())
    }

    /// Stop all forwarders.
    pub async fn stop(&self) {
        info!("Stopping host");
        self.shutdown.notify_waiters();
        let mut handles = self.handles.lock().await;
        for handle in handles.drain(..) {
            handle.abort();
        }
    }

    /// Returns a future that completes when shutdown is requested.
    pub async fn wait_for_shutdown(&self) {
        self.shutdown.notified().await;
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
}

/// Resolve a host:port string to a SocketAddr, supporting both literal IPs and DNS names.
async fn resolve_address(host: &str, port: u16) -> anyhow::Result<std::net::SocketAddr> {
    let addr_str = format!("{}:{}", host, port);

    // Try direct parse first (for literal IPs like 127.0.0.1)
    if let Ok(addr) = addr_str.parse::<std::net::SocketAddr>() {
        return Ok(addr);
    }

    // DNS lookup — collect all results and pick a random one (matching C# behavior)
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&addr_str)
        .await
        .map_err(|e| anyhow::anyhow!("DNS resolution failed for '{}': {}", addr_str, e))?
        .collect();

    if addrs.is_empty() {
        anyhow::bail!("DNS resolution returned no addresses for '{}'", addr_str);
    }

    // Pick random address (matching C# eligible address selection)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    let idx = hasher.finish() as usize % addrs.len();
    Ok(addrs[idx])
}

/// Check if a socket address matches the configured address family filter.
fn is_address_family_allowed(addr: std::net::SocketAddr, family: Option<&str>) -> bool {
    match family.map(|s| s.to_lowercase()).as_deref() {
        Some("inet") => addr.is_ipv4(),
        Some("inet6") => addr.is_ipv6(),
        _ => true, // "any" or None → allow both
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_new_with_default_config() {
        let config = Config::default();
        let host = Host::new(config);
        assert!(host.config().local_forward.is_empty());
        assert!(host.config().remote_forward.is_empty());
    }

    #[tokio::test]
    async fn host_start_stop() {
        let config = Config::default();
        let host = Host::new(config);
        host.start().await.unwrap();
        host.stop().await;
    }

    #[tokio::test]
    async fn host_shutdown_notification() {
        let config = Config::default();
        let host = Arc::new(Host::new(config));

        let host2 = host.clone();
        let handle = tokio::spawn(async move {
            host2.wait_for_shutdown().await;
            true
        });

        // Give the waiter a moment to register
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        host.stop().await;

        assert!(handle.await.unwrap());
    }

    #[tokio::test]
    async fn resolve_address_literal_ipv4() {
        let addr = resolve_address("127.0.0.1", 8080).await.unwrap();
        assert_eq!(addr, "127.0.0.1:8080".parse().unwrap());
    }

    #[tokio::test]
    async fn resolve_address_localhost() {
        let addr = resolve_address("localhost", 9090).await.unwrap();
        assert_eq!(addr.port(), 9090);
        // localhost should resolve to a loopback address
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn keep_alive_interval_propagated_to_host() {
        let config = Config {
            keep_alive_interval: Some(30),
            ..Config::default()
        };
        let host = Host::new(config);
        assert_eq!(host.config().keep_alive_interval, Some(30));
        let ka = host.config().keep_alive_interval
            .map(|secs| Duration::from_secs(secs as u64));
        assert_eq!(ka, Some(Duration::from_secs(30)));
    }
}
