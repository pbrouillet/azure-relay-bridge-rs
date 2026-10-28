//! Restart-safe, single-replica reconciliation of persisted provisioning intent.
//!
//! Transient provisioning errors retry at most five times with persisted exponential
//! backoff; rejected ARM requests and reported terminal cloud states do not retry.
//! Deletions retry until absence is confirmed. Metadata and credentials are removed,
//! while immutable config blobs and audit records remain as inaccessible history
//! (the storage trait deliberately has no blob-delete operation).
use std::time::Duration;

use azbridge_broker_api::{Connection, GitHubUser, Member, ProvisioningState, Role, Workspace};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::{
    AppState, auth,
    error::BrokerError,
    models::{PLATFORM_PARTITION, WorkspaceRecord, record_audit},
    storage::{Entity, Table},
    workspaces::{decode, delete_entity, encode, workspace_entity},
};

const MAX_ATTEMPTS: u64 = 5;

/// One bounded pass. Each workspace is serialized with API writes; failures do not
/// stop independent workspaces. Transient cloud failures are persisted with backoff.
pub async fn reconcile_once(state: &AppState) -> Result<(), BrokerError> {
    let workspaces = state
        .metadata_store
        .list(Table::Workspaces, PLATFORM_PARTITION)
        .await?;
    let mut failure = None;
    for entity in workspaces {
        let _lock = state.mutation_lock.lock().await;
        if let Err(error) = reconcile_workspace_locked(state, &entity.row_key).await {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

/// The receiver's `true` value or sender closure stops the loop, including an
/// in-flight pass. A dropped network future leaves persisted intent for restart.
pub async fn run_reconciler(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = interval.tick() => {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return; }
                    }
                    result = reconcile_once(&state) => {
                        if result.is_err() { tracing::warn!("workspace reconciliation pass failed"); }
                    }
                }
            }
        }
    }
}

fn should_retry(data: &Value, status: ProvisioningState) -> Result<bool, BrokerError> {
    if status != ProvisioningState::Failed {
        return Ok(true);
    }
    Ok(data["retryable"].as_bool().unwrap_or(false)
        && data["attempts"].as_u64().unwrap_or(0) < MAX_ATTEMPTS
        && data["retryAt"].as_u64().unwrap_or(u64::MAX) <= auth::now()?)
}

fn failure_data(data: &mut Value, error: &BrokerError) -> Result<(), BrokerError> {
    let attempts = data["attempts"].as_u64().unwrap_or(0).saturating_add(1);
    data["attempts"] = json!(attempts);
    data["retryable"] = json!(matches!(
        error,
        BrokerError::Unavailable | BrokerError::Internal(_)
    ));
    data["retryAt"] = json!(auth::now()?.saturating_add(30 * (1 << attempts.min(6))));
    Ok(())
}

async fn save_workspace(
    state: &AppState,
    entity: &Entity,
    data: Value,
    action: &str,
) -> Result<Entity, BrokerError> {
    let entity = state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &entity.row_key,
            data,
            &entity.etag,
        )
        .await?;
    record_audit(state, &entity.row_key, "system", action, &entity.row_key).await?;
    Ok(entity)
}

async fn repair_creator(
    state: &AppState,
    entity: &Entity,
    record: &WorkspaceRecord,
) -> Result<Entity, BrokerError> {
    if entity.data["creatorMembershipInitialized"].as_bool() == Some(true)
        || !matches!(
            record.workspace.state,
            ProvisioningState::Pending | ProvisioningState::Provisioning
        )
    {
        return Ok(entity.clone());
    }
    let user = record.created_by.to_string();
    if state
        .metadata_store
        .get(Table::Members, &entity.row_key, &user)
        .await?
        .is_none()
    {
        let member = Member {
            user: GitHubUser {
                id: record.created_by,
                login: user.clone(),
                avatar_url: String::new(),
            },
            role: Role::Admin,
        };
        state
            .metadata_store
            .insert(Table::Members, &entity.row_key, &user, encode(member)?)
            .await?;
    }
    if state
        .metadata_store
        .get(Table::UserWorkspaces, &user, &entity.row_key)
        .await?
        .is_none()
    {
        state
            .metadata_store
            .insert(
                Table::UserWorkspaces,
                &user,
                &entity.row_key,
                json!({"workspaceId": entity.row_key}),
            )
            .await?;
    }
    let mut data = entity.data.clone();
    data["creatorMembershipInitialized"] = json!(true);
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &entity.row_key,
            data,
            &entity.etag,
        )
        .await
}

pub(crate) async fn reconcile_workspace_locked(
    state: &AppState,
    ws: &str,
) -> Result<(), BrokerError> {
    let mut entity = match workspace_entity(state, ws).await {
        Ok(entity) => entity,
        Err(BrokerError::NotFound) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut record: WorkspaceRecord = decode(entity.data.clone())?;
    if record.workspace.state == ProvisioningState::Deleting {
        return delete_workspace_locked(state, &entity, &record.workspace).await;
    }
    entity = repair_creator(state, &entity, &record).await?;
    for member in state.metadata_store.list(Table::Members, ws).await? {
        let member: Member = decode(member.data)?;
        crate::workspaces::ensure_index(state, ws, member.user.id).await?;
    }
    if record.workspace.state != ProvisioningState::Ready
        && should_retry(&entity.data, record.workspace.state)?
    {
        record.workspace.state = ProvisioningState::Provisioning;
        record.workspace.error = None;
        let mut data = entity.data.clone();
        data["workspace"] = encode(&record.workspace)?;
        entity = save_workspace(state, &entity, data, "workspace.provisioning").await?;
        let result = state
            .relay_manager
            .ensure_namespace(&record.workspace.namespace_name, &record.workspace.location)
            .await;
        let mut data = entity.data.clone();
        match result {
            Ok(
                status @ (ProvisioningState::Ready
                | ProvisioningState::Provisioning
                | ProvisioningState::Pending),
            ) => {
                record.workspace.state = status;
                record.workspace.error = None;
                data["retryable"] = json!(false);
                if status == ProvisioningState::Ready {
                    data["attempts"] = json!(0);
                }
            }
            Ok(_) => {
                record.workspace.state = ProvisioningState::Failed;
                record.workspace.error = Some("Relay namespace provisioning failed".into());
                data["retryable"] = json!(false);
            }
            Err(error) => {
                record.workspace.state = ProvisioningState::Failed;
                record.workspace.error = Some("Relay namespace provisioning failed".into());
                failure_data(&mut data, &error)?;
            }
        }
        data["workspace"] = encode(&record.workspace)?;
        save_workspace(
            state,
            &entity,
            data,
            match record.workspace.state {
                ProvisioningState::Ready => "workspace.ready",
                ProvisioningState::Failed => "workspace.failed",
                _ => "workspace.pending",
            },
        )
        .await?;
    }
    if record.workspace.state == ProvisioningState::Ready {
        reconcile_connections_locked(state, &record.workspace).await?;
    }
    Ok(())
}

async fn save_connection(
    state: &AppState,
    entity: &Entity,
    data: Value,
    action: &str,
) -> Result<Entity, BrokerError> {
    let entity = state
        .metadata_store
        .replace(
            Table::Connections,
            &entity.partition_key,
            &entity.row_key,
            data,
            &entity.etag,
        )
        .await?;
    record_audit(
        state,
        &entity.partition_key,
        "system",
        action,
        &entity.row_key,
    )
    .await?;
    Ok(entity)
}

pub(crate) async fn reconcile_connections_locked(
    state: &AppState,
    workspace: &Workspace,
) -> Result<(), BrokerError> {
    for mut entity in state
        .metadata_store
        .list(Table::Connections, &workspace.id)
        .await?
    {
        let mut connection: Connection = decode(entity.data.clone())?;
        if connection.state == ProvisioningState::Ready {
            continue;
        }
        if connection.state == ProvisioningState::Deleting {
            // Re-check published references before every destructive cloud call.
            if crate::workspaces::connection_in_use(state, &workspace.id, &connection.name).await? {
                continue;
            }
            match state
                .relay_manager
                .delete_connection(&workspace.namespace_name, &connection.name)
                .await
            {
                Ok(true) => {
                    record_audit(
                        state,
                        &workspace.id,
                        "system",
                        "connection.deleted",
                        &connection.id,
                    )
                    .await?;
                    delete_entity(state, Table::Connections, &entity).await?;
                }
                Ok(false) => {}
                Err(_) => {
                    connection.error = Some("Relay connection deletion failed; retrying".into());
                    let mut data = entity.data.clone();
                    data["error"] = encode(&connection.error)?;
                    save_connection(state, &entity, data, "connection.delete_failed").await?;
                }
            }
            continue;
        }
        if !should_retry(&entity.data, connection.state)? {
            continue;
        }
        connection.state = ProvisioningState::Provisioning;
        connection.error = None;
        let mut data = entity.data.clone();
        data["state"] = encode(connection.state)?;
        data["error"] = Value::Null;
        entity = save_connection(state, &entity, data, "connection.provisioning").await?;
        let result = state
            .relay_manager
            .ensure_connection(&workspace.namespace_name, &connection.name)
            .await;
        let mut data = entity.data.clone();
        match result {
            Ok(
                status @ (ProvisioningState::Ready
                | ProvisioningState::Pending
                | ProvisioningState::Provisioning),
            ) => {
                connection.state = status;
                connection.error = None;
                data["retryable"] = json!(false);
                if status == ProvisioningState::Ready {
                    data["attempts"] = json!(0);
                }
            }
            Ok(_) => {
                connection.state = ProvisioningState::Failed;
                connection.error = Some("Relay connection provisioning failed".into());
                data["retryable"] = json!(false);
            }
            Err(error) => {
                connection.state = ProvisioningState::Failed;
                connection.error = Some("Relay connection provisioning failed".into());
                failure_data(&mut data, &error)?;
            }
        }
        data["state"] = encode(connection.state)?;
        data["error"] = encode(connection.error)?;
        save_connection(
            state,
            &entity,
            data,
            match connection.state {
                ProvisioningState::Ready => "connection.ready",
                ProvisioningState::Failed => "connection.failed",
                _ => "connection.pending",
            },
        )
        .await?;
    }
    Ok(())
}

async fn delete_workspace_locked(
    state: &AppState,
    entity: &Entity,
    workspace: &Workspace,
) -> Result<(), BrokerError> {
    if !state
        .metadata_store
        .list(Table::Connections, &workspace.id)
        .await?
        .is_empty()
    {
        return Err(BrokerError::Conflict);
    }
    // Remove credentials before deleting cloud resources. All gateway APIs must
    // additionally reject a Deleting workspace while this cleanup is in flight.
    for table in [
        Table::Gateways,
        Table::Enrollments,
        Table::Profiles,
        Table::RevisionMetadata,
    ] {
        for entity in state.metadata_store.list(table, &workspace.id).await? {
            delete_entity(state, table, &entity).await?;
        }
    }
    // Enrollment handles are globally addressed, while credentials are workspace-scoped.
    for enrollment in state
        .metadata_store
        .list(Table::Enrollments, PLATFORM_PARTITION)
        .await?
    {
        if enrollment.data["workspaceId"].as_str() == Some(workspace.id.as_str()) {
            delete_entity(state, Table::Enrollments, &enrollment).await?;
        }
    }
    for member in state
        .metadata_store
        .list(Table::Members, &workspace.id)
        .await?
    {
        delete_entity(state, Table::Members, &member).await?;
    }
    if let Some(users) = entity.data["cleanupUserIds"].as_array() {
        for user in users {
            if let Some(user) = user.as_str()
                && let Some(index) = state
                    .metadata_store
                    .get(Table::UserWorkspaces, user, &workspace.id)
                    .await?
            {
                delete_entity(state, Table::UserWorkspaces, &index).await?;
            }
        }
    }
    match state
        .relay_manager
        .delete_namespace(&workspace.namespace_name)
        .await
    {
        Ok(true) => {
            record_audit(
                state,
                &workspace.id,
                "system",
                "workspace.deleted",
                &workspace.id,
            )
            .await?;
            delete_entity(state, Table::Workspaces, entity).await?;
        }
        Ok(false) => {}
        Err(_) => {
            let mut data = entity.data.clone();
            data["workspace"]["error"] = json!("Relay namespace deletion failed; retrying");
            save_workspace(state, entity, data, "workspace.delete_failed").await?;
        }
    }
    Ok(())
}
