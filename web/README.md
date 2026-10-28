# azbridge admin SPA

React 19 + TypeScript + Vite, with Fluent UI, TanStack Query and a same-origin
broker API. Controls have square corners.

## Branding

The **Offset Span** bracket mark and outlined Space Grotesk wordmark identify
azbridge, with the tagline **Outbound paths. Clear control.** Native Fluent
colors, Segoe UI and square controls remain unchanged. The resolved application
appearance selects light/dark artwork, including when it differs from the OS
preference. The welcome screen uses the clean vector mark and wordmark, not a
generated interpretation of the logo. Empty illustrations are decorative and
paired with role-aware guidance.

Approved master assets, font attribution and generation provenance live in
[`docs/brand`](../docs/brand/README.md). The app ships only the needed files in
`public/brand`, plus root favicons and manifest. Tests enforce matching source
hashes and image byte budgets. Raw Foundry outputs and review boards are not
part of the application build.

```powershell
Set-Location web
npm ci
npm run dev
npm run lint
npm test
npm run build
```

Use Node 22.12+ (Node 24 recommended). Vite proxies `/api`, `/auth`, and `/healthz` to
`http://127.0.0.1:8080`. Run the broker separately; serve `web/dist` from the broker
for production. `vite preview` previews assets only and is not a production broker.

The GitHub button navigates to `/auth/github/login`. Alternatively, choose
**Sign in with a device code**, copy the displayed code, and enter it at GitHub's
device activation page. Keep the broker page open: it waits for authorization,
honors GitHub's polling intervals and reports denial or expiry. Cancel stops
polling. The same workspace/platform-admin access rules apply to both flows.
A local development sign-in
button appears **only** when `GET /healthz` explicitly returns
`{"status":"ok","mode":"development"}`. It calls `POST /auth/dev` without a payload;
only an explicitly enabled loopback-only broker development mode may provide it.
There is no simulated authentication fallback in this application.

Sessions remain in broker-managed HttpOnly cookies. Requests use same-origin
credentials; authenticated mutations, including logout, carry `X-CSRF-Token`
from `/api/v1/me`. Device sign-in starts with a custom-header CSRF barrier, then
uses a random challenge-specific CSRF token and an encrypted HttpOnly cookie.
GitHub device codes and access tokens never reach the SPA. Development login
is the only mutation that does not require a custom CSRF header.
Gateway credentials are held only in component memory, never local/session
storage, URLs, TanStack Query data, or persisted mutation results. Leaving the
gateway tab or dismissing the secret removes it from the page.

Roles are derived by matching the signed-in numeric GitHub ID against workspace
members. Controls default to hidden until membership is known; platform admins
can manage everything. Server authorization remains authoritative.

Canonical list endpoints return arrays. Role updates use
`PUT /api/v1/workspaces/{workspace}/members/{githubUserId}` with `{ "role": "reader" }`.
Profile publication includes `expectedCurrentRevision` (null for an unpublished
profile) for concurrent-publication protection. Timestamps are Unix seconds.
All DTOs are imported from `src/api/generated/index.ts`; regenerate them using
the Rust API crate's generator rather than editing generated files.

The Appearance picker uses a Fluent UI dropdown with keyboard navigation, an
explicitly spaced label, and themed menu options. It persists Light/Dark/System preferences. The UI uses
native Fluent UI `webLightTheme` / `webDarkTheme` tokens, overriding only corner
radii to keep controls square. Monaco inherits the same Fluent colors.
The app also honors `?scoutTheme=dark` or `?scoutTheme=light`.
Workspace regions come from the broker's authenticated allowed-location list.

## Profile Designer and YAML editor

The **Designer** and **YAML** tabs edit a single, unsaved draft. The browser remembers
the last selected tab; YAML is the initial default. Switching tabs never saves or
publishes a revision. Monaco stays mounted while hidden, retaining its undo history.

Designer displays **Local listeners → Relays → Remote destinations**. Add TCP/UDP
listeners or destinations, Unix socket bindings, or HTTP destinations using a Ready
workspace Relay connection. Click a rule or use **Edit** in the keyboard-accessible
**Forwarding rules** table to inspect its fields. Re-target via the inspector's Relay
selector or drag the relay end of an edge onto another Ready relay. Node positions
are automatic and are not written into the configuration. A referenced relay that
is no longer Ready remains visible with a warning.

Large diagrams show four rules per page, with ports and port names on the cards
instead of overlapping edge labels. **Previous rules**/**Next rules** change only
the view; the table always lists every rule, and **Edit** opens its diagram page.
On narrow screens, scroll inside the diagram horizontally rather than shrinking
the cards. **Additional relay in diagram** chooses an unused Ready connection to
show as a drag target. This editing diagram represents the selected profile only;
the workspace **Relays** tab combines client and server profiles into a read-only
end-to-end map. Profiles stay separate for deployment to their respective hosts.
Paging, inspecting and changing the additional relay do not modify the YAML.

Expand **Advanced settings** for global bind/address-family, timeout/retry, keepalive
and forwarding settings. Blank/default values remove the corresponding YAML key.
Use **Undo**/**Redo** in Designer or Monaco's undo commands to undo draft changes.

Designer updates YAML nodes rather than regenerating the configuration. Comments,
key order and quoting are retained on surviving fields. If the YAML serializer would
normalize whitespace or indentation, the first edit requires confirmation; cancel
leaves the exact draft untouched. Adding a second binding to shorthand converts it
to `Bindings`; removing the last binding removes its forwarding entry.

Invalid YAML, unknown/forbidden managed fields, multiple documents, anchors, aliases,
merge keys and explicit tags make Designer read-only, with a reason and an **Open YAML
tab** action. Nothing is silently discarded or flattened. Readers can inspect both
views but cannot modify either. Broker validation remains authoritative, including
Ready connection checks and the requirement for at least one forwarding rule.

Profiles use a lazily loaded Monaco editor with local YAML and editor workers.
Schemas are bundled with the application: drafts are not sent to a CDN,
external schema service or autocomplete service. The embedded schema suggests
managed configuration fields and Ready Relay connection names in the workspace.
Backend validation and authorization remain authoritative.
Monaco is pinned to the 0.52 line for compatibility with the YAML plugin's
worker API; upgrades must pass the real-browser worker tests.

Press **Ctrl+Space** (or use **Autocomplete & snippets**) for completions.
The `azbridge-*` snippets cover TCP/UDP clients and servers, an HTTP backend
and a Unix socket server. Insert a snippet on an empty top-level line;
**Tab** advances through its editable fields. **Ctrl+M** toggles Tab-key
navigation out of Monaco.

**Download YAML** exports the current editor draft, including unsaved changes.
Readers can download but cannot edit or upload. **Upload YAML** accepts UTF-8
`.yaml`/`.yml` files up to 1 MiB and requires confirmation before replacing
the local draft. Uploading does not save or publish anything: validate, save a
new revision, then explicitly publish it as before.

**Delete profile** is available to operators and administrators after confirmation.
It discards unsaved changes and removes access to the profile and its revisions;
stored immutable revisions and audit history are retained. Revoke any assigned
gateways in **Gateways** first. A failed deletion keeps the editor draft intact.
Connections are not deleted automatically. To remove a workspace, use **Manage
connections**, remove references from published profiles (or delete those profiles),
delete its connections and wait for cleanup before confirming **Delete workspace**.
Dependency errors explain the blocking resources rather than only reporting a
generic conflict.

## Import standalone configurations

Operators can migrate existing standalone YAML in either place:

- **Profiles > Import configs** reviews up to 10 files and creates profiles with
  saved, unpublished revisions. Resolve duplicate names by renaming, or explicitly
  choose to save a new revision of an existing profile.
- **Import standalone config** in a profile editor replaces only the unsaved draft
  after confirmation. Cancel leaves the draft and its validation unchanged.

Files must be UTF-8 `.yml`/`.yaml`, up to 1 MiB each. Conversion happens in the
browser: raw files, embedded keys and original endpoint values are never posted to
the broker. Review shows only sanitized YAML, removed-field/comment counts,
source namespace counts, binding counts and warnings. Embedded authentication
and endpoint globals, per-forward `ConnectionString`, `LogLevel` and `LogFileName`
are removed. Set local gateway verbosity with `-v`/`-vvv` instead.

Ordinary comments and field order are preserved; credential-like comment lines
are dropped using YAML comment tokens, not by splitting at `#` inside strings.
Syntax errors, unknown fields, multiple documents, anchors, aliases, merge keys,
explicit tags, invalid managed settings and remaining credential-like values
block import with value-free errors. Upload YAML directs standalone files here
instead of loading credentials into the editor.

Every RelayName maps to the **current workspace namespace**, not the namespace in
the file. Review flags mismatched embedded EntityPaths without displaying their
values. Migrate both hosts together; importing does not alter running standalone
hosts. Hostnames must resolve on the gateway host. GatewayPorts has no effect
without local listeners.

Import review combines selected files with saved workspace profiles, excluding
profiles being replaced or reused. Import both sides together, or import one now
and its counterpart later. A missing client or server is shown as **Incomplete**;
later imports are paired with existing profiles. If the workspace map cannot be
loaded, review explicitly falls back to selected files only. When the selected
files reference different source Relay namespaces or SAS policies, a prominent
warning explains that they could not have connected to each other as standalone
configurations. Only counts are shown and nothing is retained in import metadata,
so the Relays tab cannot repeat this warning later.

The workspace **Relays** tab keeps that combined view available after import, with
**Show incomplete only** and **Open profile** navigation. It uses the latest
successfully saved revision of each profile, labelled **Draft** or **Published**,
not gateway status or only the currently published revision. Readers can inspect
the map. Only the operator **Reconcile** action described below saves revisions;
no map action publishes, enrolls, or modifies a gateway.

Pairing follows exact, case-sensitive PortNames within each relay and server
listener group. Runtime defaults are the port number for TCP, `<absolute-port>U`
for UDP, and `sock` for an unnamed local socket listener. A numeric client name
may fall back to a server group's single non-HTTP destination, as the runtime
does; incompatible TCP/UDP modes are not paired. HTTP destinations are labelled
**HTTP direct** and do not require a local client listener. Multiple server
listener groups are flagged because Relay distributes connections among them.

The table lists every endpoint, profile/revision and status. A paged read-only
diagram shows four routes at a time, with address/port and PortName on the cards;
on narrow screens it scrolls internally instead of shrinking the cards. Commented
list entries are detected best effort and marked **Disabled in source**, never
activated. Possible renames, reused ports and disabled counterparts produce
advisory hints. These checks describe saved configuration, not proof of live
connectivity or of which revision a host runs.

Each relay also shows a **Diagnosis**: one line plus category filters for the
endpoint table. The categories are:

- **Complete**
- **Accepted by some server groups**
- **Likely renamed**: same target host, a commented-out predecessor on the same
  address and port, or names differing only in case
- **Client commented out**: a server destination whose client entry is commented out
- **Client without server**
- **Server without client**
- **HTTP direct**
- **Disabled in source**

The target-host relation follows the common standalone convention: the client binds
the target's hostname (mapped locally) and the server forwards to that host.
Wildcard and loopback addresses are ignored. For example, when a server file still
serves commented-out client entries, the diagnosis reports that it appears to match
an older client layout. Categories are diagnostic only. Exact runtime pairing and
the **Complete** status are unchanged.

Operators can choose **Reconcile** on a relay in the Relays tab, or in import review
once every file is saved. It never preselects a change. It offers:

- **Add missing server destinations** for clients without a server: Host is the
  listener's bind address, PortName is unchanged, and the port follows existing
  same-host pairs on the relay (otherwise the most common destination port), all
  editable. Unix socket listeners must be configured manually.
- **Align likely renamed PortNames**: for each pair, rename the server or the
  client, or leave both unchanged.
- **Use canonical origins**: move a hostname listener to its destination's port
  (usually 443), keeping its BindAddress and PortName. A default PortName (the old
  port number) is written explicitly so pairing does not change.

Selected changes are applied to the reviewed latest saved revision with the
Designer's comment-preserving edits, validated by the broker, and saved as one new
**draft** revision per profile. Nothing is published, and commented-out entries
stay disabled. If a profile changed after the review, nothing is saved; close the
dialog and review again. Duplicate PortNames within a server group are rejected.
A validation failure in one profile does not roll back saves in other profiles.

### Browser sign-in and canonical origins

Web applications build absolute URLs from their own address, usually
`https://<host>/`. That includes `Location` redirects, the OIDC/OAuth
`redirect_uri` registered with the identity provider, form posts back from the
identity provider and SAML assertion destinations. If the browser reaches the
application through a listener at `https://<host>:44305`, every one of those hops
goes to port 443 instead and leaves the tunnel, so sign-in fails.

azbridge does not rewrite traffic to work around this. Tunnels carry the
application's own TLS, so rewriting would require decrypting it on every client
with a locally trusted certificate authority. Even then, the identity provider only
accepts the registered redirect URI and sends the browser to it directly, never
through azbridge, and SAML destinations are signed.

Instead, keep the browser on the application's real origin. Give each hostname its
own loopback address and bind its listener on the destination's port, as upstream
azbridge documents. The Relays tab flags **Browser origin differs** when a hostname
listener's port does not match its paired destination, and **Reconcile** can fix the
ports. **Hosts entries** then shows, for each client profile, a hosts-file block
mapping every listener hostname to a stable address in `127.77.0.0/16`, with Copy
and Download. The same deterministic rule is used by `azbridge hosts`, so the
browser block and the CLI agree.

When several hostnames share one address (for example all on `127.0.0.1`), each
listener needs a unique port, and the gateway logs a warning at startup. Two
listeners that resolve to the same address and port stop startup with a clear error.

Missing connections can be created only by workspace admins, with explicit review
consent. The importer waits up to two minutes for Ready and reports failed/deleting
states. Operators must ask an admin to create missing connections. For workspace
imports, each profile is created/reused, validated by the real broker API, then
saved. All mutations use the session CSRF token. **Nothing is published.**

Partial failure does not roll back earlier connections, profiles or revisions.
Keep the review open and choose **Retry incomplete files** to reuse created
profiles and skip completed saves. A previous save whose response was lost is
checked against stored revisions before resaving. After closing/reloading, choose
reuse explicitly when importing an already-created profile. Publish each reviewed
revision yourself, then enroll hosts with `azbridge login --broker <origin>
--workspace <workspace> --profile <profile>`.

The broker also rejects credential-like text in comments and values on validation,
save, publication and gateway configuration loading. Existing published profiles
with commented credentials need a cleaned revision before gateways can load them.
These checks recognize credential syntax; they are not a general-purpose secret
detector. Do not put unrelated secrets into profile fields.

## Browser checks

After `npm run build`, run:

```powershell
npm exec -- playwright install chromium
npm run test:browser
# Alternatively, use an already installed Edge browser on Windows:
$env:PLAYWRIGHT_CHANNEL = 'msedge'
npm run test:browser
```

Vitest uses a DOM adapter for editor workflow/lifecycle tests. Playwright tests
use the actual production Monaco bundle and workers, with a local mocked broker
API, checking schema completions, snippet tab stops, YAML transfers, read-only
permissions, native Fluent theme changes and square controls. Designer checks use
the real React Flow canvas and Monaco to cover bidirectional synchronization,
edge re-targeting, shared undo, comments, invalid drafts and reader restrictions.
Unit workflow tests mock only the canvas layout and Monaco DOM adapter. Device sign-in
checks cover the activation UI, CSRF polling, cancellation, denial and session
refresh. Browser authentication responses are fixtures; Rust mock-GitHub tests
exercise the real backend protocol and session creation. No Azure resources
are needed.

To validate private files against real APIs, build the SPA and opt in to a separate
loopback-only development broker. The dedicated test starts and stops that broker;
it refuses production endpoints and performs no Azure provisioning:

```powershell
$env:AZBRIDGE_IMPORT_FILES = '["C:\\private\\client.yml","C:\\private\\server.yml"]'
$env:PLAYWRIGHT_CHANNEL = 'msedge'
npm exec -- playwright test --config playwright.import-live.config.ts
Remove-Item Env:AZBRIDGE_IMPORT_FILES
```

This test uses actual dev sign-in, workspace/connection provisioning simulation,
profile validation and revision storage. It checks sanitized stored YAML, pairing,
CSRF and no publication; logs contain counts only. Trace, screenshot and video
recording are disabled. It does not establish a real Azure Relay tunnel. Default
browser runs skip private-file validation when the opt-in variable is absent.
