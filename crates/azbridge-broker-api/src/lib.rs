//! Shared broker HTTP DTOs. JSON uses camelCase except the embedded PascalCase
//! azbridge configuration. IDs are UUID strings; timestamps are Unix seconds.
//!
//! Regenerate browser bindings with
//! `cargo run -p azbridge-broker-api --example generate_types`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    pub avatar_url: String,
}

/// Roles are ordered from least to most privileged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Reader,
    Operator,
    Admin,
}

impl Role {
    pub fn permits(self, required: Self) -> bool {
        self >= required
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProvisioningState {
    Pending,
    Provisioning,
    Ready,
    Failed,
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RelayRight {
    Send,
    Listen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub namespace_name: String,
    pub location: String,
    pub state: ProvisioningState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub user: GitHubUser,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub state: ProvisioningState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRevision {
    pub revision: u64,
    pub created_at: u64,
    pub yaml: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RelayEndpointDirection {
    Listener,
    Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RelayEndpointProtocol {
    Tcp,
    Udp,
    Socket,
    Http,
}

/// Display-only endpoint metadata; never includes connection strings or tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RelayMapEndpoint {
    pub id: String,
    pub forward_group: String,
    pub relay_name: String,
    pub direction: RelayEndpointDirection,
    pub protocol: RelayEndpointProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    pub port_name: String,
    pub explicit_port_name: bool,
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RelayMapProfile {
    pub profile: Profile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub endpoints: Vec<RelayMapEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RelayMapResponse {
    pub profiles: Vec<RelayMapProfile>,
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Gateway {
    pub id: String,
    pub name: String,
    pub profile_id: String,
    pub revoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub at: u64,
    /// GitHub user ID or gateway ID, represented as a displayable string.
    pub actor: String,
    pub action: String,
    pub target: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub user: GitHubUser,
    pub platform_admin: bool,
    pub csrf_token: String,
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub allowed_locations: Vec<String>,
}

/// Browser-bound device login; the GitHub device code never leaves the broker.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginStartResponse {
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DeviceLoginPollResponse {
    Pending { interval: u64 },
    SlowDown { interval: u64 },
    Authorized,
    Expired,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceRequest {
    pub name: String,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateMemberRequest {
    pub login: String,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemberRequest {
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectionRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateProfileRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateRevisionRequest {
    pub yaml: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PublishRevisionRequest {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_current_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateGatewayRequest {
    pub name: String,
    pub profile_id: String,
}

/// Contains a one-time secret; deliberately does not implement `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CreateGatewayResponse {
    pub gateway: Gateway,
    pub credential: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentStartRequest {
    /// Workspace UUID or name.
    pub workspace: String,
    /// Profile UUID or name.
    pub profile: String,
    pub gateway_name: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentStartResponse {
    pub enrollment_id: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Minimum polling interval, in seconds.
    pub interval: u64,
    /// Enrollment lifetime, in seconds.
    pub expires_in: u64,
}

/// Tagged device-flow result. Authorized carries a one-time secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EnrollmentPollResponse {
    Pending {
        interval: u64,
    },
    SlowDown {
        interval: u64,
    },
    Authorized {
        credential: String,
        workspace_id: String,
        gateway_id: String,
        profile_id: String,
    },
    Expired,
    Denied,
}

/// The embedded config may contain sensitive endpoint settings, so no `Debug`.
#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfigResponse {
    pub revision: u64,
    pub relay_endpoint: String,
    pub config: azbridge_config::Config,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RelayTokenRequest {
    pub relay_name: String,
    pub right: RelayRight,
}

/// Contains a SAS token; deliberately does not implement `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RelayTokenResponse {
    pub token: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatusRequest {
    pub applied_revision: u64,
    pub version: String,
    pub hostname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResponse {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrokerMode {
    Development,
    Production,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub mode: BrokerMode,
}

/// Render the complete browser contract without writing any files.
pub fn typescript_bindings() -> String {
    let config = ts_rs::Config::new().with_large_int("number");
    let mut output = String::from(
        "// Generated by cargo run -p azbridge-broker-api --example generate_types. Do not edit.\n\
         // JSON u64 values use number; consumers must stay within Number.MAX_SAFE_INTEGER.\n\n",
    );
    macro_rules! declarations {
        ($($ty:ty),+ $(,)?) => {
            $(output.push_str("export ");
              output.push_str(&<$ty>::decl(&config));
              output.push_str("\n\n");)+
        };
    }
    declarations!(
        GitHubUser,
        Role,
        ProvisioningState,
        RelayRight,
        Workspace,
        Member,
        Connection,
        Profile,
        ConfigRevision,
        RelayEndpointDirection,
        RelayEndpointProtocol,
        RelayMapEndpoint,
        RelayMapProfile,
        RelayMapResponse,
        Gateway,
        AuditEntry,
        MeResponse,
        DeviceLoginStartResponse,
        DeviceLoginPollResponse,
        CreateWorkspaceRequest,
        CreateMemberRequest,
        UpdateMemberRequest,
        CreateConnectionRequest,
        CreateProfileRequest,
        CreateRevisionRequest,
        PublishRevisionRequest,
        CreateGatewayRequest,
        CreateGatewayResponse,
        EnrollmentStartRequest,
        EnrollmentStartResponse,
        EnrollmentPollResponse,
        GatewayConfigResponse,
        RelayTokenRequest,
        RelayTokenResponse,
        GatewayStatusRequest,
        ErrorResponse,
        ValidationResponse,
        HealthResponse,
        BrokerMode,
        azbridge_config::Config,
        azbridge_config::LocalForward,
        azbridge_config::LocalForwardBinding,
        azbridge_config::RemoteForward,
        azbridge_config::RemoteForwardBinding,
    );
    output
}
