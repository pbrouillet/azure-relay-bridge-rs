//! Workspace administration. Membership rows, not their lookup indexes, authorize access.
//!
//! Development accepts lowercase alphanumeric Azure location names without contacting
//! Azure. Production restricts them to `ALLOWED_LOCATIONS`. Namespace names are always
//! server-generated; no endpoint accepts customer resource IDs or namespace overrides.
pub use crate::models::record_audit;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use azbridge_broker_api::{
    AuditEntry, Connection, CreateConnectionRequest, CreateMemberRequest, CreateWorkspaceRequest,
    GitHubUser, Member, ProvisioningState, Role, UpdateMemberRequest, Workspace,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState, auth,
    error::BrokerError,
    models::{PLATFORM_PARTITION, ProfileRecord, WorkspaceRecord},
    storage::{Entity, Table},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/v1/workspaces/{ws}",
            get(get_workspace).delete(delete_workspace),
        )
        .route(
            "/api/v1/workspaces/{ws}/members",
            get(list_members).post(create_member),
        )
        .route(
            "/api/v1/workspaces/{ws}/members/{id}",
            put(update_member).delete(delete_member),
        )
        .route(
            "/api/v1/workspaces/{ws}/connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/api/v1/workspaces/{ws}/connections/{id}",
            delete(delete_connection),
        )
        .route(
            "/api/v1/workspaces/{ws}/connections/{id}/rotate",
            post(rotate_connection),
        )
        .route("/api/v1/workspaces/{ws}/audit", get(list_audit))
}

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, BrokerError> {
    serde_json::from_value(value).map_err(|error| BrokerError::Internal(error.into()))
}

pub(crate) fn encode(value: impl Serialize) -> Result<Value, BrokerError> {
    serde_json::to_value(value).map_err(|error| BrokerError::Internal(error.into()))
}

pub(crate) fn uuid(value: &str) -> Result<(), BrokerError> {
    if Uuid::parse_str(value).is_err() {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

fn valid_name(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || " -_.".contains(c))
}

fn valid_connection_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value.ends_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

pub(crate) async fn workspace_entity(state: &AppState, ws: &str) -> Result<Entity, BrokerError> {
    uuid(ws)?;
    state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, ws)
        .await?
        .ok_or(BrokerError::NotFound)
}

pub(crate) async fn active_workspace(
    state: &AppState,
    ws: &str,
) -> Result<WorkspaceRecord, BrokerError> {
    let record: WorkspaceRecord = decode(workspace_entity(state, ws).await?.data)?;
    if record.workspace.id != ws {
        return Err(BrokerError::NotFound);
    }
    if record.workspace.state == ProvisioningState::Deleting {
        return Err(BrokerError::Conflict);
    }
    Ok(record)
}

pub(crate) async fn delete_entity(
    state: &AppState,
    table: Table,
    entity: &Entity,
) -> Result<(), BrokerError> {
    state
        .metadata_store
        .delete(table, &entity.partition_key, &entity.row_key, &entity.etag)
        .await
}

pub(crate) async fn ensure_index(
    state: &AppState,
    ws: &str,
    user_id: u64,
) -> Result<(), BrokerError> {
    let user = user_id.to_string();
    if state
        .metadata_store
        .get(Table::UserWorkspaces, &user, ws)
        .await?
        .is_none()
    {
        match state
            .metadata_store
            .insert(Table::UserWorkspaces, &user, ws, json!({"workspaceId": ws}))
            .await
        {
            Ok(_) | Err(BrokerError::Conflict) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn remove_index(state: &AppState, ws: &str, user: &str) -> Result<(), BrokerError> {
    if let Some(entity) = state
        .metadata_store
        .get(Table::UserWorkspaces, user, ws)
        .await?
    {
        delete_entity(state, Table::UserWorkspaces, &entity).await?;
    }
    Ok(())
}

async fn list_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Workspace>>, BrokerError> {
    let session = auth::current_session(&state, &headers)?;
    Ok(Json(
        auth::accessible_workspaces(&state, &session.user).await?,
    ))
}

async fn create_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<Workspace>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::require_platform_admin(&state, &headers, true).await?;
    if !valid_name(&request.name, 100)
        || request.location.is_empty()
        || request.location.len() > 64
        || !request
            .location
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        || (!state.settings.dev
            && !state
                .settings
                .cloud
                .as_ref()
                .ok_or(BrokerError::Unavailable)?
                .allowed_locations
                .contains(&request.location))
    {
        return Err(BrokerError::BadRequest);
    }
    let slug: String = request
        .name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(13)
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "workspace" } else { slug };
    let id = Uuid::new_v4().to_string();
    let namespace = format!("azb-{slug}-{}", Uuid::new_v4().simple());
    let record = WorkspaceRecord {
        workspace: Workspace {
            id: id.clone(),
            name: request.name,
            namespace_name: namespace,
            location: request.location,
            state: ProvisioningState::Pending,
            error: None,
        },
        created_by: session.user.id,
    };
    state
        .metadata_store
        .insert(Table::Workspaces, PLATFORM_PARTITION, &id, encode(&record)?)
        .await?;
    let member = Member {
        user: session.user.clone(),
        role: Role::Admin,
    };
    state
        .metadata_store
        .insert(
            Table::Members,
            &id,
            &member.user.id.to_string(),
            encode(&member)?,
        )
        .await?;
    ensure_index(&state, &id, member.user.id).await?;
    record_audit(
        &state,
        &id,
        &member.user.id.to_string(),
        "workspace.create",
        &id,
    )
    .await?;
    let entity = workspace_entity(&state, &id).await?;
    let mut data = entity.data.clone();
    data["creatorMembershipInitialized"] = json!(true);
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &id,
            data,
            &entity.etag,
        )
        .await?;
    crate::reconciler::reconcile_workspace_locked(&state, &id).await?;
    let updated: WorkspaceRecord = decode(workspace_entity(&state, &id).await?.data)?;
    Ok((StatusCode::CREATED, Json(updated.workspace)))
}

async fn get_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
) -> Result<Json<Workspace>, BrokerError> {
    auth::authorize(&state, &headers, &ws, Role::Reader, false).await?;
    Ok(Json(
        decode::<WorkspaceRecord>(workspace_entity(&state, &ws).await?.data)?.workspace,
    ))
}

async fn delete_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    let entity = workspace_entity(&state, &ws).await?;
    let mut record: WorkspaceRecord = decode(entity.data.clone())?;
    if !state
        .metadata_store
        .list(Table::Connections, &ws)
        .await?
        .is_empty()
    {
        return Err(BrokerError::WorkspaceNotEmpty);
    }
    if record.workspace.state != ProvisioningState::Deleting {
        record.workspace.state = ProvisioningState::Deleting;
        record.workspace.error = None;
        let mut data = encode(record)?;
        let users: Vec<String> = state
            .metadata_store
            .list(Table::Members, &ws)
            .await?
            .into_iter()
            .map(|e| e.row_key)
            .collect();
        // Keep the cleanup journal until namespace deletion completes, including across restarts.
        data["cleanupUserIds"] = encode(users)?;
        state
            .metadata_store
            .replace(
                Table::Workspaces,
                PLATFORM_PARTITION,
                &ws,
                data,
                &entity.etag,
            )
            .await?;
        record_audit(
            &state,
            &ws,
            &session.user.id.to_string(),
            "workspace.delete",
            &ws,
        )
        .await?;
    }
    crate::reconciler::reconcile_workspace_locked(&state, &ws).await?;
    Ok(
        if state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws)
            .await?
            .is_some()
        {
            StatusCode::ACCEPTED
        } else {
            StatusCode::NO_CONTENT
        },
    )
}

async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Member>>, BrokerError> {
    auth::authorize(&state, &headers, &ws, Role::Reader, false).await?;
    active_workspace(&state, &ws).await?;
    Ok(Json(
        state
            .metadata_store
            .list(Table::Members, &ws)
            .await?
            .into_iter()
            .map(|e| decode(e.data))
            .collect::<Result<_, _>>()?,
    ))
}

async fn lookup_user(state: &AppState, login: &str) -> Result<GitHubUser, BrokerError> {
    if login.is_empty()
        || login.len() > 39
        || !login
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(BrokerError::BadRequest);
    }
    if state.settings.dev {
        let login = login.to_ascii_lowercase();
        let id = if login == "dev-admin" {
            1
        } else if let Ok(id) = login.parse::<u64>() {
            if id == 0 {
                return Err(BrokerError::BadRequest);
            }
            id
        } else {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(login.as_bytes());
            // Stay within JavaScript's exact integer range and reserve the bootstrap ID.
            (u64::from_be_bytes(
                digest[..8]
                    .try_into()
                    .map_err(|_| BrokerError::Unavailable)?,
            ) & ((1_u64 << 52) - 1))
                + 2
        };
        return Ok(GitHubUser {
            id,
            login,
            avatar_url: String::new(),
        });
    }
    state
        .auth
        .github
        .as_ref()
        .ok_or(BrokerError::Unavailable)?
        .lookup_user(login)
        .await
}

async fn create_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
    Json(request): Json<CreateMemberRequest>,
) -> Result<(StatusCode, Json<Member>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    active_workspace(&state, &ws).await?;
    let member = Member {
        user: lookup_user(&state, &request.login).await?,
        role: request.role,
    };
    let key = member.user.id.to_string();
    if let Some(existing) = state.metadata_store.get(Table::Members, &ws, &key).await? {
        let existing: Member = decode(existing.data)?;
        if existing.role != member.role {
            return Err(BrokerError::Conflict);
        }
        ensure_index(&state, &ws, member.user.id).await?;
        return Ok((StatusCode::OK, Json(existing)));
    }
    state
        .metadata_store
        .insert(Table::Members, &ws, &key, encode(&member)?)
        .await?;
    ensure_index(&state, &ws, member.user.id).await?;
    record_audit(
        &state,
        &ws,
        &session.user.id.to_string(),
        "member.create",
        &key,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(member)))
}

fn member_id(id: &str) -> Result<(), BrokerError> {
    if !id.bytes().all(|c| c.is_ascii_digit()) || id.parse::<u64>().is_err() || id.starts_with('0')
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

async fn protect_last_admin(
    state: &AppState,
    ws: &str,
    member: &Member,
    role: Option<Role>,
) -> Result<(), BrokerError> {
    if member.role == Role::Admin && role != Some(Role::Admin) {
        let members = state.metadata_store.list(Table::Members, ws).await?;
        let mut admins = 0;
        for entity in members {
            if decode::<Member>(entity.data)?.role == Role::Admin {
                admins += 1;
            }
        }
        if admins <= 1 {
            return Err(BrokerError::Conflict);
        }
    }
    Ok(())
}

async fn update_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((ws, id)): Path<(String, String)>,
    Json(request): Json<UpdateMemberRequest>,
) -> Result<Json<Member>, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    active_workspace(&state, &ws).await?;
    member_id(&id)?;
    let entity = state
        .metadata_store
        .get(Table::Members, &ws, &id)
        .await?
        .ok_or(BrokerError::NotFound)?;
    let mut member: Member = decode(entity.data)?;
    protect_last_admin(&state, &ws, &member, Some(request.role)).await?;
    member.role = request.role;
    state
        .metadata_store
        .replace(Table::Members, &ws, &id, encode(&member)?, &entity.etag)
        .await?;
    ensure_index(&state, &ws, member.user.id).await?;
    record_audit(
        &state,
        &ws,
        &session.user.id.to_string(),
        "member.update",
        &id,
    )
    .await?;
    Ok(Json(member))
}

async fn delete_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    active_workspace(&state, &ws).await?;
    member_id(&id)?;
    if let Some(entity) = state.metadata_store.get(Table::Members, &ws, &id).await? {
        let member: Member = decode(entity.data.clone())?;
        protect_last_admin(&state, &ws, &member, None).await?;
        delete_entity(&state, Table::Members, &entity).await?;
        record_audit(
            &state,
            &ws,
            &session.user.id.to_string(),
            "member.delete",
            &id,
        )
        .await?;
    }
    remove_index(&state, &ws, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_connections(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
) -> Result<Json<Vec<Connection>>, BrokerError> {
    auth::authorize(&state, &headers, &ws, Role::Reader, false).await?;
    active_workspace(&state, &ws).await?;
    Ok(Json(
        state
            .metadata_store
            .list(Table::Connections, &ws)
            .await?
            .into_iter()
            .map(|e| decode(e.data))
            .collect::<Result<_, _>>()?,
    ))
}

async fn create_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
    Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<Connection>), BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    let workspace = active_workspace(&state, &ws).await?;
    if workspace.workspace.state != ProvisioningState::Ready {
        return Err(BrokerError::Conflict);
    }
    if !valid_connection_name(&request.name) {
        return Err(BrokerError::BadRequest);
    }
    for entity in state.metadata_store.list(Table::Connections, &ws).await? {
        if decode::<Connection>(entity.data)?
            .name
            .eq_ignore_ascii_case(&request.name)
        {
            return Err(BrokerError::Conflict);
        }
    }
    let connection = Connection {
        id: Uuid::new_v4().to_string(),
        name: request.name,
        state: ProvisioningState::Pending,
        error: None,
    };
    state
        .metadata_store
        .insert(
            Table::Connections,
            &ws,
            &connection.id,
            encode(&connection)?,
        )
        .await?;
    record_audit(
        &state,
        &ws,
        &session.user.id.to_string(),
        "connection.create",
        &connection.id,
    )
    .await?;
    crate::reconciler::reconcile_connections_locked(&state, &workspace.workspace).await?;
    let updated = state
        .metadata_store
        .get(Table::Connections, &ws, &connection.id)
        .await?
        .ok_or(BrokerError::NotFound)?;
    Ok((StatusCode::CREATED, Json(decode(updated.data)?)))
}

/// Only a profile's published revision protects a connection; old drafts do not.
pub(crate) async fn connection_in_use(
    state: &AppState,
    ws: &str,
    name: &str,
) -> Result<bool, BrokerError> {
    for entity in state.metadata_store.list(Table::Profiles, ws).await? {
        let record: ProfileRecord = decode(entity.data)?;
        if let Some(revision) = record.profile.current_revision {
            let yaml = state
                .config_store
                .get_revision(ws, &record.profile.id, &revision.to_string())
                .await?
                .ok_or(BrokerError::Conflict)?;
            let config: azbridge_config::Config =
                serde_yaml::from_str(&yaml).map_err(|_| BrokerError::Conflict)?;
            if config
                .local_forward
                .iter()
                .map(|forward| &forward.relay_name)
                .chain(
                    config
                        .remote_forward
                        .iter()
                        .map(|forward| &forward.relay_name),
                )
                .any(|relay| relay.eq_ignore_ascii_case(name))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn delete_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    let workspace = active_workspace(&state, &ws).await?;
    uuid(&id)?;
    let Some(entity) = state
        .metadata_store
        .get(Table::Connections, &ws, &id)
        .await?
    else {
        return Err(BrokerError::NotFound);
    };
    let mut connection: Connection = decode(entity.data.clone())?;
    if connection_in_use(&state, &ws, &connection.name).await? {
        return Err(BrokerError::ConnectionInUse);
    }
    if connection.state != ProvisioningState::Deleting {
        connection.state = ProvisioningState::Deleting;
        connection.error = None;
        state
            .metadata_store
            .replace(
                Table::Connections,
                &ws,
                &id,
                encode(&connection)?,
                &entity.etag,
            )
            .await?;
        record_audit(
            &state,
            &ws,
            &session.user.id.to_string(),
            "connection.delete",
            &id,
        )
        .await?;
    }
    crate::reconciler::reconcile_connections_locked(&state, &workspace.workspace).await?;
    Ok(
        if state
            .metadata_store
            .get(Table::Connections, &ws, &id)
            .await?
            .is_some()
        {
            StatusCode::ACCEPTED
        } else {
            StatusCode::NO_CONTENT
        },
    )
}

async fn rotate_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((ws, id)): Path<(String, String)>,
) -> Result<StatusCode, BrokerError> {
    let _lock = state.mutation_lock.lock().await;
    let session = auth::authorize(&state, &headers, &ws, Role::Admin, true).await?;
    let workspace = active_workspace(&state, &ws).await?;
    uuid(&id)?;
    let connection: Connection = decode(
        state
            .metadata_store
            .get(Table::Connections, &ws, &id)
            .await?
            .ok_or(BrokerError::NotFound)?
            .data,
    )?;
    if workspace.workspace.state != ProvisioningState::Ready
        || connection.state != ProvisioningState::Ready
    {
        return Err(BrokerError::Conflict);
    }
    // Write intent before ARM: even a partial key regeneration must leave an audit trail.
    record_audit(
        &state,
        &ws,
        &session.user.id.to_string(),
        "connection.rotate",
        &id,
    )
    .await?;
    state
        .relay_manager
        .rotate_keys(&workspace.workspace.namespace_name, &connection.name)
        .await
        .map_err(|_| BrokerError::Unavailable)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(ws): Path<String>,
) -> Result<Json<Vec<AuditEntry>>, BrokerError> {
    auth::authorize(&state, &headers, &ws, Role::Reader, false).await?;
    workspace_entity(&state, &ws).await?;
    let mut entries = state
        .metadata_store
        .list(Table::Audit, &ws)
        .await?
        .into_iter()
        .map(|e| decode(e.data))
        .collect::<Result<Vec<AuditEntry>, _>>()?;
    entries.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(Json(entries))
}
