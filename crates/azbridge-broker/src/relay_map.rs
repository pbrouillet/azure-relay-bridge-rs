use azbridge_broker_api::{
    RelayEndpointDirection as Direction, RelayEndpointProtocol as Protocol, RelayMapEndpoint,
};
use azbridge_config::Config;
use serde_yaml::{Mapping, Value};

const MAX_ENDPOINTS: usize = 4096;
const MAX_FIELD: usize = 1024;

pub(crate) fn extract_endpoints(yaml: &str) -> Result<Vec<RelayMapEndpoint>, ()> {
    let config = crate::profiles::parse_managed_config(yaml).map_err(|_| ())?;
    let mut endpoints = from_config(&config, false)?;
    endpoints.extend(disabled_endpoints(yaml, &config));
    if endpoints.len() > MAX_ENDPOINTS {
        return Err(());
    }
    Ok(endpoints)
}

fn effective_name(name: &Option<String>, port: i32, local_socket: bool) -> String {
    name.clone().unwrap_or_else(|| {
        if local_socket {
            "sock".into()
        } else if port < 0 {
            format!("{}U", port.unsigned_abs())
        } else {
            port.to_string()
        }
    })
}

fn from_config(config: &Config, disabled: bool) -> Result<Vec<RelayMapEndpoint>, ()> {
    let mut endpoints = Vec::new();
    let prefix = if disabled { "disabled-" } else { "" };
    for (group, forward) in config.local_forward.iter().enumerate() {
        for (index, binding) in forward.bindings.iter().enumerate() {
            let socket = binding.bind_local_socket.is_some();
            endpoints.push(RelayMapEndpoint {
                id: format!("{prefix}local-{group}-{index}"),
                forward_group: format!("{prefix}local-{group}"),
                relay_name: forward.relay_name.clone(),
                direction: Direction::Listener,
                protocol: if socket {
                    Protocol::Socket
                } else if binding.bind_port < 0 {
                    Protocol::Udp
                } else {
                    Protocol::Tcp
                },
                host: if socket {
                    None
                } else {
                    Some(binding.bind_address.clone().unwrap_or_else(|| {
                        if config.gateway_ports == Some(true) {
                            "0.0.0.0"
                        } else {
                            "127.0.0.1"
                        }
                        .into()
                    }))
                },
                port: u16::try_from(binding.bind_port.unsigned_abs()).map_err(|_| ())?,
                socket: binding.bind_local_socket.clone(),
                port_name: effective_name(&binding.port_name, binding.bind_port, socket),
                explicit_port_name: binding.port_name.is_some(),
                disabled,
                path: None,
            });
        }
    }
    for (group, forward) in config.remote_forward.iter().enumerate() {
        for (index, binding) in forward.bindings.iter().enumerate() {
            endpoints.push(RelayMapEndpoint {
                id: format!("{prefix}remote-{group}-{index}"),
                forward_group: format!("{prefix}remote-{group}"),
                relay_name: forward.relay_name.clone(),
                direction: Direction::Destination,
                protocol: if binding.http {
                    Protocol::Http
                } else if binding.local_socket.is_some() {
                    Protocol::Socket
                } else if binding.host_port < 0 {
                    Protocol::Udp
                } else {
                    Protocol::Tcp
                },
                host: if binding.local_socket.is_some() {
                    None
                } else {
                    Some(binding.host.clone().unwrap_or_else(|| "localhost".into()))
                },
                port: u16::try_from(binding.host_port.unsigned_abs()).map_err(|_| ())?,
                socket: binding.local_socket.clone(),
                port_name: effective_name(&binding.port_name, binding.host_port, false),
                explicit_port_name: binding.port_name.is_some(),
                disabled,
                path: binding.path.clone(),
            });
        }
    }
    if endpoints.len() > MAX_ENDPOINTS
        || endpoints.iter().any(|endpoint| {
            [&endpoint.relay_name, &endpoint.port_name]
                .into_iter()
                .chain(endpoint.host.iter())
                .chain(endpoint.socket.iter())
                .chain(endpoint.path.iter())
                .any(|field| field.len() > MAX_FIELD || field.chars().any(char::is_control))
        })
    {
        return Err(());
    }
    Ok(endpoints)
}

fn scalar_field(line: &str) -> Option<(&str, Value)> {
    let (key, raw) = line.split_once(':')?;
    let key = key.trim();
    if ![
        "RelayName",
        "BindAddress",
        "BindPort",
        "BindLocalSocket",
        "HostName",
        "Host",
        "HostPort",
        "LocalSocket",
        "PortName",
        "Http",
        "Insecure",
        "Path",
    ]
    .contains(&key)
        || raw.len() > MAX_FIELD
    {
        return None;
    }
    let value: Value = serde_yaml::from_str(raw.trim()).ok()?;
    if matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
        Some((key, value))
    } else {
        None
    }
}

// Best effort only: recognize scalar fields in commented list entries, never uncomment YAML.
fn disabled_endpoints(yaml: &str, active: &Config) -> Vec<RelayMapEndpoint> {
    let only_direction = match (
        active.local_forward.is_empty(),
        active.remote_forward.is_empty(),
    ) {
        (false, true) => Some("LocalForward"),
        (true, false) => Some("RemoteForward"),
        _ => None,
    };
    let relays: std::collections::HashSet<_> = active
        .local_forward
        .iter()
        .map(|f| f.relay_name.as_str())
        .chain(active.remote_forward.iter().map(|f| f.relay_name.as_str()))
        .collect();
    let only_relay = if relays.len() == 1 {
        relays.iter().next().copied()
    } else {
        None
    };
    let mut section = only_direction;
    let mut relay = only_relay.map(str::to_owned);
    let mut entry = Mapping::new();
    let mut entries = Vec::new();
    let mut flush = |fields: &mut Mapping, section: Option<&str>, relay: &Option<String>| {
        if fields.is_empty() {
            return;
        }
        let fields = std::mem::take(fields);
        let local = ["BindPort", "BindAddress", "BindLocalSocket"]
            .iter()
            .any(|key| fields.contains_key(Value::from(*key)));
        let remote = ["HostPort", "Host", "LocalSocket", "Http"]
            .iter()
            .any(|key| fields.contains_key(Value::from(*key)));
        let direction = match (local, remote) {
            (true, false) => Some("LocalForward"),
            (false, true) => Some("RemoteForward"),
            (false, false) => section,
            _ => None,
        };
        let Some(direction) = direction else { return };
        let mut fields = fields;
        if !fields.contains_key(Value::from("RelayName")) {
            let Some(relay) = relay else { return };
            fields.insert(Value::from("RelayName"), Value::from(relay.clone()));
        }
        let mut root = Mapping::new();
        root.insert(
            Value::from(direction),
            Value::Sequence(vec![Value::Mapping(fields)]),
        );
        if let Some(gateway_ports) = active.gateway_ports {
            root.insert(Value::from("GatewayPorts"), Value::Bool(gateway_ports));
        }
        if let Ok(yaml) = serde_yaml::to_string(&root)
            && let Ok(config) = crate::profiles::parse_managed_config(&yaml)
            && let Ok(mut endpoints) = from_config(&config, true)
        {
            for endpoint in &mut endpoints {
                endpoint.id = format!("disabled-{}", entries.len());
                endpoint.forward_group = endpoint.id.clone();
            }
            entries.extend(endpoints);
        }
    };
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let comment = comment.trim();
            if ["LocalForward:", "RemoteForward:"].contains(&comment) {
                flush(&mut entry, section, &relay);
                section = Some(if comment == "LocalForward:" {
                    "LocalForward"
                } else {
                    "RemoteForward"
                });
            } else if let Some(start) = comment.strip_prefix("- ") {
                flush(&mut entry, section, &relay);
                if let Some((key, value)) = scalar_field(start) {
                    entry.insert(Value::from(key), value);
                }
            } else if !entry.is_empty()
                && let Some((key, value)) = scalar_field(comment)
            {
                entry.insert(Value::from(key), value);
            }
        } else {
            flush(&mut entry, section, &relay);
            if ["LocalForward:", "RemoteForward:"].contains(&line) {
                section = Some(if line == "LocalForward:" {
                    "LocalForward"
                } else {
                    "RemoteForward"
                });
                relay = only_relay.map(str::to_owned);
            } else if let Some((key, Value::String(value))) =
                scalar_field(line.trim_start_matches("- "))
                && key == "RelayName"
            {
                relay = Some(value);
            }
        }
    }
    flush(&mut entry, section, &relay);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_extraction_parity_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../web/src/relays/fixtures/parity.json"))
                .unwrap();
        assert_eq!(
            serde_json::to_value(extract_endpoints(fixture["yaml"].as_str().unwrap()).unwrap())
                .unwrap(),
            fixture["endpoints"]
        );
    }

    #[test]
    fn extracts_runtime_defaults_and_normalizes_shorthand() {
        let endpoints = extract_endpoints("GatewayPorts: true\nLocalForward:\n- RelayName: r\n  BindPort: -53\n- RelayName: r\n  BindLocalSocket: /tmp/a\nRemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 443\n    Http: true\n    Path: /api\n  - LocalSocket: /tmp/b\n").unwrap();
        assert_eq!(endpoints.len(), 4);
        assert_eq!(endpoints[0].port_name, "53U");
        assert_eq!(endpoints[0].protocol, Protocol::Udp);
        assert_eq!(endpoints[0].host.as_deref(), Some("0.0.0.0"));
        assert_eq!(endpoints[1].port_name, "sock");
        assert_eq!(endpoints[1].protocol, Protocol::Socket);
        assert_eq!(endpoints[2].protocol, Protocol::Http);
        assert_eq!(endpoints[2].host.as_deref(), Some("localhost"));
        assert_eq!(endpoints[2].path.as_deref(), Some("/api"));
        assert_eq!(endpoints[3].port_name, "0");
        assert!(!endpoints[3].explicit_port_name);
    }

    #[test]
    fn disabled_entries_are_advisory_and_ignore_unrecognized_comments() {
        let yaml = "LocalForward:\n- RelayName: r\n  BindPort: 80\n# Notes are not entries\n# - RelayName: r\n#   BindPort: 8080\n#   PortName: 'old-name'\n# - RelayName: r\n#   BindPort: nope\n# - an ordinary list item\n# - Host: localhost\n#   HostPort: 81\n";
        let endpoints = extract_endpoints(yaml).unwrap();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0].host.as_deref(), Some("127.0.0.1"));
        assert_eq!(endpoints[1].port_name, "old-name");
        assert!(endpoints[1].disabled);
        assert_eq!(endpoints[2].direction, Direction::Destination);
        assert!(endpoints[2].disabled);
    }

    #[test]
    fn invalid_or_oversized_fields_return_err_without_values() {
        for yaml in [
            "LocalForward: [",
            "LocalForward:\n- RelayName: r\n  BindPort: 999999",
            "LocalForward:\n- RelayName: r\n  BindPort: 80\n  ConnectionString: secret",
            &format!(
                "LocalForward:\n- RelayName: r\n  BindPort: 80\n  PortName: {}",
                "x".repeat(MAX_FIELD + 1)
            ),
            &format!(
                "LocalForward:\n- RelayName: r\n  Bindings:\n{}",
                "  - BindPort: 80\n".repeat(MAX_ENDPOINTS + 1)
            ),
        ] {
            assert!(extract_endpoints(yaml).is_err());
        }
    }
}
