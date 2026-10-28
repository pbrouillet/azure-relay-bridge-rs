# Managed gateways

Enroll a gateway without distributing Azure Relay namespace keys:

```console
azbridge login --broker https://broker.example --workspace my-workspace --profile my-profile --name my-gateway
azbridge status
azbridge
azbridge logout
```

Login displays the broker's device verification URL and user code, honors polling
intervals (including `slowDown`), and saves only the broker-issued gateway
credential. GitHub access tokens never pass through the CLI. Logout revokes the
credential before removing saved state; if the broker is unavailable it fails
without silently deleting the credential.

The broker must have GitHub App Device Flow enabled, and the approving numeric
GitHub user ID must have at least workspace operator access. Approve only codes
from a login you initiated on the intended host for a trusted broker and app,
never codes sent by someone else. See the
[broker operator guide](../crates/azbridge-broker/README.md) for workspace setup
and the security implications of granting operators access to gateway hosts.

Add `--machine` to `login`, `status`, and `logout` for machine-scoped enrollment.
Machine login requires permission to write the machine configuration directory.
The Windows service uses **only machine state**, not user state or interactive
gateway environment overrides. Foreground startup prefers environment state,
then user state, then machine state.

## Containers and unattended enrollment

Provide both `AZBRIDGE_BROKER_URL` and `AZBRIDGE_GATEWAY_CREDENTIAL` with a
pre-created gateway credential. Missing, empty, or non-Unicode values in either
variable are errors; there is no partial override. Environment credentials are
never copied into a state file. Logout revokes the environment credential but
cannot remove it from the calling shell or deployment configuration.

## Configuration and startup

Managed startup fetches the assigned profile once, failing immediately on broker
errors. There is no offline cache. Restart to apply a newly published revision.
Machine/user YAML files, `AZURE_BRIDGE_CONNECTIONSTRING`, forwarding flags,
credential flags, `-f`, **all `-o` overrides**, network flags (`-g`, `-b`, `-a`),
and `--tui` do not modify managed profiles. Conflicting CLI flags are rejected,
not ignored. Only local logging flags (`-q`, `-v`, `-l`) are allowed.

Local mode and its original configuration precedence are unchanged when no
managed state or environment override exists. TUI is local-only and is rejected
while enrolled.

The gateway reports `appliedRevision` only after local sockets are bound and all
managed remote listeners have established their control channels. Startup or
status-report failures stop the partially started host. `status` shows the
**assigned** revision and checks connectivity; it does not claim that a running
process has applied that revision.

## Credential protection

`gateway.json` is stored beside the existing user or machine configuration file:
Unix user `~/.azbridge`, Unix machine `/etc/azbridge`, Windows user
`%APPDATA%\azbridge`, Windows machine `%PROGRAMDATA%\Microsoft\AzureBridge`.
Replacement files are created exclusively with Unix mode `0600` or a protected
Windows DACL granting only owner, Administrators, and SYSTEM access **before**
the credential is written, then atomically renamed. Symlink/reparse-point paths,
insecure existing file permissions, and unrecognized Windows ACLs are rejected.
Credentials and SAS tokens have no `Debug` implementation and are not printed.

Broker URLs must be HTTPS origins without embedded credentials, paths, query
strings, or fragments. Explicit loopback HTTP is allowed for development.
Authenticated requests never follow redirects.

Each Relay provider is scoped to one exact normalized HC audience and one right:
local forwards request `send`; remote TCP, UDP, HTTP, and Unix socket forwards
request `listen`. The gateway checks returned SAS audience and expiry against the
broker response. No namespace keys or static SAS connection strings are used.
Tokens are cached per relay/right and renewed before the SDK's five-minute
renewal margin; near-expiry tokens and broker errors never fall back to stale
cache entries. Broker token lifetime is at most an hour (with a small clock-skew
allowance); `TokenProvider::get_token`'s requested `valid_for` is advisory because
the broker owns token lifetime. Returned actual expiry drives SDK renewal.
Tokens with five minutes or less remaining are rejected to avoid a busy listener
renewal loop.

For mock Relay development only, a managed Relay endpoint may explicitly use
`ws://` on loopback. Production `sb://` and `wss://` endpoints always use TLS.
Revocation prevents new tokens; already established tunnels are not forcibly
terminated by the gateway.
Already issued SAS may remain usable until expiry. Removing a user's workspace
membership does not automatically revoke credentials previously issued to their
gateways; revoke those credentials separately. Publishing a more restrictive
profile is not live configuration refresh and does not remove running listeners.
