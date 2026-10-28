//! One-time device enrollment and workspace-scoped, revocable gateway credentials.
//! Revocation blocks future broker calls; already established tunnels are unaffected.
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    routing::{delete, get, post},
};
use azbridge_broker_api::{
    CreateGatewayRequest, CreateGatewayResponse, EnrollmentPollResponse, EnrollmentStartRequest,
    EnrollmentStartResponse, Gateway, GatewayConfigResponse, GatewayStatusRequest, GitHubUser,
    ProvisioningState, RelayRight, RelayTokenRequest, RelayTokenResponse, Role,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::{self, DevicePoll, now},
    error::BrokerError,
    models::{
        EnrollmentRecord, GatewayRecord, PLATFORM_PARTITION, ProfileRecord, WorkspaceRecord,
        record_audit,
    },
    profiles::published_config,
    storage::{Entity, Table},
    workspaces::{active_workspace, decode, delete_entity, encode, uuid},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/enrollments", post(start_enrollment))
        .route("/api/v1/enrollments/{id}/poll", post(poll_enrollment))
        .route(
            "/api/v1/workspaces/{ws}/gateways",
            get(list_gateways).post(create_gateway),
        )
        .route(
            "/api/v1/workspaces/{ws}/gateways/{id}",
            delete(revoke_gateway),
        )
        .route("/api/v1/gateway/config", get(gateway_config))
        .route("/api/v1/gateway/relay-tokens", post(relay_token))
        .route("/api/v1/gateway/status", post(status))
        .route("/api/v1/gateway/logout", post(logout))
}

#[cfg(test)]
#[path = "gateways_tests.rs"]
mod tests;

fn valid_name(name: &str) -> Result<(), BrokerError> {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

async fn ready_workspace(state: &AppState, ws: &str) -> Result<WorkspaceRecord, BrokerError> {
    let record = active_workspace(state, ws).await?;
    if record.workspace.state != ProvisioningState::Ready {
        return Err(BrokerError::Conflict);
    }
    Ok(record)
}

async fn list_gateways(
    State(state): State<AppState>,
    Path(ws): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<Gateway>>, BrokerError> {
    auth::authorize(&state, &headers, &ws, Role::Reader, false).await?;
    active_workspace(&state, &ws).await?;
    let mut gateways = Vec::new();
    for entity in state.metadata_store.list(Table::Gateways, &ws).await? {
        gateways.push(decode::<GatewayRecord>(entity.data)?.gateway);
    }
    Ok(Json(gateways))
}

async fn create_gateway(
    State(state): State<AppState>,
    Path(ws): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<CreateGatewayRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<CreateGatewayResponse>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Operator, true).await?;
    let Json(request) = payload.map_err(|_| BrokerError::BadRequest)?;
    Ok((
        StatusCode::CREATED,
        Json(mint(&state, &ws, request, session.user.id).await?),
    ))
}

fn digest(credential: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(credential.as_bytes()))
}

async fn mint(
    state: &AppState,
    ws: &str,
    request: CreateGatewayRequest,
    actor: u64,
) -> Result<CreateGatewayResponse, BrokerError> {
    valid_name(&request.name)?;
    ready_workspace(state, ws).await?;
    published_config(state, ws, &request.profile_id).await?;
    let gateway = Gateway {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        profile_id: request.profile_id,
        revoked: false,
        last_seen_at: None,
        applied_revision: None,
        version: None,
        hostname: None,
    };
    let credential = format!("azgw.{ws}.{}.{}", gateway.id, auth::random_secret());
    let record = GatewayRecord {
        gateway: gateway.clone(),
        credential_hash: digest(&credential),
        created_by: actor,
    };
    state
        .metadata_store
        .insert(Table::Gateways, ws, &gateway.id, encode(record)?)
        .await?;
    record_audit(state, ws, &actor.to_string(), "gateway.create", &gateway.id).await?;
    Ok(CreateGatewayResponse {
        gateway,
        credential,
        workspace_id: ws.into(),
    })
}

async fn revoke_gateway(
    State(state): State<AppState>,
    Path((ws, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Operator, true).await?;
    active_workspace(&state, &ws).await?;
    uuid(&id)?;
    let entity = state
        .metadata_store
        .get(Table::Gateways, &ws, &id)
        .await?
        .ok_or(BrokerError::NotFound)?;
    let mut record: GatewayRecord = decode(entity.data.clone())?;
    record.gateway.revoked = true;
    save_gateway(&state, &entity, &record).await?;
    record_audit(
        &state,
        &ws,
        &session.user.id.to_string(),
        "gateway.revoke",
        &id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn save_gateway(
    state: &AppState,
    entity: &Entity,
    record: &GatewayRecord,
) -> Result<(), BrokerError> {
    state
        .metadata_store
        .replace(
            Table::Gateways,
            &entity.partition_key,
            &entity.row_key,
            encode(record)?,
            &entity.etag,
        )
        .await?;
    Ok(())
}

async fn bearer(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Entity, GatewayRecord), BrokerError> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let credential = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(BrokerError::Unauthorized)?;
    if values.next().is_some() {
        return Err(BrokerError::Unauthorized);
    }
    let parts: Vec<_> = credential.split('.').collect();
    if parts.len() != 4
        || parts[0] != "azgw"
        || uuid::Uuid::parse_str(parts[1]).is_err()
        || uuid::Uuid::parse_str(parts[2]).is_err()
        || URL_SAFE_NO_PAD
            .decode(parts[3])
            .map_or(true, |secret| secret.len() != 32)
    {
        return Err(BrokerError::Unauthorized);
    }
    let entity = state
        .metadata_store
        .get(Table::Gateways, parts[1], parts[2])
        .await?
        .ok_or(BrokerError::Unauthorized)?;
    let record: GatewayRecord = decode(entity.data.clone())?;
    if record.gateway.id != parts[2]
        || record.gateway.revoked
        || !auth::constant_time_equal(&digest(credential), &record.credential_hash)
    {
        return Err(BrokerError::Unauthorized);
    }
    ready_workspace(state, parts[1]).await?;
    Ok((entity, record))
}

async fn gateway_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let (entity, mut record) = bearer(&state, &headers).await?;
    let ws = &entity.partition_key;
    let (revision, config) = published_config(&state, ws, &record.gateway.profile_id).await?;
    let workspace = ready_workspace(&state, ws).await?;
    record.gateway.last_seen_at = Some(now()?);
    save_gateway(&state, &entity, &record).await?;
    let mut response = encode(GatewayConfigResponse {
        revision,
        relay_endpoint: format!(
            "sb://{}.servicebus.windows.net/",
            workspace.workspace.namespace_name
        ),
        config,
    })?;
    // The shared Config serializer emits absent options as null. Keep the
    // managed endpoint exclusively in relayEndpoint, including on the wire.
    if let Some(config) = response
        .get_mut("config")
        .and_then(serde_json::Value::as_object_mut)
    {
        config.remove("AzureRelayEndpoint");
    }
    Ok(Json(response))
}

async fn relay_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<RelayTokenRequest>, JsonRejection>,
) -> Result<Json<RelayTokenResponse>, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let (entity, mut record) = bearer(&state, &headers).await?;
    let Json(request) = payload.map_err(|_| BrokerError::BadRequest)?;
    let ws = &entity.partition_key;
    let (_, config) = published_config(&state, ws, &record.gateway.profile_id).await?;
    let permitted = match request.right {
        RelayRight::Send => config
            .local_forward
            .iter()
            .any(|f| f.relay_name == request.relay_name),
        RelayRight::Listen => config
            .remote_forward
            .iter()
            .any(|f| f.relay_name == request.relay_name),
    };
    if !permitted {
        return Err(BrokerError::Forbidden);
    }
    let workspace = ready_workspace(&state, ws).await?;
    let token = state
        .relay_manager
        .issue_token(
            &workspace.workspace.namespace_name,
            &request.relay_name,
            request.right,
        )
        .await?;
    record.gateway.last_seen_at = Some(now()?);
    save_gateway(&state, &entity, &record).await?;
    Ok(Json(token))
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<GatewayStatusRequest>, JsonRejection>,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let (entity, mut record) = bearer(&state, &headers).await?;
    let Json(request) = payload.map_err(|_| BrokerError::BadRequest)?;
    valid_name(&request.version)?;
    valid_name(&request.hostname)?;
    let (revision, _) =
        published_config(&state, &entity.partition_key, &record.gateway.profile_id).await?;
    if request.applied_revision == 0 || request.applied_revision > revision {
        return Err(BrokerError::BadRequest);
    }
    record.gateway.last_seen_at = Some(now()?);
    record.gateway.applied_revision = Some(request.applied_revision);
    record.gateway.version = Some(request.version);
    record.gateway.hostname = Some(request.hostname);
    save_gateway(&state, &entity, &record).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let (entity, mut record) = bearer(&state, &headers).await?;
    record.gateway.revoked = true;
    record.gateway.last_seen_at = Some(now()?);
    save_gateway(&state, &entity, &record).await?;
    record_audit(
        &state,
        &entity.partition_key,
        &record.gateway.id,
        "gateway.logout",
        &record.gateway.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_enrollment(
    State(state): State<AppState>,
    payload: Result<Json<EnrollmentStartRequest>, JsonRejection>,
) -> Result<Json<EnrollmentStartResponse>, BrokerError> {
    state
        .auth
        .rate_limit("enrollment.start", 30, Duration::from_secs(60))
        .await?;
    let Json(request) = payload.map_err(|_| BrokerError::BadRequest)?;
    valid_name(&request.gateway_name)?;
    let _lock = state.mutation_lock.lock().await;
    let mut workspaces = Vec::new();
    for entity in state
        .metadata_store
        .list(Table::Workspaces, PLATFORM_PARTITION)
        .await?
    {
        let record: WorkspaceRecord = decode(entity.data)?;
        if record.workspace.id == request.workspace || record.workspace.name == request.workspace {
            workspaces.push(record.workspace);
        }
    }
    if workspaces.len() != 1 {
        return Err(BrokerError::BadRequest);
    }
    let ws = &workspaces[0].id;
    let mut profiles = Vec::new();
    for entity in state.metadata_store.list(Table::Profiles, ws).await? {
        let record: ProfileRecord = decode(entity.data)?;
        if record.profile.id == request.profile || record.profile.name == request.profile {
            profiles.push(record.profile);
        }
    }
    if profiles.len() != 1 {
        return Err(BrokerError::BadRequest);
    }
    let profile = &profiles[0].id;
    published_config(&state, ws, profile).await?;
    let device = if let Some(github) = &state.auth.github {
        github.start_device_flow().await?
    } else if state.settings.dev {
        auth::DeviceCode {
            device_code: auth::random_secret(),
            user_code: "DEV-USER-1".into(),
            verification_uri: state.settings.public_url.to_string(),
            expires_in: 900,
            interval: 1,
        }
    } else {
        return Err(BrokerError::Unavailable);
    };
    let timestamp = now()?;
    let id = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let record = EnrollmentRecord {
        workspace_id: ws.clone(),
        profile_id: profile.clone(),
        gateway_name: request.gateway_name,
        encrypted_device_code: state.auth.codec.seal(&device.device_code)?,
        expires_at: timestamp + device.expires_in,
        next_poll_at: timestamp + device.interval,
        interval: device.interval,
    };
    state
        .metadata_store
        .insert(Table::Enrollments, PLATFORM_PARTITION, &id, encode(record)?)
        .await?;
    Ok(Json(EnrollmentStartResponse {
        enrollment_id: id,
        user_code: device.user_code,
        verification_uri: device.verification_uri,
        interval: device.interval,
        expires_in: device.expires_in,
    }))
}

async fn poll_enrollment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<EnrollmentPollResponse>, BrokerError> {
    state
        .auth
        .rate_limit("enrollment.poll", 240, Duration::from_secs(60))
        .await?;
    let _lock = state.mutation_lock.lock().await;
    if URL_SAFE_NO_PAD
        .decode(&id)
        .map_or(true, |bytes| bytes.len() != 16)
    {
        return Ok(Json(EnrollmentPollResponse::Expired));
    }
    let Some(entity) = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &id)
        .await?
    else {
        return Ok(Json(EnrollmentPollResponse::Expired));
    };
    let mut record: EnrollmentRecord = decode(entity.data.clone())?;
    let timestamp = now()?;
    if record.expires_at <= timestamp {
        delete_entity(&state, Table::Enrollments, &entity).await?;
        return Ok(Json(EnrollmentPollResponse::Expired));
    }
    let early = timestamp < record.next_poll_at;
    if early {
        record.interval = record.interval.saturating_add(5);
    }
    record.next_poll_at = timestamp.saturating_add(record.interval);
    // Reserve the poll with CAS before contacting GitHub. Terminal replies must
    // consume this exact reservation before any credential can be created.
    let reserved = state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &id,
            encode(&record)?,
            &entity.etag,
        )
        .await?;
    if early {
        return Ok(Json(EnrollmentPollResponse::SlowDown {
            interval: record.interval,
        }));
    }
    let device_code: String = state.auth.codec.open(&record.encrypted_device_code)?;
    let poll = if let Some(github) = &state.auth.github {
        github.poll_device_flow(&device_code).await?
    } else if state.settings.dev {
        DevicePoll::Authorized(String::new())
    } else {
        return Err(BrokerError::Unavailable);
    };
    let reply = match poll {
        DevicePoll::Pending => EnrollmentPollResponse::Pending {
            interval: record.interval,
        },
        DevicePoll::SlowDown => {
            record.interval = record.interval.saturating_add(5);
            record.next_poll_at = timestamp.saturating_add(record.interval);
            state
                .metadata_store
                .replace(
                    Table::Enrollments,
                    PLATFORM_PARTITION,
                    &id,
                    encode(&record)?,
                    &reserved.etag,
                )
                .await?;
            EnrollmentPollResponse::SlowDown {
                interval: record.interval,
            }
        }
        DevicePoll::Expired | DevicePoll::Denied => {
            delete_entity(&state, Table::Enrollments, &reserved).await?;
            if matches!(poll, DevicePoll::Denied) {
                EnrollmentPollResponse::Denied
            } else {
                EnrollmentPollResponse::Expired
            }
        }
        DevicePoll::Authorized(token) => {
            let user = if let Some(github) = &state.auth.github {
                github.consume_token(&token).await?
            } else if state.settings.dev {
                GitHubUser {
                    id: 1,
                    login: "dev-user".into(),
                    avatar_url: String::new(),
                }
            } else {
                return Err(BrokerError::Unavailable);
            };
            delete_entity(&state, Table::Enrollments, &reserved).await?;
            if record.expires_at <= now()? {
                return Ok(Json(EnrollmentPollResponse::Expired));
            }
            match auth::workspace_role(&state, &user, &record.workspace_id, Role::Operator).await {
                Ok(_) => {}
                Err(BrokerError::Forbidden) => return Ok(Json(EnrollmentPollResponse::Denied)),
                Err(error) => return Err(error),
            }
            let created = mint(
                &state,
                &record.workspace_id,
                CreateGatewayRequest {
                    name: record.gateway_name,
                    profile_id: record.profile_id,
                },
                user.id,
            )
            .await?;
            EnrollmentPollResponse::Authorized {
                credential: created.credential,
                workspace_id: created.workspace_id,
                gateway_id: created.gateway.id,
                profile_id: created.gateway.profile_id,
            }
        }
    };
    Ok(Json(reply))
}
