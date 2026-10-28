use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use azbridge_broker::{
    AppState, app,
    auth::{Session, now},
    error::BrokerError,
    models::{GatewayRecord, PLATFORM_PARTITION, ProfileRecord, WorkspaceRecord},
    reconciler::{reconcile_once, run_reconciler},
    relay::{FakeRelayManager, RelayManager},
    settings::{Args, CloudSettings, Settings},
    storage::Table,
};
use azbridge_broker_api::{
    Connection, Gateway, GitHubUser, Member, Profile, ProvisioningState, RelayRight,
    RelayTokenResponse, Role, Workspace,
};
use clap::Parser;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

fn state() -> AppState {
    AppState::new(Settings::from_args(Args::parse_from(["broker", "--dev"])).unwrap()).unwrap()
}

fn cookie(state: &AppState, id: u64) -> String {
    let session = Session {
        user: GitHubUser {
            id,
            login: format!("user-{id}"),
            avatar_url: String::new(),
        },
        csrf_token: "test-csrf".into(),
        expires_at: now().unwrap() + 3600,
    };
    format!(
        "azbridge_session={}",
        state.auth.codec.seal(&session).unwrap()
    )
}

async fn request(
    router: Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    csrf: bool,
    data: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if csrf {
        request = request.header("x-csrf-token", "test-csrf");
    }
    let body = if data.is_null() {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from(serde_json::to_vec(&data).unwrap())
    };
    let response = router.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let data = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, data)
}

async fn create_workspace(state: &AppState) -> Workspace {
    let (status, value) = request(
        app(state.clone()),
        "POST",
        "/api/v1/workspaces",
        Some(&cookie(state, 1)),
        true,
        json!({"name":"Office Lab","location":"westus"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    serde_json::from_value(value).unwrap()
}

async fn create_connection(state: &AppState, ws: &Workspace, name: &str) -> Connection {
    let (status, value) = request(
        app(state.clone()),
        "POST",
        &format!("/api/v1/workspaces/{}/connections", ws.id),
        Some(&cookie(state, 1)),
        true,
        json!({"name":name}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    serde_json::from_value(value).unwrap()
}

async fn member(state: &AppState, ws: &Workspace, id: u64, role: Role) {
    let member = Member {
        user: GitHubUser {
            id,
            login: id.to_string(),
            avatar_url: String::new(),
        },
        role,
    };
    state
        .metadata_store
        .insert(
            Table::Members,
            &ws.id,
            &id.to_string(),
            serde_json::to_value(member).unwrap(),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::UserWorkspaces,
            &id.to_string(),
            &ws.id,
            json!({"workspaceId":ws.id}),
        )
        .await
        .unwrap();
}

#[derive(Default)]
struct ControlledRelay {
    fake: FakeRelayManager,
    fail_namespace: AtomicBool,
    fail_connection: AtomicBool,
    pending_namespace: AtomicBool,
    pending_connection: AtomicBool,
    pending_delete: AtomicBool,
    fail_delete: AtomicBool,
    fail_rotate: AtomicBool,
    reported_failed: AtomicBool,
    terminal: AtomicBool,
    namespace_calls: AtomicUsize,
    connection_calls: AtomicUsize,
    namespace_deletes: AtomicUsize,
}

#[async_trait]
impl RelayManager for ControlledRelay {
    async fn ensure_namespace(
        &self,
        namespace: &str,
        location: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        self.namespace_calls.fetch_add(1, Ordering::SeqCst);
        if self.reported_failed.load(Ordering::SeqCst) {
            return Ok(ProvisioningState::Failed);
        }
        if self.fail_namespace.load(Ordering::SeqCst) {
            return Err(if self.terminal.load(Ordering::SeqCst) {
                BrokerError::BadRequest
            } else {
                BrokerError::Internal(anyhow::anyhow!("secret=do-not-disclose"))
            });
        }
        if self.pending_namespace.load(Ordering::SeqCst) {
            return Ok(ProvisioningState::Provisioning);
        }
        self.fake.ensure_namespace(namespace, location).await
    }
    async fn delete_namespace(&self, namespace: &str) -> Result<bool, BrokerError> {
        self.namespace_deletes.fetch_add(1, Ordering::SeqCst);
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        if self.pending_delete.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.fake.delete_namespace(namespace).await
    }
    async fn ensure_connection(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        self.connection_calls.fetch_add(1, Ordering::SeqCst);
        if self.reported_failed.load(Ordering::SeqCst) {
            return Ok(ProvisioningState::Failed);
        }
        if self.fail_connection.load(Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        if self.pending_connection.load(Ordering::SeqCst) {
            return Ok(ProvisioningState::Pending);
        }
        self.fake.ensure_connection(namespace, name).await
    }
    async fn delete_connection(&self, namespace: &str, name: &str) -> Result<bool, BrokerError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        if self.pending_delete.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.fake.delete_connection(namespace, name).await
    }
    async fn rotate_keys(&self, namespace: &str, name: &str) -> Result<(), BrokerError> {
        if self.fail_rotate.load(Ordering::SeqCst) {
            return Err(BrokerError::Internal(anyhow::anyhow!(
                "secret=never-return"
            )));
        }
        self.fake.rotate_keys(namespace, name).await
    }
    async fn issue_token(
        &self,
        namespace: &str,
        name: &str,
        right: RelayRight,
    ) -> Result<RelayTokenResponse, BrokerError> {
        self.fake.issue_token(namespace, name, right).await
    }
}

#[tokio::test]
async fn workspace_development_login_creates_ready_server_owned_namespace_and_creator_membership() {
    let state = state();
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/dev")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let (status, me) = request(
        app(state.clone()),
        "GET",
        "/api/v1/me",
        Some(cookie),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let create_request = Request::builder().method("POST").uri("/api/v1/workspaces")
        .header("cookie", cookie)
        .header("x-csrf-token", me["csrfToken"].as_str().unwrap())
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"A Very Long Display Name","location":"westus","namespaceName":"evil-controlled-resource"}"#)).unwrap();
    let response = app(state.clone()).oneshot(create_request).await.unwrap();
    assert_eq!(response.status(), 201);
    let ws: Workspace =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(ws.state, ProvisioningState::Ready);
    assert!(Uuid::parse_str(&ws.id).is_ok());
    assert!(ws.namespace_name.starts_with("azb-a-very-long-d-"));
    assert!(ws.namespace_name.len() <= 50);
    assert_ne!(ws.namespace_name, "evil-controlled-resource");
    let entity = state
        .metadata_store
        .get(Table::Members, &ws.id, "1")
        .await
        .unwrap()
        .unwrap();
    let member: Member = serde_json::from_value(entity.data).unwrap();
    assert_eq!(member.role, Role::Admin);
    assert!(
        state
            .metadata_store
            .get(Table::UserWorkspaces, "1", &ws.id)
            .await
            .unwrap()
            .is_some()
    );
    let (_, list) = request(
        app(state.clone()),
        "GET",
        "/api/v1/workspaces",
        Some(cookie),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn workspace_authentication_csrf_roles_and_cross_workspace_access_are_enforced() {
    let state = state();
    let ws = create_workspace(&state).await;
    member(&state, &ws, 2, Role::Reader).await;
    member(&state, &ws, 3, Role::Operator).await;
    let path = format!("/api/v1/workspaces/{}", ws.id);
    assert_eq!(
        request(app(state.clone()), "GET", &path, None, false, Value::Null)
            .await
            .0,
        401
    );
    assert_eq!(
        request(
            app(state.clone()),
            "GET",
            &path,
            Some(&cookie(&state, 99)),
            false,
            Value::Null
        )
        .await
        .0,
        403
    );
    for id in [2, 3] {
        assert_eq!(
            request(
                app(state.clone()),
                "GET",
                &path,
                Some(&cookie(&state, id)),
                false,
                Value::Null
            )
            .await
            .0,
            200
        );
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                &format!("{path}/connections"),
                Some(&cookie(&state, id)),
                true,
                json!({"name":"relay"})
            )
            .await
            .0,
            403
        );
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                "/api/v1/workspaces",
                Some(&cookie(&state, id)),
                true,
                json!({"name":"Other","location":"eastus"})
            )
            .await
            .0,
            403
        );
    }
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{path}/connections"),
            Some(&cookie(&state, 1)),
            false,
            json!({"name":"relay"})
        )
        .await
        .0,
        403
    );
    let other = create_workspace(&state).await;
    let conn = create_connection(&state, &other, "other-relay").await;
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{path}/connections/{}", conn.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        404
    );
    assert!(
        state
            .metadata_store
            .get(Table::Connections, &other.id, &conn.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{path}/connections/{}/rotate", conn.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        404
    );
    let (_, accessible) = request(
        app(state.clone()),
        "GET",
        "/api/v1/workspaces",
        Some(&cookie(&state, 2)),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(accessible.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn workspace_validation_rejects_invalid_names_locations_and_resource_ids() {
    let state = state();
    for body in [
        json!({"name":"","location":"westus"}),
        json!({"name":" bad ","location":"westus"}),
        json!({"name":"ok","location":"West US"}),
        json!({"name":"ok","location":""}),
        json!({"name":"ok","location":"../../subscriptions/other"}),
    ] {
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                "/api/v1/workspaces",
                Some(&cookie(&state, 1)),
                true,
                body
            )
            .await
            .0,
            400
        );
    }
    let ws = create_workspace(&state).await;
    for name in [
        "",
        "../admin",
        "with/slash",
        "with space",
        "-prefix",
        "suffix-",
    ] {
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                &format!("/api/v1/workspaces/{}/connections", ws.id),
                Some(&cookie(&state, 1)),
                true,
                json!({"name":name})
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        request(
            app(state.clone()),
            "GET",
            "/api/v1/workspaces/not-a-uuid",
            Some(&cookie(&state, 1)),
            false,
            Value::Null
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            app(state.clone()),
            "GET",
            &format!("/api/v1/workspaces/{}", Uuid::new_v4()),
            Some(&cookie(&state, 1)),
            false,
            Value::Null
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("/api/v1/workspaces/{}/connections/not-an-id", ws.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn workspace_members_lookup_indexes_last_admin_and_immediate_revocation() {
    let state = state();
    let ws = create_workspace(&state).await;
    let base = format!("/api/v1/workspaces/{}/members", ws.id);
    for method in ["DELETE", "PUT"] {
        let body = if method == "PUT" {
            json!({"role":"reader"})
        } else {
            Value::Null
        };
        assert_eq!(
            request(
                app(state.clone()),
                method,
                &format!("{base}/1"),
                Some(&cookie(&state, 1)),
                true,
                body
            )
            .await
            .0,
            409
        );
    }
    let (status, added) = request(
        app(state.clone()),
        "POST",
        &base,
        Some(&cookie(&state, 1)),
        true,
        json!({"login":"2","role":"admin"}),
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(added["user"]["id"], 2);
    let (_, listed) = request(
        app(state.clone()),
        "GET",
        &base,
        Some(&cookie(&state, 2)),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(
        request(
            app(state.clone()),
            "PUT",
            &format!("{base}/1"),
            Some(&cookie(&state, 2)),
            true,
            json!({"role":"reader"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            app(state.clone()),
            "PUT",
            &format!("{base}/2"),
            Some(&cookie(&state, 1)),
            true,
            json!({"role":"reader"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/1"),
            Some(&cookie(&state, 2)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    assert!(
        state
            .metadata_store
            .get(Table::UserWorkspaces, "1", &ws.id)
            .await
            .unwrap()
            .is_none()
    );
    // Bootstrap platform admin still has access, but an ordinary removed user does not.
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &base,
            Some(&cookie(&state, 2)),
            true,
            json!({"login":"3","role":"operator"})
        )
        .await
        .0,
        201
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/3"),
            Some(&cookie(&state, 2)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            app(state.clone()),
            "GET",
            &base,
            Some(&cookie(&state, 3)),
            false,
            Value::Null
        )
        .await
        .0,
        403
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/3"),
            Some(&cookie(&state, 2)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            app(state.clone()),
            "PUT",
            &format!("{base}/99"),
            Some(&cookie(&state, 2)),
            true,
            json!({"role":"reader"})
        )
        .await
        .0,
        404
    );
    for id in ["0", "01", "+1", "bad", "18446744073709551616"] {
        assert_eq!(
            request(
                app(state.clone()),
                "DELETE",
                &format!("{base}/{id}"),
                Some(&cookie(&state, 2)),
                true,
                Value::Null
            )
            .await
            .0,
            400
        );
    }
}

#[tokio::test]
async fn workspace_member_creation_is_index_repairing_and_dev_name_lookup_is_stable() {
    let state = state();
    let ws = create_workspace(&state).await;
    let path = format!("/api/v1/workspaces/{}/members", ws.id);
    let (status, first) = request(
        app(state.clone()),
        "POST",
        &path,
        Some(&cookie(&state, 1)),
        true,
        json!({"login":"Alice","role":"reader"}),
    )
    .await;
    assert_eq!(status, 201);
    let id = first["user"]["id"].as_u64().unwrap();
    let index = state
        .metadata_store
        .get(Table::UserWorkspaces, &id.to_string(), &ws.id)
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .delete(Table::UserWorkspaces, &id.to_string(), &ws.id, &index.etag)
        .await
        .unwrap();
    let (status, second) = request(
        app(state.clone()),
        "POST",
        &path,
        Some(&cookie(&state, 1)),
        true,
        json!({"login":"alice","role":"reader"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first, second);
    assert!(
        state
            .metadata_store
            .get(Table::UserWorkspaces, &id.to_string(), &ws.id)
            .await
            .unwrap()
            .is_some()
    );
    let index = state
        .metadata_store
        .get(Table::UserWorkspaces, &id.to_string(), &ws.id)
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .delete(Table::UserWorkspaces, &id.to_string(), &ws.id, &index.etag)
        .await
        .unwrap();
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::UserWorkspaces, &id.to_string(), &ws.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &path,
            Some(&cookie(&state, 1)),
            true,
            json!({"login":"alice","role":"admin"})
        )
        .await
        .0,
        409
    );
    for login in ["", "bad/name", "0"] {
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                &path,
                Some(&cookie(&state, 1)),
                true,
                json!({"login":login,"role":"reader"})
            )
            .await
            .0,
            400
        );
    }
    // A stale index never grants access.
    state
        .metadata_store
        .insert(Table::UserWorkspaces, "99", &ws.id, json!({}))
        .await
        .unwrap();
    let (_, visible) = request(
        app(state.clone()),
        "GET",
        "/api/v1/workspaces",
        Some(&cookie(&state, 99)),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(visible, json!([]));
}

#[tokio::test]
async fn workspace_mutation_reauthorizes_after_waiting_for_lock() {
    let state = state();
    let ws = create_workspace(&state).await;
    member(&state, &ws, 2, Role::Admin).await;
    let lock = state.mutation_lock.lock().await;
    let state_clone = state.clone();
    let path = format!("/api/v1/workspaces/{}/connections", ws.id);
    let pending = tokio::spawn(async move {
        request(
            app(state_clone.clone()),
            "POST",
            &path,
            Some(&cookie(&state_clone, 2)),
            true,
            json!({"name":"must-not-create"}),
        )
        .await
    });
    tokio::task::yield_now().await;
    let entity = state
        .metadata_store
        .get(Table::Members, &ws.id, "2")
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .delete(Table::Members, &ws.id, "2", &entity.etag)
        .await
        .unwrap();
    drop(lock);
    assert_eq!(pending.await.unwrap().0, 403);
    assert!(
        state
            .metadata_store
            .list(Table::Connections, &ws.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workspace_connections_are_unique_rotatable_audited_and_prevent_namespace_delete() {
    let state = state();
    let ws = create_workspace(&state).await;
    let connection = create_connection(&state, &ws, "relay-one").await;
    assert_eq!(connection.state, ProvisioningState::Ready);
    let base = format!("/api/v1/workspaces/{}", ws.id);
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{base}/connections"),
            Some(&cookie(&state, 1)),
            true,
            json!({"name":"RELAY-ONE"})
        )
        .await
        .0,
        409
    );
    let (_, list) = request(
        app(state.clone()),
        "GET",
        &format!("{base}/connections"),
        Some(&cookie(&state, 1)),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{base}/connections/{}/rotate", connection.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &base,
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        409
    );
    let (_, audit) = request(
        app(state.clone()),
        "GET",
        &format!("{base}/audit"),
        Some(&cookie(&state, 1)),
        false,
        Value::Null,
    )
    .await;
    for action in [
        "workspace.create",
        "workspace.ready",
        "connection.create",
        "connection.ready",
        "connection.rotate",
    ] {
        assert!(
            audit
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["action"] == action),
            "{audit}"
        );
    }
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["actor"] == "system")
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/connections/{}", connection.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &base,
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
}

#[tokio::test]
async fn workspace_delete_explains_dependencies_and_succeeds_after_profile_and_connection_cleanup()
{
    let state = state();
    let ws = create_workspace(&state).await;
    let connection = create_connection(&state, &ws, "in-use").await;
    let profile = profile(
        &state,
        &ws,
        Some(1),
        Some("LocalForward:\n- RelayName: in-use\n"),
    )
    .await;
    let base = format!("/api/v1/workspaces/{}", ws.id);
    let admin = cookie(&state, 1);
    let (status, error) = request(
        app(state.clone()),
        "DELETE",
        &base,
        Some(&admin),
        true,
        Value::Null,
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "workspace_not_empty");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("connections first")
    );
    assert_eq!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .unwrap()
            .data["workspace"]["state"],
        "ready"
    );
    let connection_path = format!("{base}/connections/{}", connection.id);
    let (status, error) = request(
        app(state.clone()),
        "DELETE",
        &connection_path,
        Some(&admin),
        true,
        Value::Null,
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "connection_in_use");
    for path in [
        format!("{base}/profiles/{}", profile.id),
        connection_path,
        base,
    ] {
        let (status, error) = request(
            app(state.clone()),
            "DELETE",
            &path,
            Some(&admin),
            true,
            Value::Null,
        )
        .await;
        assert_eq!(status, 204, "{error}");
    }
    assert!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .is_none()
    );
}

async fn profile(
    state: &AppState,
    ws: &Workspace,
    current: Option<u64>,
    yaml: Option<&str>,
) -> Profile {
    let profile = Profile {
        id: Uuid::new_v4().to_string(),
        name: "profile".into(),
        current_revision: current,
    };
    state
        .metadata_store
        .insert(
            Table::Profiles,
            &ws.id,
            &profile.id,
            serde_json::to_value(ProfileRecord {
                profile: profile.clone(),
                next_revision: 2,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    if let Some(yaml) = yaml {
        state
            .config_store
            .put_revision(&ws.id, &profile.id, "1", yaml)
            .await
            .unwrap();
    }
    profile
}

#[tokio::test]
async fn workspace_connection_delete_blocks_published_local_and_remote_dependencies_not_drafts() {
    for forward in ["LocalForward", "RemoteForward"] {
        let state = state();
        let ws = create_workspace(&state).await;
        let connection = create_connection(&state, &ws, "in-use").await;
        let yaml = format!("{forward}:\n  - RelayName: in-use\n");
        let profile = profile(&state, &ws, Some(1), Some(&yaml)).await;
        let path = format!("/api/v1/workspaces/{}/connections/{}", ws.id, connection.id);
        assert_eq!(
            request(
                app(state.clone()),
                "DELETE",
                &path,
                Some(&cookie(&state, 1)),
                true,
                Value::Null
            )
            .await
            .0,
            409
        );
        let entity = state
            .metadata_store
            .get(Table::Profiles, &ws.id, &profile.id)
            .await
            .unwrap()
            .unwrap();
        let mut data = entity.data;
        data["profile"]["currentRevision"] = Value::Null;
        state
            .metadata_store
            .replace(Table::Profiles, &ws.id, &profile.id, data, &entity.etag)
            .await
            .unwrap();
        assert_eq!(
            request(
                app(state.clone()),
                "DELETE",
                &path,
                Some(&cookie(&state, 1)),
                true,
                Value::Null
            )
            .await
            .0,
            204
        );
    }
}

#[tokio::test]
async fn workspace_connection_delete_fails_closed_on_missing_or_invalid_published_config() {
    for yaml in [None, Some("LocalForward: [not a forward]")] {
        let state = state();
        let ws = create_workspace(&state).await;
        let connection = create_connection(&state, &ws, "in-use").await;
        profile(&state, &ws, Some(1), yaml).await;
        assert_eq!(
            request(
                app(state.clone()),
                "DELETE",
                &format!("/api/v1/workspaces/{}/connections/{}", ws.id, connection.id),
                Some(&cookie(&state, 1)),
                true,
                Value::Null
            )
            .await
            .0,
            409
        );
    }
}

#[tokio::test]
async fn workspace_reconciler_recovers_pending_namespace_and_connection_without_reprovisioning_ready()
 {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    manager.pending_namespace.store(true, Ordering::SeqCst);
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    assert_eq!(ws.state, ProvisioningState::Provisioning);
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("/api/v1/workspaces/{}/connections", ws.id),
            Some(&cookie(&state, 1)),
            true,
            json!({"name":"not-yet"})
        )
        .await
        .0,
        409
    );
    manager.pending_namespace.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    let (status, ready) = request(
        app(state.clone()),
        "GET",
        &format!("/api/v1/workspaces/{}", ws.id),
        Some(&cookie(&state, 1)),
        false,
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(ready["state"], "ready");
    manager.pending_connection.store(true, Ordering::SeqCst);
    let connection = create_connection(&state, &ws, "pending").await;
    assert_eq!(connection.state, ProvisioningState::Pending);
    manager.pending_connection.store(false, Ordering::SeqCst);
    // New state value, same stores: no in-memory queue is needed after restart.
    let mut restarted = state.clone();
    restarted.mutation_lock = Arc::new(tokio::sync::Mutex::new(()));
    reconcile_once(&restarted).await.unwrap();
    let entity = state
        .metadata_store
        .get(Table::Connections, &ws.id, &connection.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entity.data["state"], "ready");
    let calls = manager.connection_calls.load(Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert_eq!(manager.connection_calls.load(Ordering::SeqCst), calls);
    assert_eq!(manager.namespace_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn workspace_cloud_errors_are_sanitized_retried_with_backoff_and_bounded() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    manager.fail_namespace.store(true, Ordering::SeqCst);
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    assert_eq!(ws.state, ProvisioningState::Failed);
    assert!(!ws.error.unwrap().contains("secret"));
    reconcile_once(&state).await.unwrap();
    assert_eq!(manager.namespace_calls.load(Ordering::SeqCst), 1);
    for expected in 2..=5 {
        let entity = state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .unwrap();
        let mut data = entity.data;
        data["retryAt"] = json!(0);
        state
            .metadata_store
            .replace(
                Table::Workspaces,
                PLATFORM_PARTITION,
                &ws.id,
                data,
                &entity.etag,
            )
            .await
            .unwrap();
        reconcile_once(&state).await.unwrap();
        assert_eq!(manager.namespace_calls.load(Ordering::SeqCst), expected);
    }
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
        .await
        .unwrap()
        .unwrap();
    let mut data = entity.data;
    data["retryAt"] = json!(0);
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &ws.id,
            data,
            &entity.etag,
        )
        .await
        .unwrap();
    reconcile_once(&state).await.unwrap();
    assert_eq!(manager.namespace_calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn workspace_client_errors_are_terminal_and_connection_transient_failures_recover() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    manager.fail_namespace.store(true, Ordering::SeqCst);
    manager.terminal.store(true, Ordering::SeqCst);
    state.relay_manager = manager.clone();
    let failed = create_workspace(&state).await;
    assert_eq!(failed.state, ProvisioningState::Failed);
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, &failed.id)
        .await
        .unwrap()
        .unwrap();
    let mut data = entity.data;
    data["retryAt"] = json!(0);
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &failed.id,
            data,
            &entity.etag,
        )
        .await
        .unwrap();
    reconcile_once(&state).await.unwrap();
    assert_eq!(manager.namespace_calls.load(Ordering::SeqCst), 1);
    manager.fail_namespace.store(false, Ordering::SeqCst);
    let ws = create_workspace(&state).await;
    manager.fail_connection.store(true, Ordering::SeqCst);
    let connection = create_connection(&state, &ws, "recover").await;
    assert_eq!(connection.state, ProvisioningState::Failed);
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!(
                "/api/v1/workspaces/{}/connections/{}/rotate",
                ws.id, connection.id
            ),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        409
    );
    let entity = state
        .metadata_store
        .get(Table::Connections, &ws.id, &connection.id)
        .await
        .unwrap()
        .unwrap();
    let mut data = entity.data;
    data["retryAt"] = json!(0);
    state
        .metadata_store
        .replace(
            Table::Connections,
            &ws.id,
            &connection.id,
            data,
            &entity.etag,
        )
        .await
        .unwrap();
    manager.fail_connection.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    let entity = state
        .metadata_store
        .get(Table::Connections, &ws.id, &connection.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entity.data["state"], "ready");
}

#[tokio::test]
async fn workspace_deleting_connection_resumes_and_namespace_waits_for_no_connections() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    let connection = create_connection(&state, &ws, "delete-later").await;
    manager.pending_delete.store(true, Ordering::SeqCst);
    let path = format!("/api/v1/workspaces/{}", ws.id);
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{path}/connections/{}", connection.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        202
    );
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &path,
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        409
    );
    assert_eq!(manager.namespace_deletes.load(Ordering::SeqCst), 0);
    manager.pending_delete.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .list(Table::Connections, &ws.id)
            .await
            .unwrap()
            .is_empty()
    );
    manager.pending_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &path,
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        202
    );
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entity.data["workspace"]["state"], "deleting");
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{path}/connections"),
            Some(&cookie(&state, 1)),
            true,
            json!({"name":"blocked"})
        )
        .await
        .0,
        409
    );
    manager.pending_delete.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn workspace_delete_removes_gateway_credentials_profiles_members_and_indexes_before_pending_cloud_delete()
 {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    member(&state, &ws, 2, Role::Admin).await;
    let profile = profile(&state, &ws, Some(1), Some("{}")).await;
    let gateway = GatewayRecord {
        gateway: Gateway {
            id: Uuid::new_v4().to_string(),
            name: "gw".into(),
            profile_id: profile.id.clone(),
            revoked: false,
            last_seen_at: None,
            applied_revision: None,
            version: None,
            hostname: None,
        },
        credential_hash: "not-a-real-credential".into(),
        created_by: 1,
    };
    state
        .metadata_store
        .insert(
            Table::Gateways,
            &ws.id,
            &gateway.gateway.id,
            serde_json::to_value(&gateway).unwrap(),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            &ws.id,
            &format!("{}:00000000000000000001", profile.id),
            json!({"revision":1,"createdAt":now().unwrap()}),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::Enrollments,
            PLATFORM_PARTITION,
            "workspace-enrollment",
            json!({"workspaceId":ws.id}),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::Enrollments,
            PLATFORM_PARTITION,
            "unrelated-oauth",
            json!({"expiresAt":now().unwrap()+300}),
        )
        .await
        .unwrap();
    manager.pending_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("/api/v1/workspaces/{}", ws.id),
            Some(&cookie(&state, 2)),
            true,
            Value::Null
        )
        .await
        .0,
        202
    );
    for table in [
        Table::Members,
        Table::Profiles,
        Table::Gateways,
        Table::RevisionMetadata,
    ] {
        assert!(
            state
                .metadata_store
                .list(table, &ws.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        state
            .metadata_store
            .get(
                Table::Enrollments,
                PLATFORM_PARTITION,
                "workspace-enrollment"
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .metadata_store
            .get(Table::Enrollments, PLATFORM_PARTITION, "unrelated-oauth")
            .await
            .unwrap()
            .is_some()
    );
    for id in ["1", "2"] {
        assert!(
            state
                .metadata_store
                .get(Table::UserWorkspaces, id, &ws.id)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .is_some()
    );
    manager.pending_delete.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn workspace_reconciler_restores_creator_membership_after_interrupted_creation() {
    let state = state();
    let id = Uuid::new_v4().to_string();
    let record = WorkspaceRecord {
        workspace: Workspace {
            id: id.clone(),
            name: "Interrupted".into(),
            namespace_name: "azb-interrupted-unique".into(),
            location: "westus".into(),
            state: ProvisioningState::Pending,
            error: None,
        },
        created_by: 2,
    };
    state
        .metadata_store
        .insert(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &id,
            serde_json::to_value(record).unwrap(),
        )
        .await
        .unwrap();
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::Members, &id, "2")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        state
            .metadata_store
            .get(Table::UserWorkspaces, "2", &id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        request(
            app(state.clone()),
            "GET",
            &format!("/api/v1/workspaces/{id}"),
            Some(&cookie(&state, 2)),
            false,
            Value::Null
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn workspace_reconciler_loop_stops_on_shutdown_signal_or_closed_channel() {
    let state = state();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(run_reconciler(state.clone(), receiver));
    sender.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    drop(sender);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_reconciler(state, receiver),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn workspace_production_location_allowlist_is_enforced_without_fallback() {
    let mut state = state();
    let settings = Arc::get_mut(&mut state.settings).unwrap();
    settings.dev = false;
    settings.cloud = Some(CloudSettings {
        github_client_id: "test-client".into(),
        github_client_secret: "test-only".into(),
        cookie_key: [7; 32],
        bootstrap_admin_ids: [1].into(),
        storage_account: "teststorage".into(),
        subscription_id: Uuid::new_v4().to_string(),
        relay_resource_group: "relay-test".into(),
        allowed_locations: ["westus".to_owned()].into(),
        azure_client_id: None,
    });
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            "/api/v1/workspaces",
            Some(&cookie(&state, 1)),
            true,
            json!({"name":"Disallowed","location":"eastus"})
        )
        .await
        .0,
        400
    );
    let ws = create_workspace(&state).await;
    assert_eq!(ws.location, "westus");
    assert_eq!(ws.state, ProvisioningState::Ready);
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("/api/v1/workspaces/{}/members", ws.id),
            Some(&cookie(&state, 1)),
            true,
            json!({"login":"alice","role":"reader"})
        )
        .await
        .0,
        503
    );
}

#[tokio::test]
async fn workspace_reported_terminal_cloud_state_does_not_retry_namespace_or_connection() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    state.relay_manager = manager.clone();
    manager.reported_failed.store(true, Ordering::SeqCst);
    let failed = create_workspace(&state).await;
    assert_eq!(failed.state, ProvisioningState::Failed);
    manager.reported_failed.store(false, Ordering::SeqCst);
    let ws = create_workspace(&state).await;
    manager.reported_failed.store(true, Ordering::SeqCst);
    let connection = create_connection(&state, &ws, "terminal").await;
    assert_eq!(connection.state, ProvisioningState::Failed);
    let namespace_calls = manager.namespace_calls.load(Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert_eq!(
        manager.namespace_calls.load(Ordering::SeqCst),
        namespace_calls
    );
    assert_eq!(manager.connection_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workspace_cloud_delete_and_rotate_errors_are_safe_and_delete_intent_survives() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    let connection = create_connection(&state, &ws, "temporary").await;
    let base = format!("/api/v1/workspaces/{}", ws.id);
    manager.fail_rotate.store(true, Ordering::SeqCst);
    let (status, error) = request(
        app(state.clone()),
        "POST",
        &format!("{base}/connections/{}/rotate", connection.id),
        Some(&cookie(&state, 1)),
        true,
        Value::Null,
    )
    .await;
    assert_eq!(status, 503);
    assert!(!error.to_string().contains("secret"));
    manager.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/connections/{}", connection.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        202
    );
    let entity = state
        .metadata_store
        .get(Table::Connections, &ws.id, &connection.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entity.data["state"], "deleting");
    assert!(entity.data["error"].as_str().unwrap().contains("retrying"));
    manager.fail_delete.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::Connections, &ws.id, &connection.id)
            .await
            .unwrap()
            .is_none()
    );
    manager.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &base,
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        202
    );
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entity.data["workspace"]["state"], "deleting");
    assert!(
        entity.data["workspace"]["error"]
            .as_str()
            .unwrap()
            .contains("retrying")
    );
    manager.fail_delete.store(false, Ordering::SeqCst);
    reconcile_once(&state).await.unwrap();
    assert!(
        state
            .metadata_store
            .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn workspace_actual_profile_publish_guards_current_connection_but_not_historical_revision() {
    let state = state();
    let ws = create_workspace(&state).await;
    let first = create_connection(&state, &ws, "first").await;
    let second = create_connection(&state, &ws, "second").await;
    let base = format!("/api/v1/workspaces/{}", ws.id);
    let (status, profile) = request(
        app(state.clone()),
        "POST",
        &format!("{base}/profiles"),
        Some(&cookie(&state, 1)),
        true,
        json!({"name":"published-profile"}),
    )
    .await;
    assert_eq!(status, 201);
    let profile_path = format!("{base}/profiles/{}", profile["id"].as_str().unwrap());
    for (revision, connection) in [(1, &first), (2, &second)] {
        let yaml = format!(
            "LocalForward:\n  - RelayName: {}\n    BindPort: 8080\n",
            connection.name
        );
        assert_eq!(
            request(
                app(state.clone()),
                "POST",
                &format!("{profile_path}/revisions"),
                Some(&cookie(&state, 1)),
                true,
                json!({"yaml":yaml})
            )
            .await
            .0,
            201
        );
        let (status, _) = request(
            app(state.clone()),
            "POST",
            &format!("{profile_path}/publish"),
            Some(&cookie(&state, 1)),
            true,
            json!({"revision":revision, "expectedCurrentRevision": if revision == 1 { None } else { Some(revision - 1) }}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            request(
                app(state.clone()),
                "DELETE",
                &format!("{base}/connections/{}", connection.id),
                Some(&cookie(&state, 1)),
                true,
                Value::Null
            )
            .await
            .0,
            409
        );
    }

    assert_eq!(
        request(
            app(state.clone()),
            "DELETE",
            &format!("{base}/connections/{}", first.id),
            Some(&cookie(&state, 1)),
            true,
            Value::Null
        )
        .await
        .0,
        204
    );
    // Rolling back to a historical revision whose connection was removed is forbidden.
    assert_eq!(
        request(
            app(state.clone()),
            "POST",
            &format!("{profile_path}/publish"),
            Some(&cookie(&state, 1)),
            true,
            json!({"revision":1, "expectedCurrentRevision":2})
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn workspace_periodic_reconciler_resumes_persisted_work_and_preserves_delete_safety_check() {
    let mut state = state();
    let manager = Arc::new(ControlledRelay::default());
    manager.pending_namespace.store(true, Ordering::SeqCst);
    state.relay_manager = manager.clone();
    let ws = create_workspace(&state).await;
    manager.pending_namespace.store(false, Ordering::SeqCst);
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(run_reconciler(state.clone(), receiver));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let entity = state
                .metadata_store
                .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
                .await
                .unwrap()
                .unwrap();
            if entity.data["workspace"]["state"] == "ready" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    sender.send(true).unwrap();
    task.await.unwrap();
    create_connection(&state, &ws, "must-not-delete-namespace").await;
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, &ws.id)
        .await
        .unwrap()
        .unwrap();
    let mut data = entity.data;
    data["workspace"]["state"] = json!("deleting");
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            &ws.id,
            data,
            &entity.etag,
        )
        .await
        .unwrap();
    assert!(matches!(
        reconcile_once(&state).await,
        Err(BrokerError::Conflict)
    ));
    assert_eq!(manager.namespace_deletes.load(Ordering::SeqCst), 0);
    assert_eq!(
        state
            .metadata_store
            .list(Table::Connections, &ws.id)
            .await
            .unwrap()
            .len(),
        1
    );
}
