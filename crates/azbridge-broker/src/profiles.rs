//! Workspace-scoped profiles with immutable, explicitly published revisions.
//!
//! Validation rejects unknown YAML keys, embedded Relay authentication/endpoints,
//! local logging settings, and unauthenticated forwarding. Revisions keep their
//! original YAML; gateway snapshots are normalized and revalidated on every load.
//! Failed uploads may consume revision numbers but never appear in revision lists.

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use azbridge_broker_api::{
    ConfigRevision, Connection, CreateProfileRequest, CreateRevisionRequest, Profile,
    ProvisioningState, PublishRevisionRequest, RelayMapProfile, RelayMapResponse, Role,
    ValidationResponse,
};
use azbridge_config::Config;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    AppState,
    auth::{authorize, now},
    error::BrokerError,
    models::{
        GatewayRecord, PLATFORM_PARTITION, ProfileRecord, RevisionRecord, WorkspaceRecord,
        record_audit, revision_key,
    },
    storage::{Entity, Table},
    workspaces::delete_entity,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/workspaces/{ws}/relay-map", get(relay_map))
        .route(
            "/api/v1/workspaces/{ws}/profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/v1/workspaces/{ws}/profiles/{profile}",
            get(get_profile).delete(delete_profile),
        )
        .route(
            "/api/v1/workspaces/{ws}/profiles/{profile}/validate",
            post(validate),
        )
        .route(
            "/api/v1/workspaces/{ws}/profiles/{profile}/revisions",
            get(list_revisions).post(create_revision),
        )
        .route(
            "/api/v1/workspaces/{ws}/profiles/{profile}/revisions/{revision}",
            get(get_revision),
        )
        .route(
            "/api/v1/workspaces/{ws}/profiles/{profile}/publish",
            post(publish),
        )
}

fn decode<T: DeserializeOwned>(entity: &Entity) -> Result<T, BrokerError> {
    serde_json::from_value(entity.data.clone()).map_err(|error| BrokerError::Internal(error.into()))
}

fn encode<T: Serialize>(value: &T) -> Result<serde_json::Value, BrokerError> {
    serde_json::to_value(value).map_err(|error| BrokerError::Internal(error.into()))
}

async fn workspace(state: &AppState, ws: &str, ready: bool) -> Result<(), BrokerError> {
    uuid::Uuid::parse_str(ws).map_err(|_| BrokerError::BadRequest)?;
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, ws)
        .await?
        .ok_or(BrokerError::NotFound)?;
    let record: WorkspaceRecord = decode(&entity)?;
    if record.workspace.id != ws {
        return Err(BrokerError::NotFound);
    }
    if ready && record.workspace.state != ProvisioningState::Ready {
        return Err(BrokerError::Conflict);
    }
    Ok(())
}

async fn profile(
    state: &AppState,
    ws: &str,
    id: &str,
) -> Result<(Entity, ProfileRecord), BrokerError> {
    uuid::Uuid::parse_str(id).map_err(|_| BrokerError::BadRequest)?;
    let entity = state
        .metadata_store
        .get(Table::Profiles, ws, id)
        .await?
        .ok_or(BrokerError::NotFound)?;
    let record: ProfileRecord = decode(&entity)?;
    if record.profile.id != id {
        return Err(BrokerError::NotFound);
    }
    Ok((entity, record))
}

/// Parse and normalize broker-safe YAML, including workspace-local Ready relay references.
/// Invalid YAML/configuration returns `BadRequest`; storage failures remain errors.
pub async fn validate_config(
    state: &AppState,
    ws: &str,
    yaml: &str,
) -> Result<Config, BrokerError> {
    check_config(state, ws, yaml)
        .await?
        .map_err(|_| BrokerError::BadRequest)
}

async fn check_config(
    state: &AppState,
    ws: &str,
    yaml: &str,
) -> Result<Result<Config, Vec<String>>, BrokerError> {
    workspace(state, ws, true).await?;
    let config = match parse_managed_config(yaml) {
        Ok(config) => config,
        Err(errors) => return Ok(Err(errors)),
    };
    let connections = state.metadata_store.list(Table::Connections, ws).await?;
    let mut ready_names = HashSet::new();
    for entity in connections {
        let connection: Connection = decode(&entity)?;
        if connection.state == ProvisioningState::Ready {
            ready_names.insert(connection.name);
        }
    }
    let mut errors = Vec::new();
    for name in config
        .local_forward
        .iter()
        .map(|forward| &forward.relay_name)
        .chain(
            config
                .remote_forward
                .iter()
                .map(|forward| &forward.relay_name),
        )
    {
        if !ready_names.contains(name) {
            errors.push("RelayName must identify a Ready connection in this workspace".into());
        }
    }
    if errors.is_empty() {
        Ok(Ok(config))
    } else {
        Ok(Err(errors))
    }
}

pub(crate) fn parse_managed_config(yaml: &str) -> Result<Config, Vec<String>> {
    if yaml.len() > 1024 * 1024 {
        return Err(vec!["YAML exceeds the 1 MiB limit".into()]);
    }
    let compact: String = yaml
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if [
        "sharedaccesskey",
        "sharedaccesssignature",
        "sig=",
        "endpoint=sb://",
        "connectionstring:",
        "azurerelayshared",
        "azurerelayconnectionstring",
        "azurerelayendpoint",
        "sb-hc-token",
        "servicebusauthorization",
    ]
    .iter()
    .any(|pattern| compact.contains(pattern))
    {
        return Err(vec![
            "Credential-like text is not allowed in managed profiles (including comments)".into(),
        ]);
    }
    let mut errors = Vec::new();
    let parsed = serde_ignored::deserialize(serde_yaml::Deserializer::from_str(yaml), |path| {
        errors.push(format!("Unknown configuration field: {path}"));
    });
    let mut config: Config = match parsed {
        Ok(config) => config,
        // Parser diagnostics can contain arbitrary YAML values, including rejected credentials.
        Err(_) => return Err(vec!["Invalid YAML configuration".into()]),
    };
    let raw: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|_| vec!["Invalid YAML configuration".into()])?;
    for (field, present) in [
        (
            "AzureRelayConnectionString",
            config.azure_relay_connection_string.is_some(),
        ),
        ("AzureRelayEndpoint", config.azure_relay_endpoint.is_some()),
        (
            "AzureRelaySharedAccessKeyName",
            config.azure_relay_shared_access_key_name.is_some(),
        ),
        (
            "AzureRelaySharedAccessKey",
            config.azure_relay_shared_access_key.is_some(),
        ),
        (
            "AzureRelaySharedAccessSignature",
            config.azure_relay_shared_access_signature.is_some(),
        ),
        ("LogFileName", config.log_file_name.is_some()),
        ("LogLevel", config.log_level.is_some()),
    ] {
        if present || raw.get(field).is_some() {
            errors.push(format!("{field} is not allowed in managed profiles"));
        }
    }
    if config.local_forward.is_empty() && config.remote_forward.is_empty() {
        errors.push("At least one forwarding rule is required".into());
    }
    for (index, forward) in config.local_forward.iter_mut().enumerate() {
        if forward.connection_string.is_some()
            || raw
                .get("LocalForward")
                .and_then(|value| value.get(index))
                .and_then(|value| value.get("ConnectionString"))
                .is_some()
        {
            errors.push("LocalForward.ConnectionString is not allowed".into());
        }
        if !forward.bindings.is_empty()
            && (forward.bind_address.is_some()
                || forward.bind_port.is_some()
                || forward.port_name.is_some()
                || forward.bind_local_socket.is_some()
                || forward.host_name.is_some()
                || forward.no_authentication.is_some())
        {
            errors.push("LocalForward must not mix Bindings and shorthand fields".into());
        }
        // Inspect shorthand even when explicit bindings take precedence in normalize().
        if forward.no_authentication == Some(true)
            || forward
                .bindings
                .iter()
                .any(|binding| binding.no_authentication)
        {
            errors.push("LocalForward.NoAuthentication must not be true".into());
        }
        forward.normalize();
        if forward.bindings.is_empty() {
            errors.push("LocalForward requires at least one binding".into());
        }
    }
    for (index, forward) in config.remote_forward.iter_mut().enumerate() {
        if forward.connection_string.is_some()
            || raw
                .get("RemoteForward")
                .and_then(|value| value.get(index))
                .and_then(|value| value.get("ConnectionString"))
                .is_some()
        {
            errors.push("RemoteForward.ConnectionString is not allowed".into());
        }
        if !forward.bindings.is_empty()
            && (forward.host.is_some()
                || forward.host_port.is_some()
                || forward.port_name.is_some()
                || forward.local_socket.is_some()
                || forward.http.is_some())
        {
            errors.push("RemoteForward must not mix Bindings and shorthand fields".into());
        }
        forward.normalize();
        if forward.bindings.is_empty() {
            errors.push("RemoteForward requires at least one binding".into());
        }
    }
    // The shared validator checks every port, even socket-only bindings. Socket paths
    // intentionally remain valid managed targets; the returned config keeps their zero port.
    let mut checked = config.clone();
    for forward in &mut checked.local_forward {
        for binding in &mut forward.bindings {
            if let Some(path) = &binding.bind_local_socket {
                if path.is_empty() || path.contains('\0') {
                    errors.push("BindLocalSocket must be a nonempty socket path".into());
                } else if binding.bind_port == 0 {
                    binding.bind_port = 1;
                }
            }
        }
    }
    for forward in &mut checked.remote_forward {
        for binding in &mut forward.bindings {
            if let Some(path) = &binding.local_socket {
                if path.is_empty() || path.contains('\0') {
                    errors.push("LocalSocket must be a nonempty socket path".into());
                } else if binding.host_port == 0 {
                    binding.host_port = 1;
                }
            }
        }
    }
    if checked.validate().is_err() {
        errors.push("Invalid forwarding configuration or option value".into());
    }
    if errors.is_empty() {
        Ok(config)
    } else {
        Err(errors)
    }
}

/// Load the published immutable snapshot and revalidate its current relay references.
/// This returns no credentials or endpoint; the gateway supplies the endpoint separately.
pub async fn published_config(
    state: &AppState,
    ws: &str,
    id: &str,
) -> Result<(u64, Config), BrokerError> {
    workspace(state, ws, true).await?;
    let (_, record) = profile(state, ws, id).await?;
    let number = record
        .profile
        .current_revision
        .ok_or(BrokerError::Conflict)?;
    let snapshot = revision(state, ws, id, number).await?;
    Ok((
        snapshot.revision,
        validate_config(state, ws, &snapshot.yaml).await?,
    ))
}

async fn revision(
    state: &AppState,
    ws: &str,
    id: &str,
    number: u64,
) -> Result<ConfigRevision, BrokerError> {
    if number == 0 {
        return Err(BrokerError::BadRequest);
    }
    let entity = state
        .metadata_store
        .get(Table::RevisionMetadata, ws, &revision_key(id, number))
        .await?
        .ok_or(BrokerError::NotFound)?;
    let record: RevisionRecord = decode(&entity)?;
    if record.revision != number {
        return Err(BrokerError::NotFound);
    }
    let yaml = state
        .config_store
        .get_revision(ws, id, &number.to_string())
        .await?
        .ok_or(BrokerError::NotFound)?;
    Ok(ConfigRevision {
        revision: number,
        created_at: record.created_at,
        yaml,
    })
}

async fn list_profiles(
    State(state): State<AppState>,
    Path(ws): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Profile>>, BrokerError> {
    authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace(&state, &ws, false).await?;
    let records = state.metadata_store.list(Table::Profiles, &ws).await?;
    let mut profiles = records
        .iter()
        .map(decode::<ProfileRecord>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|record| record.profile)
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Json(profiles))
}

async fn relay_map(
    State(state): State<AppState>,
    Path(ws): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RelayMapResponse>, BrokerError> {
    authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace(&state, &ws, false).await?;
    let records = state.metadata_store.list(Table::Profiles, &ws).await?;
    let revisions = state
        .metadata_store
        .list(Table::RevisionMetadata, &ws)
        .await?;
    let connections = state
        .metadata_store
        .list(Table::Connections, &ws)
        .await?
        .iter()
        .map(decode::<Connection>)
        .collect::<Result<Vec<_>, _>>()?;
    let mut profiles = Vec::new();
    for entity in records {
        let record: ProfileRecord = decode(&entity)?;
        // Reserved numbers can be gaps after failed uploads; only metadata marks a saved revision.
        let prefix = format!("{}:", record.profile.id);
        let saved = revisions
            .iter()
            .filter(|entity| entity.row_key.starts_with(&prefix))
            .map(decode::<RevisionRecord>)
            .collect::<Result<Vec<_>, _>>();
        let (latest, error) = match saved {
            Ok(saved) => (
                saved.into_iter().map(|revision| revision.revision).max(),
                None,
            ),
            Err(_) => (None, Some("Unable to read saved revision metadata".into())),
        };
        let mut entry = RelayMapProfile {
            profile: record.profile,
            revision: latest,
            endpoints: Vec::new(),
            error,
        };
        if let Some(number) = latest {
            match revision(&state, &ws, &entry.profile.id, number).await {
                Ok(snapshot) => match crate::relay_map::extract_endpoints(&snapshot.yaml) {
                    Ok(endpoints) => entry.endpoints = endpoints,
                    Err(()) => entry.error = Some("Unable to read saved configuration".into()),
                },
                Err(_) => entry.error = Some("Unable to load saved revision".into()),
            }
        }
        profiles.push(entry);
    }
    profiles.sort_by(|left, right| left.profile.name.cmp(&right.profile.name));
    Ok(Json(RelayMapResponse {
        profiles,
        connections,
    }))
}

async fn create_profile(
    State(state): State<AppState>,
    Path(ws): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<Profile>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let actor = authorize(&state, &headers, &ws, Role::Operator, true).await?;
    workspace(&state, &ws, true).await?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(BrokerError::BadRequest);
    }
    for entity in state.metadata_store.list(Table::Profiles, &ws).await? {
        if decode::<ProfileRecord>(&entity)?
            .profile
            .name
            .eq_ignore_ascii_case(name)
        {
            return Err(BrokerError::Conflict);
        }
    }
    let profile = Profile {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        current_revision: None,
    };
    let record = ProfileRecord {
        profile: profile.clone(),
        next_revision: 1,
    };
    state
        .metadata_store
        .insert(Table::Profiles, &ws, &profile.id, encode(&record)?)
        .await?;
    record_audit(
        &state,
        &ws,
        &actor.user.id.to_string(),
        "profile.create",
        &profile.id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(profile)))
}

async fn get_profile(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Profile>, BrokerError> {
    authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace(&state, &ws, false).await?;
    Ok(Json(profile(&state, &ws, &id).await?.1.profile))
}

async fn delete_profile(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let actor = authorize(&state, &headers, &ws, Role::Operator, true).await?;
    workspace(&state, &ws, true).await?;
    let (entity, _) = profile(&state, &ws, &id).await?;
    for gateway in state.metadata_store.list(Table::Gateways, &ws).await? {
        let record: GatewayRecord = decode(&gateway)?;
        if record.gateway.profile_id == id && !record.gateway.revoked {
            return Err(BrokerError::ProfileInUse);
        }
    }
    // Immutable revisions remain as inaccessible history, just as after workspace deletion.
    delete_entity(&state, Table::Profiles, &entity).await?;
    record_audit(
        &state,
        &ws,
        &actor.user.id.to_string(),
        "profile.delete",
        &id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn validate(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<CreateRevisionRequest>,
) -> Result<Json<ValidationResponse>, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    authorize(&state, &headers, &ws, Role::Operator, true).await?;
    workspace(&state, &ws, true).await?;
    profile(&state, &ws, &id).await?;
    let errors = check_config(&state, &ws, &request.yaml)
        .await?
        .err()
        .unwrap_or_default();
    Ok(Json(ValidationResponse {
        valid: errors.is_empty(),
        errors,
    }))
}

async fn list_revisions(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Vec<ConfigRevision>>, BrokerError> {
    authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace(&state, &ws, false).await?;
    profile(&state, &ws, &id).await?;
    let prefix = format!("{id}:");
    let mut revisions = Vec::new();
    for entity in state
        .metadata_store
        .list(Table::RevisionMetadata, &ws)
        .await?
    {
        if entity.row_key.starts_with(&prefix) {
            let record: RevisionRecord = decode(&entity)?;
            revisions.push(revision(&state, &ws, &id, record.revision).await?);
        }
    }
    revisions.sort_by_key(|revision| revision.revision);
    Ok(Json(revisions))
}

async fn get_revision(
    State(state): State<AppState>,
    Path((ws, id, number)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Result<Json<ConfigRevision>, BrokerError> {
    authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace(&state, &ws, false).await?;
    profile(&state, &ws, &id).await?;
    Ok(Json(revision(&state, &ws, &id, number).await?))
}

async fn create_revision(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<CreateRevisionRequest>,
) -> Result<(StatusCode, Json<ConfigRevision>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let actor = authorize(&state, &headers, &ws, Role::Operator, true).await?;
    workspace(&state, &ws, true).await?;
    let (entity, mut record) = profile(&state, &ws, &id).await?;
    validate_config(&state, &ws, &request.yaml).await?;
    let number = record.next_revision;
    if number == 0 {
        return Err(BrokerError::Conflict);
    }
    record.next_revision = number.checked_add(1).ok_or(BrokerError::Conflict)?;
    // Reserve the number by CAS before touching the immutable blob. Failed uploads
    // leave honest gaps, never an overwritten revision or a listed incomplete upload.
    state
        .metadata_store
        .replace(Table::Profiles, &ws, &id, encode(&record)?, &entity.etag)
        .await?;
    state
        .config_store
        .put_revision(&ws, &id, &number.to_string(), &request.yaml)
        .await?;
    let metadata = RevisionRecord {
        revision: number,
        created_at: now()?,
    };
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            &ws,
            &revision_key(&id, number),
            encode(&metadata)?,
        )
        .await?;
    record_audit(
        &state,
        &ws,
        &actor.user.id.to_string(),
        "profile.revision.create",
        &revision_key(&id, number),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ConfigRevision {
            revision: number,
            created_at: metadata.created_at,
            yaml: request.yaml,
        }),
    ))
}

async fn publish(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<PublishRevisionRequest>,
) -> Result<Json<Profile>, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let actor = authorize(&state, &headers, &ws, Role::Operator, true).await?;
    workspace(&state, &ws, true).await?;
    let (entity, mut record) = profile(&state, &ws, &id).await?;
    if record.profile.current_revision != request.expected_current_revision {
        return Err(BrokerError::Conflict);
    }
    let snapshot = revision(&state, &ws, &id, request.revision).await?;
    validate_config(&state, &ws, &snapshot.yaml).await?;
    record.profile.current_revision = Some(request.revision);
    state
        .metadata_store
        .replace(Table::Profiles, &ws, &id, encode(&record)?, &entity.etag)
        .await?;
    record_audit(
        &state,
        &ws,
        &actor.user.id.to_string(),
        "profile.publish",
        &revision_key(&id, request.revision),
    )
    .await?;
    Ok(Json(record.profile))
}
