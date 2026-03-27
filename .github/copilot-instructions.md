# Copilot Instructions for azure-relay-bridge-rs

## What This Project Is

A Rust rewrite of [Azure/azure-relay-bridge](https://github.com/Azure/azure-relay-bridge) (`azbridge`), a CLI tool that creates VPN-less TCP, UDP, HTTP, and Unix Socket tunnels between any pair of hosts using Azure Relay Hybrid Connections. Traffic flows over outbound HTTPS (port 443) only — no inbound firewall rules or VPNs required.

The upstream C# implementation is the authoritative reference for behavior, CLI interface, and configuration format. The [azure-relay-dotnet](https://github.com/Azure/azure-relay-dotnet) SDK is the reference for the Hybrid Connections protocol.

## Reference Repos

- **Upstream C# azbridge**: `C:\src\OSS\azure-relay-bridge` (local clone) or [Azure/azure-relay-bridge](https://github.com/Azure/azure-relay-bridge) on GitHub (use MCP tools if local path is inaccessible)
- **Azure Relay .NET SDK**: [Azure/azure-relay-dotnet](https://github.com/Azure/azure-relay-dotnet)

## Agent Behavior

- When user says "proceed", "go for", or "implement" — make the code changes directly. Do not just read and display the current code.
- Wire compatibility with the upstream C# implementation is the top priority. When implementing protocol features, always verify byte-level and behavioral parity with the C# SDK. Key areas:
  - Auth: `ServiceBusAuthorization` HTTP header (not `sb-hc-token` query param)
  - Audience normalization: `sb://` → `http://`, lowercase, trailing `/`
  - Preamble error codes: `0`/`1`/`255` (not `1`/`2`)

## Build & Test

```sh
cargo build              # debug build
cargo build --release    # release build
cargo test --workspace   # run all tests
cargo test -p azure-relay                 # test just the relay crate
cargo test -p azure-relay test_name       # run a single test by name
cargo clippy --workspace # lint (must be warning-free)
cargo fmt --check        # check formatting
```

Uses Rust edition 2024.

## Workspace Structure

This is a Cargo workspace with two crates:

- **`crates/azure-relay/`** — Standalone Azure Relay Hybrid Connections client library. Implements the [WebSocket-based protocol](https://learn.microsoft.com/en-us/azure/azure-relay/relay-hybrid-connections-protocol) from scratch. Publishable independently.
- **Root crate (`src/`)** — The `azbridge` CLI binary, built on top of `azure-relay`.

## Architecture

### azure-relay crate

| Module | Role |
|---|---|
| `connection_string` | `RelayConnectionStringBuilder` — parses/builds Azure Relay connection strings |
| `token_provider` | `TokenProvider` trait, `SharedAccessSignatureTokenProvider` (HMAC-SHA256), `SharedAccessSignatureToken` |
| `protocol` | JSON control channel message types (`AcceptCommand`, `RequestCommand`, `ResponseCommand`, `RenewTokenCommand`), URI construction utilities |
| `error` | `RelayError` enum with typed variants |
| `client` | `HybridConnectionClient` — WebSocket sender |
| `listener` | `HybridConnectionListener` — WebSocket listener with control channel |
| `stream` | `HybridConnectionStream` — AsyncRead/AsyncWrite over WebSocket |
| `http` | HTTP request/response handling over relay |

### azbridge CLI

| Component | Role |
|---|---|
| `config` | Config model (`Config`, `LocalForward`, `RemoteForward`, bindings), YAML serde, validation, merge, expression parsers (`-L`, `-T`, `-R`, `-H`) |
| `cli` | `clap` derive CLI matching upstream `azbridge` flag structure |
| `config_loader` | Multi-layer config loading (machine → user → file → `-o` → CLI flags) |
| `logging` | `tracing` setup with log-level mapping (`QUIET`→`DEBUG3`), console and file output via `tracing-appender` |
| `preamble` | Wire protocol — version/mode/port_name preamble (byte-compatible with C#) |
| `stream_pump` | Bidirectional 64KB-buffered async copy with half-close semantics |
| `tcp` | `TcpLocalForwardBridge` + `TcpRemoteForwarder` |
| `socket` | Unix socket bridges (`cfg(unix)`) |
| `udp` | UDP datagram framing (2-byte BE length prefix) + bridge scaffolding |
| `http_forward` | HTTP forwarding config, header filtering, relay prefix stripping |
| `host` | Top-level `Host` orchestrator with start/stop/shutdown |

### Wire Protocol

The azbridge preamble exchanged over each relay connection (must be byte-identical to C#):
```
Sender → Listener: [version_major=1, version_minor=0, mode(0=stream/1=dgram), port_name_len, port_name_bytes...]
Listener → Sender: [version_major, version_minor, mode_echo_or_error]
```

UDP uses 2-byte big-endian length-prefixed datagram framing over the stream.

## Key Conventions

- Use `clap` (derive style) for CLI argument parsing to match the upstream flag structure.
- Use `tokio` as the async runtime — the tool is fundamentally I/O-bound with many concurrent connections.
- Use `azure_identity` and `azure_core` crates for AAD authentication.
- Configuration file format is YAML with PascalCase field names, matching the upstream `azbridge_config.*.yml` schema. Use `serde` + `serde_yaml`.
- Error handling: `thiserror` for the `azure-relay` library crate, `anyhow` for the `azbridge` binary.
- Unit tests in the `azure-relay` crate mirror the [azure-relay-dotnet](https://github.com/Azure/azure-relay-dotnet) SDK test suite — same inputs, same expected outputs.
- Integration tests requiring a live Azure Relay namespace use the `RELAY_CONNECTION_STRING` environment variable.

## Testing Policy

Every implementation phase must ship with matching, updated unit tests. No module is considered complete until its public API is tested. Follow these rules:

### Coverage requirements

1. **Every public type, trait, and method** must have at least one test exercising its primary behavior.
2. **Every error path** (Result::Err return, panic guard) must have a test triggering it.
3. **Constructor variants** — if a type has `from_connection_string()`, `from_uri()`, and `from_uri_no_auth()`, all three must be tested.
4. **Serialization round-trips** — any type implementing `Serialize`/`Deserialize` must have a round-trip test (construct → serialize → deserialize → assert equal).
5. **Wire protocol compatibility** — any encode/decode logic must be tested with known byte sequences matching the C# implementation's output.

### Test organization

| Test type | Location | When to use |
|---|---|---|
| Inline `#[cfg(test)] mod tests` | Same file as source | Unit tests tightly coupled to private internals, helper functions, validation logic |
| `tests/*.rs` integration tests | `crates/azure-relay/tests/` | Tests that mirror C# SDK test methods by name and exercise the public API |
| Mock relay protocol tests | `crates/azure-relay/src/test_utils.rs` mock server | Tests verifying wire protocol behavior (auth, preamble, rendezvous, control channel). Required for any protocol-level changes. |
| Live integration tests | `tests/integration/` (gated by `#[ignore]` or env var check) | Tests requiring `RELAY_CONNECTION_STRING` and a live Azure Relay namespace |

### Naming convention

- **Inline tests**: descriptive snake_case — `provider_new_rejects_empty_key`, `build_audience_strips_hc_prefix`
- **Integration tests mirroring C#**: match the C# test method name in snake_case — e.g., C# `ConnectionStringBuilderOperationValidation` → `connection_string_builder_operation_validation`
- **Error path tests**: suffix with the expected outcome — `_rejects_empty`, `_fails_on_missing`, `_returns_err`

### Phase gate checklist

Before marking a phase complete, verify:

- [ ] `cargo test --workspace` passes with no failures
- [ ] `cargo clippy --workspace` produces zero warnings
- [ ] Every new public API item has at least one test
- [ ] Every new error variant that can be triggered has a test triggering it
- [ ] If the module has a C# SDK counterpart, the corresponding C# test cases are ported

