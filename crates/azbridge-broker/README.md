# azbridge workspace broker

The broker is the control plane for managed azbridge gateways. One process
serves the Rust HTTP API and the built React administration UI from the same
origin. Azure Relay carries tunnel traffic; the broker is not a data-plane proxy.
This MVP uses one broker replica, Azure Table metadata and immutable application
configuration revisions in a private Blob container. Each workspace gets a
server-generated **separate Relay namespace** in a dedicated resource group.
Workspace identity is a UUID, not its display name or Azure namespace name.

## Production startup

Production is the default. It requires the following process environment
settings and a built SPA; invalid or missing settings stop startup rather than
falling back to development mode. Do not put secrets on command lines or print
environment dumps.

| Setting | Required value |
| --- | --- |
| `PUBLIC_URL` | Externally reachable HTTPS origin, no credentials, subpath, query or fragment |
| `LISTEN_ADDR` | Optional socket address; defaults to `0.0.0.0:8080` |
| `GITHUB_CLIENT_ID` | GitHub App OAuth client ID, **not** numeric App ID |
| `GITHUB_CLIENT_SECRET` | GitHub App client secret, not private key/webhook secret |
| `COOKIE_KEY` | Standard base64 encoding of exactly 32 random bytes; retain across restarts |
| `BOOTSTRAP_ADMIN_IDS` | Comma-separated, nonzero numeric GitHub user IDs |
| `STORAGE_ACCOUNT` | Azure Storage account name (3–24 lowercase letters/digits), not a connection string |
| `AZURE_SUBSCRIPTION_ID` | Subscription UUID containing the dedicated Relay resource group |
| `RELAY_RESOURCE_GROUP` | Resource group reserved for broker-managed Relay namespaces |
| `ALLOWED_LOCATIONS` | Comma-separated lowercase Azure location codes allowed for workspace creation |
| `AZURE_CLIENT_ID` | Optional user-assigned managed identity client UUID; set for the deployed identity |
| `STATIC_DIR` | Optional path containing `index.html`; defaults to `web/dist` |

Names are unprefixed: `BROKER_PUBLIC_URL` is a deployment output, whereas the
runtime setting is `PUBLIC_URL`. The deployment uses managed identity for Azure
resource management and Table/Blob access, not storage account keys. It grants
Azure Relay Owner only on the dedicated Relay resource group, plus data roles
on broker storage; see the [deployment guide](../../docs/broker-deployment.md)
for the full identity, Key Vault, ACR and resource contract.

Build the SPA before a direct production start:

```powershell
npm --prefix web ci
npm --prefix web run build
# Inject the required settings securely into this process first.
cargo run --locked -p azbridge-broker
```

Settings are read at startup. After changing them, restart/redeploy the broker.
Keep the cookie key stable; rotating it invalidates browser sessions. A valid
`ALLOWED_LOCATIONS` entry is not proof that Azure supports that region, capacity,
quota or your subscription. Production storage and resource calls fail closed;
no in-memory fallback is enabled.

## Local development only

```powershell
cargo run --locked -p azbridge-broker -- --dev
# In a second shell:
npm --prefix web run dev
```

`--dev` is an explicit command-line switch, never an environment fallback.
Both the listen address and public URL must be loopback; defaults are
`127.0.0.1:8080` and `http://127.0.0.1:8080/`. No cloud settings are required.
Development uses in-memory stores, a simulated Relay manager and a local
administrator identity (numeric ID 1) via `POST /auth/dev`; it does not establish
real Azure tunnels. State is lost on restart. `/auth/dev` returns not found in
production. The SPA shows development sign-in only when `/healthz` explicitly
reports development mode. Never expose development mode via a public reverse
proxy, port forward or container published on a public interface.

The [web guide](../../web/README.md) describes the Vite proxy. Production serves
the built SPA from the broker itself; Vite preview is not a production server.

## GitHub registration and admission

Register a **GitHub App** for each environment, using its OAuth user
authorization flow for browser login. Set the production homepage to the broker
origin and the exact user authorization callback to
`https://<broker-host>/auth/github/callback`. Enable **Device Flow** in the app's
settings for browser device-code sign-in and CLI gateway enrollment. A plain OAuth App is not the documented
deployment registration. Repository write access, organization membership
permissions, webhooks and an installation private key are not required for this
identity-only integration. See the deployment guide's registration checklist.

GitHub authenticates the person; it does **not** grant workspace admission.
Bootstrap IDs establish platform administrators. Workspace administrators add a
GitHub login in the UI; the broker resolves it and stores the **numeric user ID**
as the allowlist key. Renaming a login is not a role change. An authenticated
GitHub account without platform administration or workspace membership is denied.
Membership and role are checked server-side, not trusted from browser controls.

| Role | Workspace capabilities |
| --- | --- |
| `reader` | Read workspace, members, connections, profiles/revisions, gateway metadata and audit |
| `operator` | Reader capabilities plus create/delete profiles, validate/create/publish revisions, create/enroll/revoke gateways |
| `admin` | Operator capabilities plus membership, connection lifecycle/key rotation and workspace deletion |
| Platform administrator | Create workspaces and administer every workspace; not confined to an individual membership |

Device enrollment requires at least operator access to the selected workspace
and a published profile. A gateway's bearer credential is independent of its
enrolling user's session: removing that user's membership is not a substitute
for revoking their issued gateway credentials.

## Multiple workspaces: first-run workflow

1. Deploy/configure the broker, then sign in as a bootstrap platform
   administrator. Resolve bootstrap users by numeric ID before deployment.
2. Create a workspace with a name and allowed Azure region. Wait for `ready`;
   provisioning is asynchronous. Repeat for each isolation boundary. Namespace
   names are broker-generated; clients cannot supply arbitrary Azure resource
   IDs or point a workspace at an existing namespace.
3. Within each workspace, add members with least-privilege roles. A person may
   belong to multiple workspaces with different roles. Platform administrators
   can access all workspaces, so they remain trusted across those boundaries.
4. Create a connection and wait for `ready`. Create a profile, add YAML referring
   only to that workspace's ready connection names, validate it, then publish the
   revision. Revision creation is not publication. Publication checks the
   expected current revision to reject stale concurrent updates.
5. Enroll gateways against that workspace/profile, or create a gateway in the UI
   for unattended use. Copy its one-time credential directly into a secure secret
   store; it cannot be retrieved later. Do not share credentials between hosts.
6. Start the gateway with plain `azbridge`. Use the UI's reported
   `appliedRevision` to distinguish a successful startup from an assigned
   revision. Restart gateways after publication; the MVP does not hot-reload.
7. Revoke unused credentials and review audit entries. Workspace deletion removes
   its managed namespace/resources asynchronously; do not reuse deleted metadata
   as an authorization record.

### Deleting profiles and workspaces

Operators and administrators can delete a profile with the confirmed **Delete
profile** action, or `DELETE /api/v1/workspaces/{workspace}/profiles/{profile}`.
The endpoint requires the usual session, CSRF token and workspace authorization.
Non-revoked gateways assigned to the profile block deletion with `profile_in_use`
(409); revoke them in **Gateways** first. Unpublished profiles and unused published
profiles can both be deleted. Immutable revision blobs, revision metadata and audit
entries remain as inaccessible history, not a recoverable profile. A new profile
with the same name receives a new ID. Connections are not deleted with a profile.

Workspace deletion deliberately does not cascade through connections. A
`workspace_not_empty` (409) response explains the required cleanup; **Manage
connections** opens it from the workspace list. Published profiles referencing
a connection block that connection's deletion with `connection_in_use` (409).
Revoke affected gateways, delete or update those profiles, then delete the
connections and wait for deletion to finish before retrying workspace deletion.
Already established tunnels may remain open after gateway revocation.

An example profile for a local database client (create `database` first):

```yaml
LocalForward:
  - RelayName: database
    Bindings:
      - BindPort: 15432
        PortName: postgres
```

The corresponding remote gateway needs its own published profile referencing the
same connection with `RemoteForward` and the target host/port. Managed profiles
reject embedded Azure credentials/endpoints, per-forward connection strings,
`NoAuthentication: true`, log configuration and unknown fields. The broker
provides the workspace Relay endpoint separately. TCP, UDP, HTTP and supported
Unix socket targets retain their normal azbridge configuration semantics.

The raw YAML credential-text guard includes **comments**, case-insensitively:
Relay key/SAS names, SAS `sig=`, connection string/endpoint syntax and Relay
authorization token/header names are forbidden anywhere in managed YAML.
Errors do not echo rejected values. This check runs on validation, revision
creation, publication and every published-config load. It is a syntax guard,
not a general-purpose detector for arbitrary secrets. Legacy published revisions
with commented credentials fail closed; save and explicitly publish a cleaned
revision before restarting gateways.

To migrate standalone files, operators can use the browser's
**Profiles > Import configs**, or **Import standalone config** in an editor.
Conversion strips credentials/endpoints/logging locally, before any API request.
The wizard can ask admins to create missing connections and waits for Ready,
then validates and saves revisions without publishing. All Relay references
switch to this workspace's namespace. See the [import guide](../../web/README.md#import-standalone-configurations)
for comment handling, pairing warnings, partial retries and private-file local checks.

### End-to-end relay map

`GET /api/v1/workspaces/{workspace}/relay-map` requires workspace **Reader**
access (or higher). It returns `RelayMapResponse`: workspace `connections` with
their provisioning states and `profiles`, each containing the profile metadata,
latest saved `revision`, extracted `endpoints`, and an optional value-free `error`.
Empty profiles have no revision. Revision metadata, not reserved revision counters,
determines the latest successfully saved snapshot; failed uploads may leave gaps.

Extraction shares the managed configuration parser and normalization used for
gateway validation but does not require connections to be Ready. Endpoints include
relay name, listener/destination direction, protocol, effective PortName, whether
the name was explicit, host/port or socket, optional HTTP path, and a disabled flag.
Commented scalar list entries are detected best effort; they never become runnable
bindings. Metadata fields are limited to 1 KiB and each profile to 4096 endpoints.
The response does not include raw YAML, connection strings, keys, or SAS tokens.
A missing or invalid saved snapshot is isolated to its profile; workspace metadata
storage failures remain HTTP errors.

The browser uses this read-only endpoint in import review and the **Relays** tab.
Client and server profiles remain separate for host deployment, while the combined
map labels draft/published revisions, missing sides, incompatible modes, direct
HTTP destinations, diagnosis categories and rename hints. Exact matching and the per-server-group
numeric single-destination fallback mirror the runtime. The endpoint itself is
read-only. The browser's opt-in **Reconcile** action uses only the existing
revision GET, validate and create-revision APIs, with Operator access and CSRF,
and saves unpublished drafts. Nothing publishes or alters connections,
enrollments or running gateways.

Table storage holds `Workspaces`, `Members`, `UserWorkspaces`, `PlatformAdmins`,
`Connections`, `Profiles`, `Gateways`, `Enrollments`, `Audit` and
`RevisionMetadata`. The `configs` Blob container holds revision content.
Membership rows are authoritative; the user-to-workspace index is a lookup, not
authorization. Workspace namespaces and partitioning are logical isolation
inside one trusted control plane, not separate Azure tenants, broker processes
or storage accounts. Back up both metadata and revision blobs coherently.

## Security and operational limits

- **Browser sessions and CSRF:** Browser OAuth uses state and PKCE. The broker
  consumes the GitHub token to resolve identity and revokes it; it is not a
  stored Azure credential. Broker sessions expire after eight hours and use
  encrypted HttpOnly, SameSite=Lax cookies (Secure in production). Mutations,
  including logout, require `X-CSRF-Token` obtained from `/api/v1/me`; a supplied
  Origin must match the broker origin in production. Browser logout clears the
  cookie; it is not global revocation of a copied session cookie. Membership
  checks still occur on subsequent authorized requests.
- **Browser device sign-in:** The optional device-code button uses the same
  identity, allowlist and session rules as web OAuth. A custom-header barrier
  protects initiation; subsequent polls require a random CSRF token and an
  encrypted, browser-bound HttpOnly challenge cookie. Device codes are encrypted
  in the enrollment table, polling intervals are enforced server-side, and CAS
  prevents replay/concurrent session issuance. Tokens are consumed and revoked
  before a session is set. Codes expire; cancelling the UI stops polling.
  Only authorize codes you requested yourself, never codes sent by another person.
- **Gateway credentials and SAS:** The broker returns a random gateway bearer
  credential once and stores its hash. It is not a GitHub token or namespace
  key. A gateway can retrieve only its assigned published profile and request
  SAS for the relay/right that profile permits (`send` for local forwards,
  `listen` for remote forwards). The broker retains access to Relay keys and
  issues audience-scoped SAS with a one-hour lifetime; keys are never returned
  to gateways. Treat state files, environment credentials and SAS as secrets.
- **Operator reach:** A published profile can bind listeners and reach services
  available from the gateway host, including loopback and Unix sockets.
  Operators controlling that profile therefore influence network reachability
  on every assigned host. Review target hosts, ports, socket paths and bind
  addresses; use OS/network isolation and narrowly scoped memberships. A
  workspace is not a sandbox for untrusted profile authors.
- **Device-code phishing:** Approve only codes displayed by a login you started
  on the intended gateway, for the expected GitHub App and trusted broker.
  Never approve a code sent by chat, email or a support caller. A deceived
  operator may authorize an attacker's gateway against their workspace.
- **Revocation latency:** Gateway revocation blocks later broker calls and SAS
  renewal. Removing a member blocks later user authorization but does not
  automatically revoke previously minted gateways. Already issued SAS may
  remain usable until expiry, and **established tunnels are not forcibly
  terminated**, even on expiry. For urgent containment, stop/isolate the
  gateway or affected service as well; do not assume a UI revoke kills sockets.
- **Startup-only config:** Gateways fetch configuration once at startup, without
  an offline cache or background refresh. Publication does not rebind running
  listeners. `status` reports assignment/connectivity, not process liveness.
  See the [managed gateway guide](../../docs/managed-gateway.md) for selection,
  allowed logging flags, machine enrollment and protected credential files.
- **MVP deployment:** Run one replica. In-process mutation serialization and
  reconciliation are not distributed locking or a high-availability guarantee.
  Storage, Relay and GitHub outages can prevent enrollment/configuration/token
  acquisition. Do not bypass failures with public development authentication,
  namespace keys on gateways or stale credentials.

## Deployment and validation

The [azd deployment guide](../../docs/broker-deployment.md) is the operator
procedure for provisioning, packaging and deploying the container. Read its
permissions, costs, placeholder-to-broker transition and secret handling before
running any `azd provision` or `azd deploy` command. These instructions do not
execute a deployment.

Offline CI runs Rust tests/lints, regenerates TypeScript DTOs and checks
`web/src/api/generated/index.ts` for drift, runs the actual web
lint/TypeScript/Vitest/build commands, compiles both Bicep entry points and
checks infrastructure contracts. A separate job builds `Dockerfile.broker`
without pushing or deploying it. Local offline checks are documented in the
[root development guide](../../README.md#broker-and-web-checks).

No local Docker build is claimed when the Docker daemon is unavailable.
Compilation, mock HTTP/Relay tests and offline contracts do not verify live
Azure resource provisioning, managed-identity permissions, GitHub OAuth/device
authorization, production ingress or real tunnel interoperability. Perform the
deployment guide's acceptance checks only in an explicitly authorized live
environment.
