# Prepare and deploy the workspace broker

The broker serves its Rust API and built React admin UI from one Linux container.
These files **prepare deployment; they do not constitute a deployed service**.
No subscription, tenant, administrator, GitHub registration or Azure region is
preselected. Commands that provision or deploy below are for an operator to run
after reviewing costs, Azure permissions and GitHub configuration.

## Infrastructure and permissions

`infra/main.bicep` is subscription-scoped. It creates two resource groups with
deterministic, globally unique resource-name suffixes:

- **Application group:** Container Apps environment and broker app, Basic ACR,
  Log Analytics, user-assigned managed identity, Key Vault and StorageV2 account.
- **Relay group:** Initially empty except for its broker role assignment. The
  API creates one Relay namespace per workspace in this group; no shared global
  namespace is assumed.

The broker identity has only:

| Role | Scope |
| --- | --- |
| Storage Blob Data Contributor | Broker storage account |
| Storage Table Data Contributor | Broker storage account |
| Key Vault Secrets User | Broker Key Vault |
| AcrPull | Broker container registry |
| Azure Relay Owner | Dedicated Relay resource group |

Azure Relay Owner includes Relay management operations required to create
namespaces, Hybrid Connections and authorization rules; it is **not** subscription
Owner. Never put unrelated Relay workloads in the dedicated Relay group.

Storage shared-key authorization and anonymous blob access are disabled; the
broker uses Microsoft Entra tokens. The private `configs` blob container stores
configuration revisions with versioning and seven-day soft deletion enabled.
The metadata tables are exactly `Workspaces`, `Members`, `UserWorkspaces`,
`PlatformAdmins`, `Connections`, `Profiles`, `Gateways`, `Enrollments`, `Audit`
and `RevisionMetadata`. Application revision immutability does not imply a locked
Azure WORM policy.

Storage and Key Vault have publicly reachable, authenticated HTTPS endpoints.
This baseline does not provision private endpoints or a virtual network.
Container Apps redirects HTTP to HTTPS, runs a single active revision with
minimum/maximum replicas both set to one, and uses 0.5 vCPU / 1 GiB memory.
Single-revision rolling updates may briefly overlap old and new revisions;
this is not a globally distributed locking or high-availability design.
Log Analytics retains 30 days with a 1 GiB daily ingestion cap; hitting the cap
can stop ingestion. ACR, storage, logging, the running replica and workspace
Relay namespaces may all incur charges.

The deploying principal needs permission to create resource groups at subscription
scope, create their resources and assign these roles (for example, Contributor
plus appropriately scoped Role Based Access Control Administrator). It also
needs ACR push permission for application publication; the **broker** identity
deliberately receives pull only. Required providers include `Microsoft.App`,
`Microsoft.ContainerRegistry`, `Microsoft.ManagedIdentity`,
`Microsoft.OperationalInsights`, `Microsoft.Storage`, `Microsoft.KeyVault` and
`Microsoft.Relay`. Provider registration, supported regions and namespace quotas
must be checked by the operator; preparation does not perform those live checks.

## GitHub App registration

Create a **GitHub App**, not an Azure app registration. The broker uses its
OAuth web or device-code flow to identify people by numeric GitHub user ID. Use the GitHub
App's **Client ID** (`Iv...`), not its numeric App ID, and generate a **client
secret**, not a webhook secret or private signing key.

1. Register an app under your user or organization settings with an HTTPS
   homepage. You can use a temporary operator-controlled HTTPS URL while
   provisioning the broker; it is not the final production callback.
2. Do not grant repository write permissions or organization membership
   permissions merely for login. Public identity information is sufficient.
   Webhooks are not required for this sign-in flow.
   Enable **Device Flow** under the app's identifying/authorizing users settings
   for browser device-code sign-in and CLI gateway enrollment; browser OAuth
   alone does not enable it.
3. After provisioning, use the `BROKER_PUBLIC_URL` output as the homepage and
   `GITHUB_CALLBACK_URL` as the exact user authorization callback URL:
   `https://<broker-host>/auth/github/callback`.
   Leave wildcard matching disabled. The hostname is computed from the
   Container Apps environment's default domain, without a self-reference to
   the app's own FQDN.
4. Keep the client secret in a secure password manager or CI secret store.
   Never commit it, paste it into a command line, or log an environment dump.
5. Choose bootstrap administrators by **nonzero numeric user IDs**, not mutable
   login names. For example, an operator can resolve an ID with
   `gh api users/YOUR_LOGIN --jq .id`. Use a comma-separated list of those IDs.
   Platform administration and workspace membership are distinct permissions.

Register separate GitHub Apps and use different cookie keys for independent
environments. Never point a production callback at a development HTTP origin.
See the [broker operator guide](../crates/azbridge-broker/README.md) for numeric-ID
allowlisting, role boundaries, multiworkspace setup, browser CSRF, gateway
credentials, device-code phishing and revocation limitations. Profile authors
can reach services from their assigned gateway hosts; membership is a trust
decision, not just permission to view configuration.

## Container and runtime configuration

`Dockerfile.broker` builds the SPA with `npm ci` / `npm run build`, then builds
only `azbridge-broker` with `cargo build --release --locked -p azbridge-broker`.
The runtime is Debian bookworm-slim with CA certificates and OpenSSL runtime
libraries, UID/GID 10001, no source tree and no development-mode argument.
Static assets are copied to `/app/web/dist`. The root `.dockerignore` excludes
build output, node_modules, azd environments and common secret-file formats.
Do not put credentials in other build-context files. Base image tags follow
their maintained major series; operators requiring reproducible base images
should approve and pin matching Linux image digests before release.

| Environment variable | Contract / source |
| --- | --- |
| `PUBLIC_URL` | HTTPS origin only, no credentials/query/fragment/subpath; computed by Bicep |
| `LISTEN_ADDR` | `0.0.0.0:8080` |
| `GITHUB_CLIENT_ID` | GitHub App OAuth Client ID |
| `GITHUB_CLIENT_SECRET` | Key Vault reference to `github-client-secret` |
| `COOKIE_KEY` | Key Vault reference to `cookie-key`; standard base64 of exactly 32 random bytes |
| `BOOTSTRAP_ADMIN_IDS` | Comma-separated nonzero numeric GitHub IDs |
| `STORAGE_ACCOUNT` | Provisioned account name, not connection string |
| `AZURE_SUBSCRIPTION_ID` | Deployment subscription ID |
| `RELAY_RESOURCE_GROUP` | Dedicated Relay resource group name |
| `ALLOWED_LOCATIONS` | Comma-separated lowercase Azure region codes, e.g. `eastus,westus2` |
| `AZURE_CLIENT_ID` | Broker user-assigned identity **client ID**, not principal/resource ID |
| `STATIC_DIR` | `/app/web/dist`, containing `index.html` |

Variable names are **unprefixed**: do not add `BROKER_` to the runtime settings.
Production validates all required settings and never falls back to `--dev`.

Both secret values enter Bicep through `@secure()` parameters, are written to
Key Vault, and are referenced by the Container App using the managed identity.
No outputs contain secret values. Keep a stable cookie key across restarts;
rotating it invalidates existing sessions. After changing Key Vault secrets,
allow Container Apps secret-reference refresh and restart/redeploy the revision
as appropriate. Re-provisioning writes the supplied secret values again, so
retain the authoritative values securely outside the repository.

## Operator workflow (not executed by preparation)

Install Azure CLI with Bicep, Azure Developer CLI with Docker-language support
(configuration validated against azd 1.35.1), and a running Docker engine
capable of Linux amd64 builds. Review `.azure/deployment-plan.md` and validate
the broker/UI before any live deployment.

From the repository root, configure a new azd environment and nonsecret values:

```powershell
azd auth login
azd env new YOUR_ENVIRONMENT
azd env set AZURE_SUBSCRIPTION_ID YOUR_SUBSCRIPTION_ID
azd env set AZURE_LOCATION YOUR_SUPPORTED_AZURE_REGION
azd env set GITHUB_CLIENT_ID YOUR_GITHUB_APP_CLIENT_ID
azd env set BOOTSTRAP_ADMIN_IDS YOUR_NUMERIC_GITHUB_USER_ID
azd env set ALLOWED_LOCATIONS eastus,westus2
```

Supply secrets as process environment variables so they are not persisted by
`azd env set` or typed into shell history. PowerShell 7 example:

```powershell
$env:GITHUB_CLIENT_SECRET = [System.Net.NetworkCredential]::new(
    '', (Read-Host 'GitHub App client secret' -AsSecureString)
).Password
$bytes = [byte[]]::new(32)
[System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
$env:COOKIE_KEY = [Convert]::ToBase64String($bytes)
# Save the key securely without printing it; reuse that value on later provision runs.
```

The following commands **create Azure resources / incur charges**. Run only
after the environment and deployment permissions have been reviewed:

```powershell
azd provision
# Update the GitHub App homepage/callback from the nonsecret outputs.
azd package broker
azd deploy broker
Remove-Item Env:GITHUB_CLIENT_SECRET, Env:COOKIE_KEY
```

`azd provision` can run before an image exists in ACR. On the first run, it
creates a public Microsoft hello-world **placeholder**, with port 80 and root
probes, **without injecting broker secrets or environment variables**. A
placeholder is not an operational broker and has no broker authentication.
Do not direct users to it.

`azure.yaml` selects `infra/broker.bicep` and `broker.parameters.json` for
**revision-based deployment**. `azd deploy broker` publishes the built image
and applies port 8080 plus `/healthz` readiness/liveness probes together.
Changing only the image of the placeholder manually leaves the wrong port and
probes configured. `SERVICE_BROKER_IMAGE_NAME` is populated by azd at deployment;
`SERVICE_BROKER_RESOURCE_EXISTS` is populated during provisioning. Subsequent
provisioning preserves the existing image rather than reverting to hello-world.
Do not hardcode the existence parameter to false on an existing deployment.

Useful nonsecret outputs include `AZURE_RESOURCE_GROUP`,
`AZURE_RELAY_RESOURCE_GROUP`, `AZURE_CONTAINER_APP_NAME`,
`AZURE_CONTAINER_REGISTRY_ENDPOINT`, `AZURE_STORAGE_ACCOUNT_NAME`,
`BROKER_PUBLIC_URL` and `GITHUB_CALLBACK_URL`. Avoid commands that dump all azd
environment values, since operators may have stored unrelated secrets there.

## Validation and acceptance

Local template compilation (no Azure login or live resource changes):

```powershell
az bicep build --file infra\main.bicep --stdout | Out-Null
az bicep build --file infra\broker.bicep --stdout | Out-Null
python infra\tests\test_contract.py
cargo test -p azbridge-broker
npm --prefix web run lint
npm --prefix web run build
docker build --platform linux/amd64 --file Dockerfile.broker --tag azbridge-broker:local .
```

Do not mistake successful compilation for Azure validation or deployment.
The standard-library Python contract tests compile both templates in memory and
check secure parameters, identity/RBAC scope, storage, environment variables,
single-replica ingress and placeholder-versus-broker probe configuration.
Without a Docker daemon, the image build is **unverified**, even if Rust and
SPA builds pass independently. Template compilation does not validate region
availability, quotas, RBAC propagation, network access or Key Vault resolution.

After an explicitly authorized deployment, verify `/healthz` returns broker
JSON with production mode (not a placeholder HTML response), sign in as a
bootstrap administrator, create separate workspaces, and verify their Relay
namespaces are created in the dedicated group and allowed locations. Verify
authorization boundaries, a published/retrieved config revision, gateway
enrollment/heartbeat and a real tunnel before exposing the broker to users.
An initial 403/secret-resolution failure can require waiting for RBAC
propagation and retrying the operator's deployment; never work around it by
enabling storage keys, registry admin credentials or development authentication.

## Module selection and references

The AVM catalog was checked in pattern-first order. The selected AZD patterns
are `avm/ptn/azd/container-apps-stack:0.4.0` and
`avm/ptn/azd/acr-container-app:0.5.0`. They support external HTTPS ingress and
a precreated user-assigned identity for ACR pull, avoiding a circular identity
dependency. Remaining resources use pinned AVM resource modules for storage
(`0.33.1`), Key Vault (`0.13.3`), Log Analytics (`0.16.1`) and managed identity
(`0.6.0`). Key Vault is pinned to a version compatible with the available
Bicep 0.42.1 type catalog. The dedicated Relay role assignment and secure secret
resources are small resource-specific Bicep declarations. AVM usage telemetry
is disabled.

- [AVM Bicep pattern catalog](https://azure.github.io/Azure-Verified-Modules/indexes/bicep/bicep-pattern-modules/)
- [AVM module source and contracts](https://github.com/Azure/bicep-registry-modules/tree/main/avm)
- [azd Docker-language support](https://learn.microsoft.com/azure/developer/azure-developer-cli/docker-language-support)
- [azd Container Apps revision-based deployment](https://learn.microsoft.com/azure/developer/azure-developer-cli/container-apps-workflows)
- [Container Apps Key Vault references](https://learn.microsoft.com/azure/container-apps/manage-secrets)
- [Azure Relay Owner role definition](https://learn.microsoft.com/azure/role-based-access-control/built-in-roles/integration#azure-relay-owner)
- [GitHub App OAuth callback registration](https://docs.github.com/apps/creating-github-apps/registering-a-github-app/about-the-user-authorization-callback-url)
- [GitHub App OAuth and device flow](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app)
