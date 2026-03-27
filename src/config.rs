use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Regex for validating relay names
// ---------------------------------------------------------------------------

static RELAY_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9A-Za-z/_\-\.]+$").unwrap());

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during configuration loading, validation, and parsing.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Configuration error in {file}: {message}")]
    Validation { file: String, message: String },
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

// ---------------------------------------------------------------------------
// Root configuration
// ---------------------------------------------------------------------------

/// Root configuration for azbridge.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Config {
    pub address_family: Option<String>,
    pub azure_relay_connection_string: Option<String>,
    pub azure_relay_endpoint: Option<String>,
    pub azure_relay_shared_access_key_name: Option<String>,
    pub azure_relay_shared_access_key: Option<String>,
    pub azure_relay_shared_access_signature: Option<String>,
    pub bind_address: Option<String>,
    pub clear_all_forwardings: Option<bool>,
    pub connection_attempts: Option<u32>,
    pub connect_timeout: Option<u32>,
    pub exit_on_forward_failure: Option<bool>,
    pub gateway_ports: Option<bool>,
    pub keep_alive_interval: Option<u32>,
    pub log_level: Option<String>,
    pub log_file_name: Option<String>,
    pub local_forward: Vec<LocalForward>,
    pub remote_forward: Vec<RemoteForward>,
}

impl Config {
    /// Load configuration from a YAML file.
    pub fn from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Validate the configuration, returning the first error found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_with_file("<unknown>")
    }

    /// Validate with a file name for error messages.
    pub fn validate_with_file(&self, file: &str) -> Result<(), ConfigError> {
        if let Some(ref af) = self.address_family {
            let lower = af.to_lowercase();
            if lower != "any" && lower != "inet" && lower != "inet6" {
                return Err(ConfigError::Validation {
                    file: file.to_string(),
                    message: format!(
                        "AddressFamily must be 'any', 'inet', or 'inet6', got '{af}'"
                    ),
                });
            }
        }

        if let Some(attempts) = self.connection_attempts
            && !(1..=10).contains(&attempts) {
                return Err(ConfigError::Validation {
                    file: file.to_string(),
                    message: format!(
                        "ConnectionAttempts must be 1..10, got {attempts}"
                    ),
                });
            }

        if let Some(timeout) = self.connect_timeout
            && timeout > 120 {
                return Err(ConfigError::Validation {
                    file: file.to_string(),
                    message: format!(
                        "ConnectTimeout must be 0..120, got {timeout}"
                    ),
                });
            }

        for lf in &self.local_forward {
            if !lf.relay_name.is_empty() && !RELAY_NAME_REGEX.is_match(&lf.relay_name) {
                return Err(ConfigError::Validation {
                    file: file.to_string(),
                    message: format!(
                        "Invalid LocalForward relay name: '{}'",
                        lf.relay_name
                    ),
                });
            }
            for b in &lf.bindings {
                validate_bind_port(b.bind_port, file)?;
            }
        }

        for rf in &self.remote_forward {
            if !rf.relay_name.is_empty() && !RELAY_NAME_REGEX.is_match(&rf.relay_name) {
                return Err(ConfigError::Validation {
                    file: file.to_string(),
                    message: format!(
                        "Invalid RemoteForward relay name: '{}'",
                        rf.relay_name
                    ),
                });
            }
            for b in &rf.bindings {
                validate_host_port(b.host_port, file)?;
            }
        }

        Ok(())
    }

    /// Merge another config into this one. Non-None values from `other` override.
    pub fn merge(&mut self, other: &Config) {
        macro_rules! merge_opt {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field.clone_from(&other.$field);
                }
            };
        }
        merge_opt!(address_family);
        merge_opt!(azure_relay_connection_string);
        merge_opt!(azure_relay_endpoint);
        merge_opt!(azure_relay_shared_access_key_name);
        merge_opt!(azure_relay_shared_access_key);
        merge_opt!(azure_relay_shared_access_signature);
        merge_opt!(bind_address);
        merge_opt!(clear_all_forwardings);
        merge_opt!(connection_attempts);
        merge_opt!(connect_timeout);
        merge_opt!(exit_on_forward_failure);
        merge_opt!(gateway_ports);
        merge_opt!(keep_alive_interval);
        merge_opt!(log_level);
        merge_opt!(log_file_name);

        if other.clear_all_forwardings == Some(true) {
            self.local_forward.clear();
            self.remote_forward.clear();
        }

        self.local_forward
            .extend(other.local_forward.iter().cloned());
        self.remote_forward
            .extend(other.remote_forward.iter().cloned());
    }
}

#[allow(dead_code)]
fn validate_bind_port(port: i32, file: &str) -> Result<(), ConfigError> {
    if port == 0 || !(-65535..=65535).contains(&port) {
        return Err(ConfigError::Validation {
            file: file.to_string(),
            message: format!("bind_port must be in -65535..=65535 and not 0, got {port}"),
        });
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_host_port(port: i32, file: &str) -> Result<(), ConfigError> {
    if port == 0 || !(-65535..=65535).contains(&port) {
        return Err(ConfigError::Validation {
            file: file.to_string(),
            message: format!("host_port must be in -65535..=65535 and not 0, got {port}"),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LocalForward
// ---------------------------------------------------------------------------

/// A local forwarding rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
#[derive(Default)]
pub struct LocalForward {
    pub relay_name: String,
    pub connection_string: Option<String>,
    pub bindings: Vec<LocalForwardBinding>,
    // Convenience single-binding fields (proxy to bindings[0])
    #[serde(skip_serializing)]
    pub bind_address: Option<String>,
    #[serde(skip_serializing)]
    pub bind_port: Option<i32>,
    #[serde(skip_serializing)]
    pub port_name: Option<String>,
    #[serde(skip_serializing)]
    pub bind_local_socket: Option<String>,
    #[serde(skip_serializing)]
    pub host_name: Option<String>,
    #[serde(skip_serializing)]
    pub no_authentication: Option<bool>,
}


impl LocalForward {
    /// Promote convenience single-binding fields into `bindings[0]` when
    /// `bindings` is empty. Called after deserialization to normalise configs
    /// that use the flat shorthand form.
    pub fn normalize(&mut self) {
        if !self.bindings.is_empty() {
            return;
        }

        let has_convenience = self.bind_address.is_some()
            || self.bind_port.is_some()
            || self.port_name.is_some()
            || self.bind_local_socket.is_some()
            || self.host_name.is_some()
            || self.no_authentication.is_some();

        if !has_convenience {
            return;
        }

        self.bindings.push(LocalForwardBinding {
            bind_address: self.bind_address.take(),
            host_name: self.host_name.take(),
            bind_port: self.bind_port.take().unwrap_or(0),
            port_name: self.port_name.take(),
            bind_local_socket: self.bind_local_socket.take(),
            no_authentication: self.no_authentication.take().unwrap_or(false),
        });
    }
}

/// A binding for a local forwarding rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LocalForwardBinding {
    pub bind_address: Option<String>,
    pub host_name: Option<String>,
    /// Port number. Negative = UDP.
    pub bind_port: i32,
    pub port_name: Option<String>,
    pub bind_local_socket: Option<String>,
    pub no_authentication: bool,
}

// ---------------------------------------------------------------------------
// RemoteForward
// ---------------------------------------------------------------------------

/// A remote forwarding rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
#[derive(Default)]
pub struct RemoteForward {
    pub relay_name: String,
    pub connection_string: Option<String>,
    pub bindings: Vec<RemoteForwardBinding>,
    // Convenience fields
    #[serde(skip_serializing)]
    pub host: Option<String>,
    #[serde(skip_serializing)]
    pub host_port: Option<i32>,
    #[serde(skip_serializing)]
    pub port_name: Option<String>,
    #[serde(skip_serializing)]
    pub local_socket: Option<String>,
    #[serde(skip_serializing)]
    pub http: Option<bool>,
}


impl RemoteForward {
    /// Promote convenience single-binding fields into `bindings[0]` when
    /// `bindings` is empty.
    pub fn normalize(&mut self) {
        if !self.bindings.is_empty() {
            return;
        }

        let has_convenience = self.host.is_some()
            || self.host_port.is_some()
            || self.port_name.is_some()
            || self.local_socket.is_some()
            || self.http.is_some();

        if !has_convenience {
            return;
        }

        self.bindings.push(RemoteForwardBinding {
            host: self.host.take(),
            host_port: self.host_port.take().unwrap_or(0),
            port_name: self.port_name.take(),
            local_socket: self.local_socket.take(),
            http: self.http.take().unwrap_or(false),
            insecure: false,
            path: None,
        });
    }
}

/// A binding for a remote forwarding rule.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RemoteForwardBinding {
    pub host: Option<String>,
    pub host_port: i32,
    pub port_name: Option<String>,
    pub local_socket: Option<String>,
    pub http: bool,
    pub insecure: bool,
    pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// Forwarding expression parsers
// ---------------------------------------------------------------------------

/// Parse a `-L` expression: `[bind_address:]port[/port_name]{;...}:relay_name`
pub fn parse_local_forward(expr: &str) -> Result<LocalForward, ConfigError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(ConfigError::Parse(
            "Empty local forward expression".to_string(),
        ));
    }

    // Split on the LAST colon to separate bindings from relay_name.
    let last_colon = expr.rfind(':').ok_or_else(|| {
        ConfigError::Parse(format!(
            "Invalid local forward expression (no colon separator): '{expr}'"
        ))
    })?;

    let left = &expr[..last_colon];
    let relay_name = &expr[last_colon + 1..];

    if relay_name.is_empty() {
        return Err(ConfigError::Parse(
            "Empty relay name in local forward expression".to_string(),
        ));
    }

    if !RELAY_NAME_REGEX.is_match(relay_name) {
        return Err(ConfigError::Parse(format!(
            "Invalid relay name: '{relay_name}'"
        )));
    }

    if left.is_empty() {
        return Err(ConfigError::Parse(
            "No binding specification in local forward expression".to_string(),
        ));
    }

    // Split on ';' for multiple bindings
    let binding_strs: Vec<&str> = left.split(';').collect();
    let mut bindings = Vec::new();

    for bs in binding_strs {
        bindings.push(parse_local_binding(bs)?);
    }

    Ok(LocalForward {
        relay_name: relay_name.to_string(),
        bindings,
        ..Default::default()
    })
}

/// Parse a single local binding segment such as `29876`, `127.0.0.1:29876`,
/// `29876/myport`, `29876U`, or `/tmp/mysocket`.
fn parse_local_binding(s: &str) -> Result<LocalForwardBinding, ConfigError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ConfigError::Parse(
            "Empty binding in local forward expression".to_string(),
        ));
    }

    // Try to split on colon for host:port form.
    if let Some(colon_pos) = s.find(':') {
        let host_part = &s[..colon_pos];
        let port_part = &s[colon_pos + 1..];
        let (port, port_name) = parse_port_with_name(port_part)?;
        Ok(LocalForwardBinding {
            bind_address: Some(host_part.to_string()),
            bind_port: port,
            port_name,
            ..Default::default()
        })
    } else {
        // Single segment: port, port/name, portU, or unix socket
        let (port, port_name) = match parse_port_with_name(s) {
            Ok(result) => result,
            Err(_) => {
                // Treat as unix socket path
                return Ok(LocalForwardBinding {
                    bind_local_socket: Some(s.to_string()),
                    ..Default::default()
                });
            }
        };

        if port == 0 {
            // Non-numeric, treat as socket
            return Ok(LocalForwardBinding {
                bind_local_socket: Some(s.to_string()),
                ..Default::default()
            });
        }

        Ok(LocalForwardBinding {
            bind_port: port,
            port_name,
            ..Default::default()
        })
    }
}

/// Parse a port string that may have a `/port_name` suffix and/or a `U` suffix
/// for UDP. Returns `(port_number, optional_port_name)`.
/// A `U` suffix makes the port negative.
fn parse_port_with_name(s: &str) -> Result<(i32, Option<String>), ConfigError> {
    if let Some(slash_pos) = s.find('/') {
        let left = &s[..slash_pos];
        let right = &s[slash_pos + 1..];

        // Try left as port
        if let Some(port) = try_parse_port(left) {
            return Ok((port, Some(right.to_string())));
        }
        // Try right as port
        if let Some(port) = try_parse_port(right) {
            return Ok((port, Some(left.to_string())));
        }
        return Err(ConfigError::Parse(format!(
            "Cannot parse port from '{s}'"
        )));
    }

    // No slash — just a port (possibly with U suffix)
    if let Some(port) = try_parse_port(s) {
        Ok((port, None))
    } else {
        Err(ConfigError::Parse(format!(
            "Cannot parse port number from '{s}'"
        )))
    }
}

/// Try to parse a port string like "29876" or "29876U".
/// Returns negative for UDP (U suffix).
fn try_parse_port(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_str, is_udp) = if s.ends_with('U') || s.ends_with('u') {
        (&s[..s.len() - 1], true)
    } else {
        (s, false)
    };

    let port: i32 = num_str.parse().ok()?;
    if port <= 0 || port > 65535 {
        return None;
    }
    Some(if is_udp { -port } else { port })
}

/// Parse a `-T` expression (new format): `relay_name:[port_name/][host:]hostport{;...}`
pub fn parse_remote_forward(expr: &str) -> Result<RemoteForward, ConfigError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(ConfigError::Parse(
            "Empty remote forward expression".to_string(),
        ));
    }

    // Split on the FIRST colon to get relay_name
    let first_colon = expr.find(':').ok_or_else(|| {
        ConfigError::Parse(format!(
            "Invalid remote forward expression (no colon separator): '{expr}'"
        ))
    })?;

    let relay_name = &expr[..first_colon];
    let right = &expr[first_colon + 1..];

    if relay_name.is_empty() {
        return Err(ConfigError::Parse(
            "Empty relay name in remote forward expression".to_string(),
        ));
    }

    if !RELAY_NAME_REGEX.is_match(relay_name) {
        return Err(ConfigError::Parse(format!(
            "Invalid relay name: '{relay_name}'"
        )));
    }

    if right.is_empty() {
        return Err(ConfigError::Parse(
            "No binding specification in remote forward expression".to_string(),
        ));
    }

    // Split on ';' for multiple bindings
    let binding_strs: Vec<&str> = right.split(';').collect();
    let mut bindings = Vec::new();

    for bs in binding_strs {
        bindings.push(parse_remote_binding(bs)?);
    }

    Ok(RemoteForward {
        relay_name: relay_name.to_string(),
        bindings,
        ..Default::default()
    })
}

/// Parse a single remote binding segment in -T (new) format:
/// `[port_name/][host:]port_or_socket`
fn parse_remote_binding(s: &str) -> Result<RemoteForwardBinding, ConfigError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ConfigError::Parse(
            "Empty binding in remote forward expression".to_string(),
        ));
    }

    // Check for host:port form (colon present)
    if let Some(colon_pos) = s.rfind(':') {
        let left = &s[..colon_pos];
        let right = &s[colon_pos + 1..];

        // right should be the port (possibly with U)
        if let Some(port) = try_parse_port(right) {
            // left may be [port_name/]host
            let (port_name, host) = split_port_name_prefix(left);
            return Ok(RemoteForwardBinding {
                host: Some(host.to_string()),
                host_port: port,
                port_name: port_name.map(|s| s.to_string()),
                ..Default::default()
            });
        }

        // Could not parse right as port — try the whole thing as a socket
        return Ok(RemoteForwardBinding {
            local_socket: Some(s.to_string()),
            ..Default::default()
        });
    }

    // No colon — single segment: [port_name/]port or socket path
    if let Some(slash_pos) = s.find('/') {
        let left = &s[..slash_pos];
        let right = &s[slash_pos + 1..];

        // Try right as port (port_name/port form)
        if let Some(port) = try_parse_port(right) {
            return Ok(RemoteForwardBinding {
                host_port: port,
                port_name: Some(left.to_string()),
                ..Default::default()
            });
        }
        // Try left as port (port/port_name form)
        if let Some(port) = try_parse_port(left) {
            return Ok(RemoteForwardBinding {
                host_port: port,
                port_name: Some(right.to_string()),
                ..Default::default()
            });
        }
        // Treat as socket path
        return Ok(RemoteForwardBinding {
            local_socket: Some(s.to_string()),
            ..Default::default()
        });
    }

    // Pure port or socket
    if let Some(port) = try_parse_port(s) {
        Ok(RemoteForwardBinding {
            host_port: port,
            ..Default::default()
        })
    } else {
        Ok(RemoteForwardBinding {
            local_socket: Some(s.to_string()),
            ..Default::default()
        })
    }
}

/// Split an optional `port_name/rest` prefix. Returns `(Some(port_name), rest)`
/// if a slash is present and the left side is non-numeric, otherwise
/// `(None, original)`.
fn split_port_name_prefix(s: &str) -> (Option<&str>, &str) {
    if let Some(slash_pos) = s.find('/') {
        let left = &s[..slash_pos];
        let right = &s[slash_pos + 1..];
        // If left is purely numeric it's not a port name
        if left.parse::<u16>().is_ok() {
            (None, s)
        } else {
            (Some(left), right)
        }
    } else {
        (None, s)
    }
}

/// Parse a `-R` expression (legacy format):
/// `relay_name:host:[port_name/]port{;...}`
pub fn parse_remote_forward_legacy(expr: &str) -> Result<RemoteForward, ConfigError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(ConfigError::Parse(
            "Empty remote forward expression".to_string(),
        ));
    }

    // Split on the FIRST colon to get relay_name
    let first_colon = expr.find(':').ok_or_else(|| {
        ConfigError::Parse(format!(
            "Invalid remote forward expression (no colon separator): '{expr}'"
        ))
    })?;

    let relay_name = &expr[..first_colon];
    let right = &expr[first_colon + 1..];

    if relay_name.is_empty() {
        return Err(ConfigError::Parse(
            "Empty relay name in remote forward expression".to_string(),
        ));
    }

    if !RELAY_NAME_REGEX.is_match(relay_name) {
        return Err(ConfigError::Parse(format!(
            "Invalid relay name: '{relay_name}'"
        )));
    }

    if right.is_empty() {
        return Err(ConfigError::Parse(
            "No binding specification in remote forward expression".to_string(),
        ));
    }

    // Split on ';' for multiple bindings
    let binding_strs: Vec<&str> = right.split(';').collect();
    let mut bindings = Vec::new();

    for bs in binding_strs {
        bindings.push(parse_remote_binding_legacy(bs)?);
    }

    Ok(RemoteForward {
        relay_name: relay_name.to_string(),
        bindings,
        ..Default::default()
    })
}

/// Parse a single remote binding segment in -R (legacy) format:
/// `host:[port_name/]port` or `[port_name/]port` or socket path
fn parse_remote_binding_legacy(s: &str) -> Result<RemoteForwardBinding, ConfigError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ConfigError::Parse(
            "Empty binding in remote forward expression".to_string(),
        ));
    }

    // Legacy format: host:port_name/port or host:port
    if let Some(colon_pos) = s.find(':') {
        let host = &s[..colon_pos];
        let port_segment = &s[colon_pos + 1..];

        // port_segment may be "port_name/port" or just "port"
        let (port, port_name) = parse_port_with_name(port_segment)?;
        return Ok(RemoteForwardBinding {
            host: Some(host.to_string()),
            host_port: port,
            port_name,
            ..Default::default()
        });
    }

    // No colon — single segment: [port_name/]port or socket path
    if let Some(slash_pos) = s.find('/') {
        let left = &s[..slash_pos];
        let right = &s[slash_pos + 1..];

        if let Some(port) = try_parse_port(right) {
            return Ok(RemoteForwardBinding {
                host_port: port,
                port_name: Some(left.to_string()),
                ..Default::default()
            });
        }
        if let Some(port) = try_parse_port(left) {
            return Ok(RemoteForwardBinding {
                host_port: port,
                port_name: Some(right.to_string()),
                ..Default::default()
            });
        }
        // Treat as socket path
        return Ok(RemoteForwardBinding {
            local_socket: Some(s.to_string()),
            ..Default::default()
        });
    }

    // Pure port or socket
    if let Some(port) = try_parse_port(s) {
        Ok(RemoteForwardBinding {
            host_port: port,
            ..Default::default()
        })
    } else {
        Ok(RemoteForwardBinding {
            local_socket: Some(s.to_string()),
            ..Default::default()
        })
    }
}

/// Parse a `-H` expression:
/// `relay_name:{http|https}/[host][/path]:hostport{;...}`
pub fn parse_remote_http_forward(expr: &str) -> Result<RemoteForward, ConfigError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(ConfigError::Parse(
            "Empty HTTP forward expression".to_string(),
        ));
    }

    // Split on first colon -> relay_name
    let first_colon = expr.find(':').ok_or_else(|| {
        ConfigError::Parse(format!(
            "Invalid HTTP forward expression (no colon separator): '{expr}'"
        ))
    })?;

    let relay_name = &expr[..first_colon];
    let right = &expr[first_colon + 1..];

    if relay_name.is_empty() {
        return Err(ConfigError::Parse(
            "Empty relay name in HTTP forward expression".to_string(),
        ));
    }

    if !RELAY_NAME_REGEX.is_match(relay_name) {
        return Err(ConfigError::Parse(format!(
            "Invalid relay name: '{relay_name}'"
        )));
    }

    if right.is_empty() {
        return Err(ConfigError::Parse(
            "No binding specification in HTTP forward expression".to_string(),
        ));
    }

    // Split on ';' for multiple bindings
    let binding_strs: Vec<&str> = right.split(';').collect();
    let mut bindings = Vec::new();

    for bs in binding_strs {
        bindings.push(parse_http_binding(bs)?);
    }

    Ok(RemoteForward {
        relay_name: relay_name.to_string(),
        bindings,
        ..Default::default()
    })
}

/// Parse a single HTTP binding segment: `{http|https}/[host][/path]:hostport`
/// or `{http|https}/[host][/path]` (uses default port).
fn parse_http_binding(s: &str) -> Result<RemoteForwardBinding, ConfigError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ConfigError::Parse(
            "Empty binding in HTTP forward expression".to_string(),
        ));
    }

    // Must start with http/ or https/
    let (scheme, rest) = if let Some(rest) = s.strip_prefix("https/") {
        ("https", rest)
    } else if let Some(rest) = s.strip_prefix("http/") {
        ("http", rest)
    } else {
        return Err(ConfigError::Parse(format!(
            "HTTP forward binding must start with 'http/' or 'https/', got '{s}'"
        )));
    };

    let is_https = scheme == "https";
    let default_port: i32 = if is_https { 443 } else { 80 };

    // rest is [host][/path]:hostport  or  [host][/path]
    // Split on the LAST colon to try to find a port
    let (host_path_part, port) = if let Some(last_colon) = rest.rfind(':') {
        let maybe_port_str = &rest[last_colon + 1..];
        if let Ok(p) = maybe_port_str.parse::<i32>() {
            if p > 0 && p <= 65535 {
                (&rest[..last_colon], p)
            } else {
                (rest, default_port)
            }
        } else {
            (rest, default_port)
        }
    } else {
        (rest, default_port)
    };

    // host_path_part is [host][/path]
    let (host, path) = if let Some(slash_pos) = host_path_part.find('/') {
        let h = &host_path_part[..slash_pos];
        let p = &host_path_part[slash_pos..]; // includes leading /
        (
            if h.is_empty() { None } else { Some(h.to_string()) },
            if p == "/" { None } else { Some(p.to_string()) },
        )
    } else {
        (
            if host_path_part.is_empty() {
                None
            } else {
                Some(host_path_part.to_string())
            },
            None,
        )
    };

    Ok(RemoteForwardBinding {
        host,
        host_port: port,
        port_name: Some(scheme.to_string()),
        http: true,
        insecure: false,
        path,
        ..Default::default()
    })
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // -L parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_local_forward_port_only() {
        let lf = parse_local_forward("29876:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings.len(), 1);
        assert_eq!(lf.bindings[0].bind_port, 29876);
        assert!(lf.bindings[0].bind_address.is_none());
    }

    #[test]
    fn parse_local_forward_host_port() {
        let lf = parse_local_forward("127.0.0.1:29876:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings.len(), 1);
        assert_eq!(
            lf.bindings[0].bind_address.as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(lf.bindings[0].bind_port, 29876);
    }

    #[test]
    fn parse_local_forward_with_port_name() {
        let lf = parse_local_forward("29876/myport:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings[0].bind_port, 29876);
        assert_eq!(lf.bindings[0].port_name.as_deref(), Some("myport"));
    }

    #[test]
    fn parse_local_forward_with_port_name_reversed() {
        let lf = parse_local_forward("myport/29876:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings[0].bind_port, 29876);
        assert_eq!(lf.bindings[0].port_name.as_deref(), Some("myport"));
    }

    #[test]
    fn parse_local_forward_udp() {
        let lf = parse_local_forward("29876U:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings[0].bind_port, -29876);
    }

    #[test]
    fn parse_local_forward_multiple_bindings() {
        let lf = parse_local_forward("29876;29877:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings.len(), 2);
        assert_eq!(lf.bindings[0].bind_port, 29876);
        assert_eq!(lf.bindings[1].bind_port, 29877);
    }

    #[test]
    fn parse_local_forward_multiple_bindings_with_hosts() {
        let lf =
            parse_local_forward("127.0.0.1:29876;127.0.0.2:29877:myrelay")
                .unwrap();
        assert_eq!(lf.bindings.len(), 2);
        assert_eq!(
            lf.bindings[0].bind_address.as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(lf.bindings[0].bind_port, 29876);
        assert_eq!(
            lf.bindings[1].bind_address.as_deref(),
            Some("127.0.0.2")
        );
        assert_eq!(lf.bindings[1].bind_port, 29877);
    }

    #[test]
    fn parse_local_forward_unix_socket() {
        let lf = parse_local_forward("/tmp/mysocket:myrelay").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings.len(), 1);
        assert_eq!(
            lf.bindings[0].bind_local_socket.as_deref(),
            Some("/tmp/mysocket")
        );
    }

    #[test]
    fn parse_local_forward_empty_fails() {
        assert!(parse_local_forward("").is_err());
    }

    #[test]
    fn parse_local_forward_no_relay_fails() {
        assert!(parse_local_forward("29876").is_err());
    }

    #[test]
    fn parse_local_forward_empty_relay_name_fails() {
        assert!(parse_local_forward("29876:").is_err());
    }

    #[test]
    fn parse_local_forward_invalid_relay_name_fails() {
        assert!(parse_local_forward("29876:relay name!").is_err());
    }

    // -----------------------------------------------------------------------
    // -T parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_remote_forward_port_only() {
        let rf = parse_remote_forward("myrelay:29876").unwrap();
        assert_eq!(rf.relay_name, "myrelay");
        assert_eq!(rf.bindings.len(), 1);
        assert_eq!(rf.bindings[0].host_port, 29876);
    }

    #[test]
    fn parse_remote_forward_host_port() {
        let rf = parse_remote_forward("myrelay:hostname:29876").unwrap();
        assert_eq!(rf.relay_name, "myrelay");
        assert_eq!(rf.bindings[0].host.as_deref(), Some("hostname"));
        assert_eq!(rf.bindings[0].host_port, 29876);
    }

    #[test]
    fn parse_remote_forward_with_port_name() {
        let rf = parse_remote_forward("myrelay:myport/29876").unwrap();
        assert_eq!(rf.bindings[0].host_port, 29876);
        assert_eq!(rf.bindings[0].port_name.as_deref(), Some("myport"));
    }

    #[test]
    fn parse_remote_forward_udp() {
        let rf = parse_remote_forward("myrelay:29876U").unwrap();
        assert_eq!(rf.bindings[0].host_port, -29876);
    }

    #[test]
    fn parse_remote_forward_unix_socket() {
        let rf = parse_remote_forward("myrelay:/tmp/mysocket").unwrap();
        assert_eq!(
            rf.bindings[0].local_socket.as_deref(),
            Some("/tmp/mysocket")
        );
    }

    #[test]
    fn parse_remote_forward_multiple_bindings() {
        let rf = parse_remote_forward("myrelay:29876;29877").unwrap();
        assert_eq!(rf.bindings.len(), 2);
        assert_eq!(rf.bindings[0].host_port, 29876);
        assert_eq!(rf.bindings[1].host_port, 29877);
    }

    #[test]
    fn parse_remote_forward_relay_name_with_special_chars() {
        let rf =
            parse_remote_forward("my_relay/sub.name-1:29876").unwrap();
        assert_eq!(rf.relay_name, "my_relay/sub.name-1");
        assert_eq!(rf.bindings[0].host_port, 29876);
    }

    #[test]
    fn parse_remote_forward_empty_fails() {
        assert!(parse_remote_forward("").is_err());
    }

    #[test]
    fn parse_remote_forward_no_binding_fails() {
        assert!(parse_remote_forward("myrelay:").is_err());
    }

    // -----------------------------------------------------------------------
    // -R (legacy) parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_remote_forward_legacy_host_port() {
        let rf =
            parse_remote_forward_legacy("myrelay:myhost:8080").unwrap();
        assert_eq!(rf.relay_name, "myrelay");
        assert_eq!(rf.bindings[0].host.as_deref(), Some("myhost"));
        assert_eq!(rf.bindings[0].host_port, 8080);
    }

    #[test]
    fn parse_remote_forward_legacy_host_portname_port() {
        let rf = parse_remote_forward_legacy("myrelay:myhost:pname/8080")
            .unwrap();
        assert_eq!(rf.bindings[0].host.as_deref(), Some("myhost"));
        assert_eq!(rf.bindings[0].host_port, 8080);
        assert_eq!(rf.bindings[0].port_name.as_deref(), Some("pname"));
    }

    #[test]
    fn parse_remote_forward_legacy_port_only() {
        let rf = parse_remote_forward_legacy("myrelay:8080").unwrap();
        assert_eq!(rf.bindings[0].host_port, 8080);
    }

    // -----------------------------------------------------------------------
    // -H parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_http_forward_basic() {
        let rf =
            parse_remote_http_forward("myrelay:http/localhost:8080")
                .unwrap();
        assert_eq!(rf.relay_name, "myrelay");
        assert_eq!(rf.bindings.len(), 1);
        assert!(rf.bindings[0].http);
        assert_eq!(rf.bindings[0].host.as_deref(), Some("localhost"));
        assert_eq!(rf.bindings[0].host_port, 8080);
        assert_eq!(rf.bindings[0].port_name.as_deref(), Some("http"));
    }

    #[test]
    fn parse_http_forward_https() {
        let rf =
            parse_remote_http_forward("myrelay:https/localhost:443")
                .unwrap();
        assert!(rf.bindings[0].http);
        assert!(!rf.bindings[0].insecure);
        assert_eq!(rf.bindings[0].host.as_deref(), Some("localhost"));
        assert_eq!(rf.bindings[0].host_port, 443);
        assert_eq!(rf.bindings[0].port_name.as_deref(), Some("https"));
    }

    #[test]
    fn parse_http_forward_with_path() {
        let rf = parse_remote_http_forward(
            "myrelay:http/localhost/api/v1:8080",
        )
        .unwrap();
        assert_eq!(rf.bindings[0].host.as_deref(), Some("localhost"));
        assert_eq!(rf.bindings[0].host_port, 8080);
        assert_eq!(rf.bindings[0].path.as_deref(), Some("/api/v1"));
    }

    #[test]
    fn parse_http_forward_default_port_http() {
        let rf =
            parse_remote_http_forward("myrelay:http/localhost").unwrap();
        assert_eq!(rf.bindings[0].host_port, 80);
        assert_eq!(rf.bindings[0].host.as_deref(), Some("localhost"));
    }

    #[test]
    fn parse_http_forward_default_port_https() {
        let rf =
            parse_remote_http_forward("myrelay:https/localhost").unwrap();
        assert_eq!(rf.bindings[0].host_port, 443);
    }

    #[test]
    fn parse_http_forward_bad_scheme_fails() {
        assert!(
            parse_remote_http_forward("myrelay:ftp/localhost:21").is_err()
        );
    }

    #[test]
    fn parse_http_forward_empty_fails() {
        assert!(parse_remote_http_forward("").is_err());
    }

    // -----------------------------------------------------------------------
    // Config YAML round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn config_yaml_round_trip() {
        let config = Config {
            address_family: Some("inet".to_string()),
            azure_relay_connection_string: Some("Endpoint=sb://test.servicebus.windows.net/;SharedAccessKeyName=send;SharedAccessKey=abc123=".to_string()),
            azure_relay_endpoint: Some("sb://test.servicebus.windows.net/".to_string()),
            bind_address: Some("127.0.0.1".to_string()),
            connection_attempts: Some(3),
            connect_timeout: Some(30),
            exit_on_forward_failure: Some(true),
            gateway_ports: Some(false),
            keep_alive_interval: Some(15),
            log_level: Some("info".to_string()),
            local_forward: vec![LocalForward {
                relay_name: "myrelay".to_string(),
                bindings: vec![LocalForwardBinding {
                    bind_port: 8080,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            remote_forward: vec![RemoteForward {
                relay_name: "myremote".to_string(),
                bindings: vec![RemoteForwardBinding {
                    host_port: 9090,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let deserialized: Config = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(
            deserialized.address_family.as_deref(),
            Some("inet")
        );
        assert_eq!(deserialized.connection_attempts, Some(3));
        assert_eq!(deserialized.connect_timeout, Some(30));
        assert_eq!(deserialized.local_forward.len(), 1);
        assert_eq!(deserialized.local_forward[0].relay_name, "myrelay");
        assert_eq!(
            deserialized.local_forward[0].bindings[0].bind_port,
            8080
        );
        assert_eq!(deserialized.remote_forward.len(), 1);
        assert_eq!(
            deserialized.remote_forward[0].relay_name,
            "myremote"
        );
    }

    #[test]
    fn config_yaml_deserialize_pascal_case() {
        let yaml = r#"
AddressFamily: inet
AzureRelayConnectionString: "conn_string"
LocalForward:
  - RelayName: lr1
    Bindings:
      - BindPort: 8080
RemoteForward:
  - RelayName: rr1
    Bindings:
      - HostPort: 9090
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.address_family.as_deref(), Some("inet"));
        assert_eq!(
            config.azure_relay_connection_string.as_deref(),
            Some("conn_string")
        );
        assert_eq!(config.local_forward[0].relay_name, "lr1");
        assert_eq!(config.local_forward[0].bindings[0].bind_port, 8080);
        assert_eq!(config.remote_forward[0].relay_name, "rr1");
        assert_eq!(
            config.remote_forward[0].bindings[0].host_port,
            9090
        );
    }

    // -----------------------------------------------------------------------
    // Validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn validate_good_config() {
        let config = Config {
            address_family: Some("inet".to_string()),
            connection_attempts: Some(3),
            connect_timeout: Some(30),
            local_forward: vec![LocalForward {
                relay_name: "valid-relay".to_string(),
                bindings: vec![LocalForwardBinding {
                    bind_port: 8080,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_bad_address_family() {
        let config = Config {
            address_family: Some("ipx".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_connection_attempts_too_low() {
        let config = Config {
            connection_attempts: Some(0),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_connection_attempts_too_high() {
        let config = Config {
            connection_attempts: Some(11),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_connect_timeout_range() {
        let config = Config {
            connect_timeout: Some(121),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let ok = Config {
            connect_timeout: Some(120),
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn validate_bad_relay_name() {
        let config = Config {
            local_forward: vec![LocalForward {
                relay_name: "bad relay!".to_string(),
                bindings: vec![LocalForwardBinding {
                    bind_port: 80,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_bad_remote_relay_name() {
        let config = Config {
            remote_forward: vec![RemoteForward {
                relay_name: "bad relay!".to_string(),
                bindings: vec![RemoteForwardBinding {
                    host_port: 80,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    // -----------------------------------------------------------------------
    // Merge tests
    // -----------------------------------------------------------------------

    #[test]
    fn merge_overrides_values() {
        let mut base = Config {
            address_family: Some("inet".to_string()),
            connection_attempts: Some(1),
            local_forward: vec![LocalForward {
                relay_name: "base-relay".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let overlay = Config {
            address_family: Some("inet6".to_string()),
            log_level: Some("debug".to_string()),
            local_forward: vec![LocalForward {
                relay_name: "overlay-relay".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        base.merge(&overlay);
        assert_eq!(base.address_family.as_deref(), Some("inet6"));
        assert_eq!(base.connection_attempts, Some(1)); // not overridden
        assert_eq!(base.log_level.as_deref(), Some("debug"));
        assert_eq!(base.local_forward.len(), 2);
        assert_eq!(base.local_forward[0].relay_name, "base-relay");
        assert_eq!(base.local_forward[1].relay_name, "overlay-relay");
    }

    #[test]
    fn merge_clear_all_forwardings() {
        let mut base = Config {
            local_forward: vec![LocalForward {
                relay_name: "lf1".to_string(),
                ..Default::default()
            }],
            remote_forward: vec![RemoteForward {
                relay_name: "rf1".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let overlay = Config {
            clear_all_forwardings: Some(true),
            local_forward: vec![LocalForward {
                relay_name: "new-lf".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        base.merge(&overlay);
        assert_eq!(base.local_forward.len(), 1);
        assert_eq!(base.local_forward[0].relay_name, "new-lf");
        assert_eq!(base.remote_forward.len(), 0);
    }

    #[test]
    fn merge_none_does_not_override() {
        let mut base = Config {
            address_family: Some("inet".to_string()),
            ..Default::default()
        };
        let overlay = Config::default();
        base.merge(&overlay);
        assert_eq!(base.address_family.as_deref(), Some("inet"));
    }

    // -----------------------------------------------------------------------
    // Normalize tests
    // -----------------------------------------------------------------------

    #[test]
    fn local_forward_normalize_promotes_fields() {
        let mut lf = LocalForward {
            relay_name: "test".to_string(),
            bind_port: Some(8080),
            bind_address: Some("127.0.0.1".to_string()),
            port_name: Some("myport".to_string()),
            ..Default::default()
        };
        lf.normalize();
        assert_eq!(lf.bindings.len(), 1);
        assert_eq!(lf.bindings[0].bind_port, 8080);
        assert_eq!(
            lf.bindings[0].bind_address.as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(lf.bindings[0].port_name.as_deref(), Some("myport"));
        // Convenience fields should be consumed
        assert!(lf.bind_port.is_none());
        assert!(lf.bind_address.is_none());
    }

    #[test]
    fn local_forward_normalize_noop_when_bindings_exist() {
        let mut lf = LocalForward {
            relay_name: "test".to_string(),
            bindings: vec![LocalForwardBinding {
                bind_port: 9090,
                ..Default::default()
            }],
            bind_port: Some(8080),
            ..Default::default()
        };
        lf.normalize();
        assert_eq!(lf.bindings.len(), 1);
        assert_eq!(lf.bindings[0].bind_port, 9090);
    }

    #[test]
    fn remote_forward_normalize_promotes_fields() {
        let mut rf = RemoteForward {
            relay_name: "test".to_string(),
            host: Some("myhost".to_string()),
            host_port: Some(3000),
            ..Default::default()
        };
        rf.normalize();
        assert_eq!(rf.bindings.len(), 1);
        assert_eq!(rf.bindings[0].host.as_deref(), Some("myhost"));
        assert_eq!(rf.bindings[0].host_port, 3000);
    }

    // -----------------------------------------------------------------------
    // Edge cases & error paths
    // -----------------------------------------------------------------------

    #[test]
    fn parse_local_forward_whitespace_trimmed() {
        let lf = parse_local_forward("  29876:myrelay  ").unwrap();
        assert_eq!(lf.relay_name, "myrelay");
        assert_eq!(lf.bindings[0].bind_port, 29876);
    }

    #[test]
    fn parse_remote_forward_whitespace_trimmed() {
        let rf = parse_remote_forward("  myrelay:29876  ").unwrap();
        assert_eq!(rf.relay_name, "myrelay");
        assert_eq!(rf.bindings[0].host_port, 29876);
    }

    #[test]
    fn parse_local_forward_udp_lowercase() {
        let lf = parse_local_forward("29876u:myrelay").unwrap();
        assert_eq!(lf.bindings[0].bind_port, -29876);
    }

    #[test]
    fn parse_remote_forward_host_port_with_port_name() {
        let rf =
            parse_remote_forward("myrelay:pname/myhost:8080").unwrap();
        assert_eq!(rf.bindings[0].host.as_deref(), Some("myhost"));
        assert_eq!(rf.bindings[0].host_port, 8080);
        assert_eq!(rf.bindings[0].port_name.as_deref(), Some("pname"));
    }

    #[test]
    fn parse_http_forward_multiple_bindings() {
        let rf = parse_remote_http_forward(
            "myrelay:http/host1:8080;http/host2:9090",
        )
        .unwrap();
        assert_eq!(rf.bindings.len(), 2);
        assert_eq!(rf.bindings[0].host.as_deref(), Some("host1"));
        assert_eq!(rf.bindings[0].host_port, 8080);
        assert_eq!(rf.bindings[1].host.as_deref(), Some("host2"));
        assert_eq!(rf.bindings[1].host_port, 9090);
    }

    #[test]
    fn default_config_validates() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parse_http_binding_https_not_insecure_by_default() {
        let binding = parse_http_binding("https/localhost:443").unwrap();
        assert!(!binding.insecure, "HTTPS bindings should not default to insecure");
        assert_eq!(binding.host.as_deref(), Some("localhost"));
        assert_eq!(binding.host_port, 443);
        assert!(binding.http);
    }

    #[test]
    fn parse_http_binding_http_not_insecure() {
        let binding = parse_http_binding("http/localhost:80").unwrap();
        assert!(!binding.insecure, "HTTP bindings should not be insecure");
        assert_eq!(binding.host.as_deref(), Some("localhost"));
        assert_eq!(binding.host_port, 80);
        assert!(binding.http);
    }

    #[test]
    fn validate_good_address_families() {
        for af in &["any", "inet", "inet6", "Any", "INET", "Inet6"] {
            let config = Config {
                address_family: Some(af.to_string()),
                ..Default::default()
            };
            assert!(
                config.validate().is_ok(),
                "Expected '{af}' to be valid"
            );
        }
    }
}
