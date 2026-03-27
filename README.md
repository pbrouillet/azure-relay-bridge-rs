# azure-relay-bridge-rs (azbridge)

A Rust implementation of [Azure Relay Bridge](https://github.com/Azure/azure-relay-bridge) — a CLI tool that creates VPN-less TCP, UDP, HTTP, and Unix socket tunnels between any pair of hosts using [Azure Relay Hybrid Connections](https://learn.microsoft.com/en-us/azure/azure-relay/relay-hybrid-connections-protocol).

Traffic flows over outbound HTTPS (port 443) only — no inbound firewall rules or VPNs required.

## Quick Start

```sh
# Local forward: expose remote-host:3306 on local port 3306 via Azure Relay
azbridge -L 3306:my-relay -x "Endpoint=sb://mynamespace.servicebus.windows.net;..."

# Remote forward: accept relay connections and forward to localhost:8080
azbridge -T my-relay:8080 -x "Endpoint=sb://mynamespace.servicebus.windows.net;..."
```

## Supported Tunnel Types

| Mode | Flag | Example |
|------|------|---------|
| **TCP local forward** | `-L` | `-L 3306:my-relay` |
| **TCP remote forward** | `-T` | `-T my-relay:3306` |
| **UDP** | `-L` / `-T` | `-L 5000U:my-relay` (suffix `U`) |
| **HTTP reverse proxy** | `-H` | `-H my-relay:http:localhost:8080` |
| **Unix socket** | `-L` / `-T` | `-L /tmp/sock:my-relay` (Linux/macOS) |

## Installation

### Build from source

Requires [Rust 1.85+](https://rustup.rs/) (edition 2024).

```sh
cargo build --release
# Binary at target/release/azbridge
```

### Docker

```sh
docker build -t azbridge .
docker run azbridge -L 8080:my-relay -x "Endpoint=sb://..."
```

## CLI Usage

```
azbridge [OPTIONS]

Options:
  -L <spec>       Local forward: [bind_address:]port:relay_name[/port_name]
  -T <spec>       Remote forward: relay_name:[host:]port[/port_name]
  -H <spec>       HTTP forward: relay_name:http[s]:host:port[/path]
  -e <endpoint>   Azure Relay endpoint (sb://namespace.servicebus.windows.net)
  -x <string>     Azure Relay connection string
  -K <name>       Shared Access Key name
  -k <key>        Shared Access Key
  -s <sig>        Shared Access Signature token
  -f <file>       Config file path
  -o <key:value>  Override config option
  -g              Gateway ports (bind to 0.0.0.0 instead of 127.0.0.1)
  -b <address>    Bind address for outbound connections
  -v              Increase verbosity (repeat for more: -vv, -vvv)
  -q              Quiet mode (errors only)
  -l <level>      Log level (FATAL, ERROR, WARNING, INFO, VERBOSE, DEBUG1-3)
  -a <file>       Log to file
  -I              Install as Windows service
  -U              Uninstall Windows service
```

## Configuration

azbridge loads configuration from YAML files in this order (later overrides earlier):

1. **Machine config**: `/etc/azbridge/azbridge_config.machine.yml` (Linux) or `%PROGRAMDATA%\Microsoft\AzureBridge\azbridge_config.machine.yml` (Windows)
2. **User config**: `~/.azbridge/azbridge_config.yml`
3. **`-f` file** (overrides user config)
4. **`-o` overrides** (inline YAML key:value)
5. **CLI flags** (highest priority)

### Config file format

```yaml
AzureRelayConnectionString: "Endpoint=sb://mynamespace.servicebus.windows.net;SharedAccessKeyName=...;SharedAccessKey=..."

LocalForward:
  - RelayName: my-database-relay
    Bindings:
      - BindPort: 3306
        PortName: mysql

RemoteForward:
  - RelayName: my-web-relay
    Bindings:
      - Host: localhost
        HostPort: 8080
        PortName: http

# Optional settings
ExitOnForwardFailure: true   # Stop if any bridge fails to start (default: true)
ConnectionAttempts: 1        # Retry count for listener connections (1-10)
ConnectTimeout: 60           # Timeout in seconds for connections (0-120)
AddressFamily: any           # any, inet (IPv4 only), inet6 (IPv6 only)
GatewayPorts: false          # Bind to 0.0.0.0 instead of 127.0.0.1
```

## Authentication

### Shared Access Signature (SAS)

Provide credentials via connection string, CLI flags, or config file:

```sh
# Connection string (contains key name + key)
azbridge -L 3306:relay -x "Endpoint=sb://ns.servicebus.windows.net;SharedAccessKeyName=send;SharedAccessKey=base64key=="

# Separate flags
azbridge -L 3306:relay -e sb://ns.servicebus.windows.net -K send -k base64key==
```

### Azure Active Directory (Entra ID)

When no SAS credentials are provided, azbridge automatically uses `az login` credentials via the Azure Identity SDK:

```sh
az login
azbridge -L 3306:relay -e sb://ns.servicebus.windows.net
```

## Architecture

```
┌──────────────────────────────────────────────────┐
│                   azbridge CLI                    │
│  config · cli · host · logging · service          │
├──────────────────────────────────────────────────┤
│        TCP · UDP · HTTP · Unix Socket             │
│   local forward bridges · remote forwarders       │
│         stream pump · preamble protocol           │
├──────────────────────────────────────────────────┤
│              azure-relay crate                    │
│  client · listener · stream · protocol · auth     │
│    WebSocket · SAS tokens · AAD · connection str  │
└──────────────────────────────────────────────────┘
            │                        ▲
            │ outbound HTTPS/443     │
            ▼                        │
    ┌───────────────────────────────────┐
    │      Azure Relay Service          │
    │    Hybrid Connections endpoint    │
    └───────────────────────────────────┘
```

## Development

```sh
cargo build              # Debug build
cargo test --workspace   # Run all tests (268 passing + 30 ignored)
cargo clippy --workspace # Lint
cargo fmt --check        # Format check
```

### Running live integration tests

The 30 ignored tests require a live Azure Relay namespace:

```sh
# Set connection string and run ignored tests
$env:RELAY_CONNECTION_STRING = "Endpoint=sb://...;SharedAccessKeyName=...;SharedAccessKey=..."
cargo test -p azure-relay --ignored
```

Prerequisites: create two Hybrid Connections named `authenticated` and `unauthenticated` in the namespace.

## Compatibility

This is a wire-compatible port of the [C# azbridge](https://github.com/Azure/azure-relay-bridge). Key compatibility points:

- Preamble protocol is byte-identical to C#
- `ServiceBusAuthorization` header (not query param)
- SAS audience normalization (`sb://` → `http://`, lowercase, trailing `/`)
- UDP datagram framing (2-byte big-endian length prefix)
- Config file format (YAML with PascalCase fields)
- CLI flags match upstream

## License

MIT
