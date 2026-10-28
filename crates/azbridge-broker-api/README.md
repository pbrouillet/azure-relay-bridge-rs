# azbridge broker API contract

Shared Rust DTOs and generated TypeScript bindings. There are no HTTP clients or
server handlers in this crate.

## Regeneration and verification

From the workspace root:

```console
cargo run -p azbridge-broker-api --example generate_types
cargo test -p azbridge-broker-api
cargo test -p azbridge-config --features typescript
```

The generator always resolves `web/src/api/generated/index.ts` relative to this
crate's manifest, not the current directory. Tests compare the checked-in
bindings without writing files. Import browser types from `./generated`.

## Wire conventions

- All DTO fields and enum values are camelCase. `EnrollmentPollResponse` is a
  discriminated union tagged by `status`: `pending`, `slowDown`, `authorized`,
  `expired`, or `denied`. Authorized fields are flattened next to `status`.
- `GatewayConfigResponse.config` preserves the existing **PascalCase**
  `azbridge-config` serialization, including typed local/remote bindings.
  Shorthand convenience fields on forwards are optional input-only fields.
- IDs are UUID strings; enrollment accepts workspace/profile IDs **or names**.
  GitHub IDs, revisions, Unix-second timestamps, and second-based durations are
  Rust `u64` and JSON numbers. Browser consumers must use values within
  `Number.MAX_SAFE_INTEGER`; no conversion to JSON strings or `bigint` occurs.
- Optional fields deserialize from absent or null values and omit `None` on
  serialization. Config's own existing serialization is unchanged.
- `Role` ordering is `reader < operator < admin`; `permits(required)` implements
  that comparison. Platform-admin status is separate from workspace roles.
- `MeResponse.workspaces` is `Workspace[]`; `Member` contains `user` and `role`.
  `AuditEntry.actor` is a string containing the GitHub ID or gateway ID.
- Credentials, SAS tokens, enrollment responses, session/CSRF responses, and
  gateway configs deliberately do not implement `Debug`.

## DTO mapping

| Operation | Request | Response |
| --- | --- | --- |
| Current browser user | — | `MeResponse` |
| `POST /auth/github/device/start` | — | `DeviceLoginStartResponse` |
| `POST /auth/github/device/poll` | — | `DeviceLoginPollResponse` |
| Create workspace | `CreateWorkspaceRequest` | `Workspace` |
| Add/update member | `CreateMemberRequest` | `Member` |
| Create connection | `CreateConnectionRequest` | `Connection` |
| Create profile | `CreateProfileRequest` | `Profile` |
| Create revision | `CreateRevisionRequest` | `ConfigRevision` |
| Publish/rollback revision | `PublishRevisionRequest` | `Profile` |
| Pre-create gateway | `CreateGatewayRequest` | `CreateGatewayResponse` |
| `POST /api/v1/enrollments` | `EnrollmentStartRequest` | `EnrollmentStartResponse` |
| `POST /api/v1/enrollments/{id}/poll` | — | `EnrollmentPollResponse` |
| `GET /api/v1/gateway/config` | — | `GatewayConfigResponse` |
| `POST /api/v1/gateway/relay-tokens` | `RelayTokenRequest` | `RelayTokenResponse` |
| `POST /api/v1/gateway/status` | `GatewayStatusRequest` | — |
| Validation | `CreateRevisionRequest` | `ValidationResponse` |
| Errors | — | `ErrorResponse` |
| Health | — | `HealthResponse` |

Collection responses use arrays of the domain types; no collection wrapper is
introduced. Browser authentication uses `/auth/github/login`,
`/auth/github/callback`, device-code sign-in, and `POST /auth/logout`; gateway self-revocation uses
`POST /api/v1/gateway/logout`. These paths are documented here for integration,
not registered by this DTO-only crate.

Device login starts with `X-CSRF-Token: device-login`. Subsequent polls require
the random `csrfToken` from the start response and its browser-bound HttpOnly
challenge cookie. Poll statuses are `pending`, `slowDown` (both carry `interval`
in seconds), `authorized`, `expired`, or `denied`. Authorization sets a normal
broker session cookie; it does not return a GitHub token or gateway credential.
