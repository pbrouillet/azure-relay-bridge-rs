//! GitHub authentication establishes broker sessions, never Azure credentials.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use azbridge_broker_api::{GitHubUser, MeResponse, Member, Role, Workspace};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode as HttpStatus};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{sync::Mutex, time::Instant};
use url::Url;

use crate::{
    AppState,
    error::BrokerError,
    models::{PLATFORM_PARTITION, WorkspaceRecord},
    settings::Settings,
    storage::Table,
};

const SESSION_COOKIE: &str = "azbridge_session";
const CHALLENGE_COOKIE: &str = "azbridge_oauth";
const SESSION_SECONDS: u64 = 8 * 60 * 60;
const OAUTH_SECONDS: u64 = 10 * 60;

mod device;

pub fn now() -> Result<u64, BrokerError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| BrokerError::Internal(error.into()))
}

pub fn random_secret() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

pub fn constant_time_equal(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

/// Also encrypts short-lived device codes stored in the enrollment table.
pub struct SessionCodec {
    cipher: Aes256Gcm,
}

impl SessionCodec {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
        }
    }

    pub fn seal<T: Serialize>(&self, value: &T) -> Result<String, BrokerError> {
        let nonce = rand::random::<[u8; 12]>();
        let plaintext =
            serde_json::to_vec(value).map_err(|error| BrokerError::Internal(error.into()))?;
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .map_err(|_| BrokerError::Internal(anyhow::anyhow!("session encryption failed")))?;
        let mut bytes = nonce.to_vec();
        bytes.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn open<T: DeserializeOwned>(&self, value: &str) -> Result<T, BrokerError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| BrokerError::Unauthorized)?;
        if bytes.len() < 28 {
            return Err(BrokerError::Unauthorized);
        }
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
            .map_err(|_| BrokerError::Unauthorized)?;
        serde_json::from_slice(&plaintext).map_err(|_| BrokerError::Unauthorized)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub user: GitHubUser,
    pub csrf_token: String,
    pub expires_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Challenge {
    state: String,
    verifier: String,
    expires_at: u64,
}

pub struct AuthService {
    pub codec: SessionCodec,
    pub github: Option<Arc<GitHubClient>>,
    pub bootstrap_admin_ids: BTreeSet<u64>,
    rate_limits: Mutex<HashMap<&'static str, VecDeque<Instant>>>,
}

impl AuthService {
    pub fn from_settings(settings: &Settings) -> Result<Self, BrokerError> {
        if settings.dev {
            return Ok(Self {
                codec: SessionCodec::new(&rand::random()),
                github: None,
                bootstrap_admin_ids: BTreeSet::from([1]),
                rate_limits: Mutex::new(HashMap::new()),
            });
        }
        let cloud = settings.cloud.as_ref().ok_or(BrokerError::Unavailable)?;
        Ok(Self {
            codec: SessionCodec::new(&cloud.cookie_key),
            github: Some(Arc::new(GitHubClient::new(
                cloud.github_client_id.clone(),
                cloud.github_client_secret.clone(),
            )?)),
            bootstrap_admin_ids: cloud.bootstrap_admin_ids.clone(),
            rate_limits: Mutex::new(HashMap::new()),
        })
    }

    /// Bounded, process-wide limits for the single-replica MVP's public endpoints.
    pub async fn rate_limit(
        &self,
        bucket: &'static str,
        capacity: usize,
        window: Duration,
    ) -> Result<(), BrokerError> {
        let now = Instant::now();
        let mut limits = self.rate_limits.lock().await;
        let entries = limits.entry(bucket).or_default();
        while entries
            .front()
            .is_some_and(|at| now.duration_since(*at) >= window)
        {
            entries.pop_front();
        }
        if entries.len() >= capacity {
            return Err(BrokerError::TooManyRequests);
        }
        entries.push_back(now);
        Ok(())
    }
}

/// No Debug: the HTTP client and OAuth replies contain application/user secrets.
pub struct GitHubClient {
    client: Client,
    client_id: String,
    client_secret: String,
    web_url: Url,
    api_url: Url,
}

#[derive(Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub enum DevicePoll {
    Pending,
    SlowDown,
    Authorized(String),
    Expired,
    Denied,
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GitHubIdentity {
    id: u64,
    login: String,
    avatar_url: String,
}

impl GitHubClient {
    pub fn new(client_id: String, client_secret: String) -> Result<Self, BrokerError> {
        Self::with_endpoints(
            client_id,
            client_secret,
            Url::parse("https://github.com/")
                .map_err(|error| BrokerError::Internal(error.into()))?,
            Url::parse("https://api.github.com/")
                .map_err(|error| BrokerError::Internal(error.into()))?,
        )
    }

    pub(crate) fn with_endpoints(
        client_id: String,
        client_secret: String,
        web_url: Url,
        api_url: Url,
    ) -> Result<Self, BrokerError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .user_agent("azbridge-broker")
            .build()
            .map_err(|_| BrokerError::Unavailable)?;
        Ok(Self {
            client,
            client_id,
            client_secret,
            web_url,
            api_url,
        })
    }

    pub fn authorization_url(
        &self,
        callback: &str,
        state: &str,
        verifier: &str,
    ) -> Result<Url, BrokerError> {
        let mut url = self
            .web_url
            .join("login/oauth/authorize")
            .map_err(|_| BrokerError::Unavailable)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", callback)
            .append_pair("state", state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(url)
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        callback: &str,
        verifier: &str,
    ) -> Result<String, BrokerError> {
        let url = self
            .web_url
            .join("login/oauth/access_token")
            .map_err(|_| BrokerError::Unavailable)?;
        let response = self
            .client
            .post(url)
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", callback),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let reply: TokenReply = checked_json(response).await?;
        if reply.error.is_some() {
            return Err(BrokerError::Unauthorized);
        }
        reply
            .access_token
            .filter(|token| !token.is_empty())
            .ok_or(BrokerError::Unauthorized)
    }

    pub async fn start_device_flow(&self) -> Result<DeviceCode, BrokerError> {
        let url = self
            .web_url
            .join("login/device/code")
            .map_err(|_| BrokerError::Unavailable)?;
        let response = self
            .client
            .post(url)
            .header(header::ACCEPT, "application/json")
            .form(&[("client_id", &self.client_id)])
            .send()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let reply: DeviceCode = checked_json(response).await?;
        if reply.device_code.is_empty()
            || reply.user_code.is_empty()
            || reply.interval == 0
            || reply.interval > 60
            || reply.expires_in == 0
            || reply.expires_in > 3600
        {
            return Err(BrokerError::Unavailable);
        }
        let verification =
            Url::parse(&reply.verification_uri).map_err(|_| BrokerError::Unavailable)?;
        if verification.scheme() != "https" || verification.host_str() != Some("github.com") {
            return Err(BrokerError::Unavailable);
        }
        Ok(reply)
    }

    pub async fn poll_device_flow(&self, device_code: &str) -> Result<DevicePoll, BrokerError> {
        let url = self
            .web_url
            .join("login/oauth/access_token")
            .map_err(|_| BrokerError::Unavailable)?;
        let response = self
            .client
            .post(url)
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let reply: TokenReply = checked_json(response).await?;
        match (reply.error.as_deref(), reply.access_token) {
            (Some("authorization_pending"), _) => Ok(DevicePoll::Pending),
            (Some("slow_down"), _) => Ok(DevicePoll::SlowDown),
            (Some("expired_token"), _) => Ok(DevicePoll::Expired),
            (Some("access_denied"), _) => Ok(DevicePoll::Denied),
            (None, Some(token)) if !token.is_empty() => Ok(DevicePoll::Authorized(token)),
            (Some("incorrect_device_code" | "device_flow_disabled"), _) => {
                Err(BrokerError::BadRequest)
            }
            _ => Err(BrokerError::Unavailable),
        }
    }

    pub async fn user(&self, token: &str) -> Result<GitHubUser, BrokerError> {
        let url = self
            .api_url
            .join("user")
            .map_err(|_| BrokerError::Unavailable)?;
        self.identity(self.client.get(url).bearer_auth(token)).await
    }

    pub async fn lookup_user(&self, login: &str) -> Result<GitHubUser, BrokerError> {
        if login.is_empty()
            || login.len() > 39
            || !login
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(BrokerError::BadRequest);
        }
        let url = self
            .api_url
            .join(&format!("users/{login}"))
            .map_err(|_| BrokerError::Unavailable)?;
        self.identity(self.client.get(url)).await
    }

    async fn identity(&self, request: reqwest::RequestBuilder) -> Result<GitHubUser, BrokerError> {
        let response = request
            .header(header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let identity: GitHubIdentity = checked_json(response).await?;
        if identity.id == 0 || identity.login.is_empty() {
            return Err(BrokerError::Unavailable);
        }
        Ok(GitHubUser {
            id: identity.id,
            login: identity.login,
            avatar_url: identity.avatar_url,
        })
    }

    pub async fn revoke_token(&self, token: &str) -> Result<(), BrokerError> {
        let url = self
            .api_url
            .join(&format!("applications/{}/token", self.client_id))
            .map_err(|_| BrokerError::Unavailable)?;
        let response = self
            .client
            .delete(url)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .json(&serde_json::json!({"access_token": token}))
            .send()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        if response.status() != HttpStatus::NO_CONTENT {
            tracing::warn!(
                status = response.status().as_u16(),
                "GitHub token revocation failed"
            );
            return Err(BrokerError::Unavailable);
        }
        Ok(())
    }

    pub async fn consume_token(&self, token: &str) -> Result<GitHubUser, BrokerError> {
        let identity = self.user(token).await;
        // Revoke even if profile lookup failed; never persist a GitHub user token.
        self.revoke_token(token).await?;
        identity
    }
}

async fn checked_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, BrokerError> {
    match response.status() {
        HttpStatus::UNAUTHORIZED => return Err(BrokerError::Unauthorized),
        HttpStatus::FORBIDDEN => return Err(BrokerError::Forbidden),
        HttpStatus::NOT_FOUND => return Err(BrokerError::NotFound),
        status if !status.is_success() => {
            tracing::warn!(status = status.as_u16(), "GitHub API request failed");
            return Err(BrokerError::Unavailable);
        }
        _ => {}
    }
    if response
        .content_length()
        .is_some_and(|length| length > 64 * 1024)
    {
        return Err(BrokerError::Unavailable);
    }
    response.json().await.map_err(|_| BrokerError::Unavailable)
}

pub fn current_session(state: &AppState, headers: &HeaderMap) -> Result<Session, BrokerError> {
    let token = cookie(headers, SESSION_COOKIE)?;
    let session: Session = state.auth.codec.open(token)?;
    if session.expires_at <= now()? || session.user.id == 0 {
        return Err(BrokerError::Unauthorized);
    }
    Ok(session)
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, BrokerError> {
    let cookies = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or(BrokerError::Unauthorized)?;
    let mut matches = cookies.split(';').filter_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then_some(value)
    });
    let value = matches.next().ok_or(BrokerError::Unauthorized)?;
    if matches.next().is_some() {
        return Err(BrokerError::Unauthorized);
    }
    Ok(value)
}

pub fn require_csrf(
    state: &AppState,
    headers: &HeaderMap,
    session: &Session,
) -> Result<(), BrokerError> {
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(BrokerError::Forbidden)?;
    if !constant_time_equal(csrf, &session.csrf_token) {
        return Err(BrokerError::Forbidden);
    }
    check_origin(state, headers)
}

fn check_origin(state: &AppState, headers: &HeaderMap) -> Result<(), BrokerError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };
    let origin = origin.to_str().map_err(|_| BrokerError::Forbidden)?;
    if origin == state.settings.public_url.origin().ascii_serialization() {
        return Ok(());
    }
    if state.settings.dev {
        let url = Url::parse(origin).map_err(|_| BrokerError::Forbidden)?;
        let loopback = match url.host() {
            Some(url::Host::Domain("localhost")) => true,
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if loopback && matches!(url.scheme(), "http" | "https") {
            return Ok(());
        }
    }
    Err(BrokerError::Forbidden)
}

pub async fn is_platform_admin(state: &AppState, user: &GitHubUser) -> Result<bool, BrokerError> {
    if state.auth.bootstrap_admin_ids.contains(&user.id) {
        return Ok(true);
    }
    Ok(state
        .metadata_store
        .get(
            Table::PlatformAdmins,
            PLATFORM_PARTITION,
            &user.id.to_string(),
        )
        .await?
        .is_some())
}

pub async fn workspace_role(
    state: &AppState,
    user: &GitHubUser,
    workspace: &str,
    required: Role,
) -> Result<Role, BrokerError> {
    uuid::Uuid::parse_str(workspace).map_err(|_| BrokerError::BadRequest)?;
    if is_platform_admin(state, user).await? {
        return Ok(Role::Admin);
    }
    let entity = state
        .metadata_store
        .get(Table::Members, workspace, &user.id.to_string())
        .await?
        .ok_or(BrokerError::Forbidden)?;
    let member: Member =
        serde_json::from_value(entity.data).map_err(|error| BrokerError::Internal(error.into()))?;
    if member.user.id != user.id || !member.role.permits(required) {
        return Err(BrokerError::Forbidden);
    }
    Ok(member.role)
}

pub async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    workspace: &str,
    required: Role,
    mutating: bool,
) -> Result<Session, BrokerError> {
    let session = current_session(state, headers)?;
    if mutating {
        require_csrf(state, headers, &session)?;
    }
    workspace_role(state, &session.user, workspace, required).await?;
    Ok(session)
}

pub async fn require_platform_admin(
    state: &AppState,
    headers: &HeaderMap,
    mutating: bool,
) -> Result<Session, BrokerError> {
    let session = current_session(state, headers)?;
    if mutating {
        require_csrf(state, headers, &session)?;
    }
    if !is_platform_admin(state, &session.user).await? {
        return Err(BrokerError::Forbidden);
    }
    Ok(session)
}

pub async fn accessible_workspaces(
    state: &AppState,
    user: &GitHubUser,
) -> Result<Vec<Workspace>, BrokerError> {
    let entities = if is_platform_admin(state, user).await? {
        state
            .metadata_store
            .list(Table::Workspaces, PLATFORM_PARTITION)
            .await?
    } else {
        let index = state
            .metadata_store
            .list(Table::UserWorkspaces, &user.id.to_string())
            .await?;
        let mut entities = Vec::new();
        for entry in index {
            // An index is not authorization: re-check the authoritative membership.
            if state
                .metadata_store
                .get(Table::Members, &entry.row_key, &user.id.to_string())
                .await?
                .is_some()
                && let Some(workspace) = state
                    .metadata_store
                    .get(Table::Workspaces, PLATFORM_PARTITION, &entry.row_key)
                    .await?
            {
                entities.push(workspace);
            }
        }
        entities
    };
    entities
        .into_iter()
        .map(|entity| {
            serde_json::from_value::<WorkspaceRecord>(entity.data)
                .map(|record| record.workspace)
                .map_err(|error| BrokerError::Internal(error.into()))
        })
        .collect()
}

pub async fn cleanup_expired_enrollments(state: &AppState) -> Result<(), BrokerError> {
    let timestamp = now()?;
    for entity in state
        .metadata_store
        .list(Table::Enrollments, PLATFORM_PARTITION)
        .await?
    {
        let expiry = entity
            .data
            .get("expiresAt")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| BrokerError::Internal(anyhow::anyhow!("invalid enrollment expiry")))?;
        if expiry <= timestamp {
            match state
                .metadata_store
                .delete(
                    Table::Enrollments,
                    PLATFORM_PARTITION,
                    &entity.row_key,
                    &entity.etag,
                )
                .await
            {
                Ok(()) | Err(BrokerError::NotFound | BrokerError::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(device::routes())
        .route("/auth/github/login", get(login))
        .route("/auth/github/callback", get(callback))
        .route("/auth/logout", post(logout))
        .route("/auth/dev", post(dev_login))
        .route("/api/v1/me", get(me))
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, BrokerError> {
    let session = current_session(&state, &headers)?;
    let platform_admin = is_platform_admin(&state, &session.user).await?;
    let workspaces = accessible_workspaces(&state, &session.user).await?;
    if !platform_admin && workspaces.is_empty() {
        return Err(BrokerError::Forbidden);
    }
    let allowed_locations = if state.settings.dev {
        ["eastus", "westus", "westeurope", "uksouth"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        state
            .settings
            .cloud
            .as_ref()
            .ok_or(BrokerError::Unavailable)?
            .allowed_locations
            .iter()
            .cloned()
            .collect()
    };
    Ok(Json(MeResponse {
        user: session.user,
        platform_admin,
        csrf_token: session.csrf_token,
        workspaces,
        allowed_locations,
    }))
}

async fn login(State(state): State<AppState>) -> Result<Response, BrokerError> {
    let github = state.auth.github.as_ref().ok_or(BrokerError::NotFound)?;
    state
        .auth
        .rate_limit("oauth_login", 30, Duration::from_secs(60))
        .await?;
    let challenge = Challenge {
        state: random_secret(),
        verifier: random_secret(),
        expires_at: now()? + OAUTH_SECONDS,
    };
    state
        .metadata_store
        .insert(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &format!("oauth:{}", challenge.state),
            serde_json::json!({"expiresAt": challenge.expires_at, "kind": "oauth"}),
        )
        .await?;
    let callback = state
        .settings
        .public_url
        .join("auth/github/callback")
        .map_err(|_| BrokerError::Unavailable)?;
    let authorization =
        github.authorization_url(callback.as_str(), &challenge.state, &challenge.verifier)?;
    let mut response = Redirect::temporary(authorization.as_str()).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(
            &state,
            CHALLENGE_COOKIE,
            &state.auth.codec.seal(&challenge)?,
            OAUTH_SECONDS,
            "/auth/github",
        )?,
    );
    Ok(response)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, BrokerError> {
    if query.error.is_some() {
        return Err(BrokerError::Forbidden);
    }
    let code = query.code.ok_or(BrokerError::BadRequest)?;
    let returned_state = query.state.ok_or(BrokerError::BadRequest)?;
    let challenge: Challenge = state.auth.codec.open(cookie(&headers, CHALLENGE_COOKIE)?)?;
    if challenge.expires_at <= now()? || !constant_time_equal(&returned_state, &challenge.state) {
        return Err(BrokerError::Unauthorized);
    }
    let challenge_key = format!("oauth:{}", challenge.state);
    let record = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &challenge_key)
        .await?
        .ok_or(BrokerError::Unauthorized)?;
    state
        .metadata_store
        .delete(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &challenge_key,
            &record.etag,
        )
        .await?;
    let github = state.auth.github.as_ref().ok_or(BrokerError::NotFound)?;
    let callback = state
        .settings
        .public_url
        .join("auth/github/callback")
        .map_err(|_| BrokerError::Unavailable)?;
    let token = github
        .exchange_code(&code, callback.as_str(), &challenge.verifier)
        .await?;
    let user = github.consume_token(&token).await?;
    if !is_platform_admin(&state, &user).await?
        && accessible_workspaces(&state, &user).await?.is_empty()
    {
        return Err(BrokerError::Forbidden);
    }
    let mut response = Redirect::to("/").into_response();
    set_session(&state, &user, &mut response)?;
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(&state, CHALLENGE_COOKIE, "", 0, "/auth/github")?,
    );
    Ok(response)
}

async fn dev_login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, BrokerError> {
    if !state.settings.dev {
        return Err(BrokerError::NotFound);
    }
    check_origin(&state, &headers)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    set_session(
        &state,
        &GitHubUser {
            id: 1,
            login: "dev-admin".into(),
            avatar_url: String::new(),
        },
        &mut response,
    )?;
    Ok(response)
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, BrokerError> {
    let session = current_session(&state, &headers)?;
    require_csrf(&state, &headers, &session)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(&state, SESSION_COOKIE, "", 0, "/")?,
    );
    Ok(response)
}

fn set_session(
    state: &AppState,
    user: &GitHubUser,
    response: &mut Response,
) -> Result<(), BrokerError> {
    let session = Session {
        user: user.clone(),
        csrf_token: random_secret(),
        expires_at: now()? + SESSION_SECONDS,
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(
            state,
            SESSION_COOKIE,
            &state.auth.codec.seal(&session)?,
            SESSION_SECONDS,
            "/",
        )?,
    );
    Ok(())
}

fn cookie_header(
    state: &AppState,
    name: &str,
    value: &str,
    max_age: u64,
    path: &str,
) -> Result<HeaderValue, BrokerError> {
    let secure = if state.settings.public_url.scheme() == "https" {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
    ))
    .map_err(|_| BrokerError::Internal(anyhow::anyhow!("session cookie encoding failed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use azbridge_broker_api::ProvisioningState;
    use clap::Parser;
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header as request_header, method, path},
    };

    fn state() -> AppState {
        let settings = Settings::from_args(
            crate::settings::Args::try_parse_from(["broker", "--dev"]).unwrap(),
        )
        .unwrap();
        AppState::new(settings).unwrap()
    }

    fn user(id: u64) -> GitHubUser {
        GitHubUser {
            id,
            login: format!("user-{id}"),
            avatar_url: String::new(),
        }
    }

    fn session_headers(state: &AppState, id: u64, expires_at: u64) -> HeaderMap {
        let session = Session {
            user: user(id),
            csrf_token: "csrf-test".into(),
            expires_at,
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!(
                "{SESSION_COOKIE}={}",
                state.auth.codec.seal(&session).unwrap()
            )
            .parse()
            .unwrap(),
        );
        headers
    }

    async fn mock_client(server: &MockServer) -> GitHubClient {
        GitHubClient::with_endpoints(
            "client-id".into(),
            "test-secret".into(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn codec_roundtrip_uses_random_nonces_and_rejects_invalid_tokens() {
        let codec = SessionCodec::new(&[7; 32]);
        let first = codec.seal(&json!({"value": "private"})).unwrap();
        let second = codec.seal(&json!({"value": "private"})).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            codec.open::<serde_json::Value>(&first).unwrap(),
            json!({"value": "private"})
        );
        for invalid in ["not-base64!", "a", &URL_SAFE_NO_PAD.encode([0; 28])] {
            assert!(matches!(
                codec.open::<Session>(invalid),
                Err(BrokerError::Unauthorized)
            ));
        }
        assert!(matches!(
            SessionCodec::new(&[8; 32]).open::<serde_json::Value>(&first),
            Err(BrokerError::Unauthorized)
        ));
        assert!(matches!(
            codec.open::<Session>(&first),
            Err(BrokerError::Unauthorized)
        ));
        let mut tampered = URL_SAFE_NO_PAD.decode(first).unwrap();
        tampered[20] ^= 1;
        assert!(matches!(
            codec.open::<serde_json::Value>(&URL_SAFE_NO_PAD.encode(tampered)),
            Err(BrokerError::Unauthorized)
        ));
    }

    #[test]
    fn generated_secrets_time_and_comparisons() {
        let first = random_secret();
        assert_eq!(first.len(), 43);
        assert_ne!(first, random_secret());
        assert!(constant_time_equal(&first, &first));
        assert!(!constant_time_equal(&first, "wrong"));
        assert!(now().unwrap() > 0);
    }

    #[tokio::test]
    async fn production_auth_has_no_dev_login_and_uses_secure_cookies() {
        use crate::{
            settings::CloudSettings,
            storage::{InMemoryConfigStore, InMemoryMetadataStore},
        };
        let settings = Settings {
            dev: false,
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            public_url: Url::parse("https://broker.example.com/").unwrap(),
            static_dir: "missing-test-assets".into(),
            cloud: Some(CloudSettings {
                github_client_id: "test-app".into(),
                github_client_secret: "test-only-secret".into(),
                cookie_key: [9; 32],
                bootstrap_admin_ids: BTreeSet::from([1]),
                storage_account: "teststorage".into(),
                subscription_id: "12345678-1234-1234-1234-123456789012".into(),
                relay_resource_group: "test-relays".into(),
                allowed_locations: BTreeSet::from(["westus".into()]),
                azure_client_id: None,
            }),
        };
        let state = AppState::with_stores(
            settings,
            Arc::new(InMemoryMetadataStore::new()),
            Arc::new(InMemoryConfigStore::new()),
        )
        .unwrap();
        assert!(state.auth.github.is_some());
        let Json(current) = me(
            State(state.clone()),
            session_headers(&state, 1, now().unwrap() + 60),
        )
        .await
        .unwrap();
        assert_eq!(current.allowed_locations, ["westus"]);
        let cookie = cookie_header(&state, SESSION_COOKIE, "test-value", 60, "/").unwrap();
        assert!(cookie.to_str().unwrap().contains("; Secure"));
        let response = crate::app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/dev")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let mut invalid = Settings::from_args(
            crate::settings::Args::try_parse_from(["broker", "--dev"]).unwrap(),
        )
        .unwrap();
        invalid.dev = false;
        assert!(matches!(
            AuthService::from_settings(&invalid),
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn public_endpoint_budget_returns_explicit_rate_limit() {
        let state = state();
        state
            .auth
            .rate_limit("test", 1, Duration::from_secs(60))
            .await
            .unwrap();
        assert!(matches!(
            state
                .auth
                .rate_limit("test", 1, Duration::from_secs(60))
                .await,
            Err(BrokerError::TooManyRequests)
        ));
        state
            .auth
            .rate_limit("another", 1, Duration::from_secs(60))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn enrollment_cleanup_respects_expiry_and_reports_corrupt_records() {
        let state = state();
        state
            .metadata_store
            .insert(
                Table::Enrollments,
                PLATFORM_PARTITION,
                "expired",
                json!({"expiresAt":0}),
            )
            .await
            .unwrap();
        state
            .metadata_store
            .insert(
                Table::Enrollments,
                PLATFORM_PARTITION,
                "active",
                json!({"expiresAt":now().unwrap()+100}),
            )
            .await
            .unwrap();
        cleanup_expired_enrollments(&state).await.unwrap();
        assert!(
            state
                .metadata_store
                .get(Table::Enrollments, PLATFORM_PARTITION, "expired")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .metadata_store
                .get(Table::Enrollments, PLATFORM_PARTITION, "active")
                .await
                .unwrap()
                .is_some()
        );
        state
            .metadata_store
            .insert(Table::Enrollments, PLATFORM_PARTITION, "corrupt", json!({}))
            .await
            .unwrap();
        assert!(matches!(
            cleanup_expired_enrollments(&state).await,
            Err(BrokerError::Internal(_))
        ));
    }

    #[test]
    fn sessions_csrf_origin_and_cookie_duplicates_are_rejected() {
        let state = state();
        let mut headers = session_headers(&state, 1, now().unwrap() + 100);
        let session = current_session(&state, &headers).unwrap();
        assert_eq!(session.user.id, 1);
        assert!(matches!(
            require_csrf(&state, &headers, &session),
            Err(BrokerError::Forbidden)
        ));
        headers.insert("x-csrf-token", "wrong".parse().unwrap());
        assert!(matches!(
            require_csrf(&state, &headers, &session),
            Err(BrokerError::Forbidden)
        ));
        headers.insert("x-csrf-token", "csrf-test".parse().unwrap());
        headers.insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());
        require_csrf(&state, &headers, &session).unwrap();
        headers.insert(header::ORIGIN, "https://untrusted.example".parse().unwrap());
        assert!(matches!(
            require_csrf(&state, &headers, &session),
            Err(BrokerError::Forbidden)
        ));
        assert!(matches!(
            current_session(&state, &session_headers(&state, 1, now().unwrap() - 1)),
            Err(BrokerError::Unauthorized)
        ));
        let existing = headers[header::COOKIE].to_str().unwrap().to_owned();
        headers.insert(
            header::COOKIE,
            format!("{existing}; {existing}").parse().unwrap(),
        );
        assert!(matches!(
            current_session(&state, &headers),
            Err(BrokerError::Unauthorized)
        ));
        assert!(matches!(
            current_session(&state, &HeaderMap::new()),
            Err(BrokerError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn dev_session_me_logout_flow_has_csrf_and_no_cache() {
        let state = state();
        let app = crate::app(state);
        let response = app
            .clone()
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
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/me")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let me: MeResponse = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(me.platform_admin);
        assert_eq!(me.user.id, 1);
        assert_eq!(
            me.allowed_locations,
            ["eastus", "westus", "westeurope", "uksouth"]
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(header::COOKIE, cookie)
                    .header("x-csrf-token", &me.csrf_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
        assert!(
            response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
    }

    #[tokio::test]
    async fn workspace_authorization_is_authoritative_and_immediately_revocable() {
        let state = state();
        let id = uuid::Uuid::new_v4().to_string();
        let member = Member {
            user: user(2),
            role: Role::Reader,
        };
        let entity = state
            .metadata_store
            .insert(
                Table::Members,
                &id,
                "2",
                serde_json::to_value(member).unwrap(),
            )
            .await
            .unwrap();
        state
            .metadata_store
            .insert(Table::UserWorkspaces, "2", &id, json!({"workspaceId": id}))
            .await
            .unwrap();
        state
            .metadata_store
            .insert(
                Table::Workspaces,
                PLATFORM_PARTITION,
                &id,
                serde_json::to_value(WorkspaceRecord {
                    workspace: Workspace {
                        id: id.clone(),
                        name: "Office".into(),
                        namespace_name: "azb-office-123".into(),
                        location: "westus".into(),
                        state: ProvisioningState::Ready,
                        error: None,
                    },
                    created_by: 1,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            workspace_role(&state, &user(2), &id, Role::Reader)
                .await
                .unwrap(),
            Role::Reader
        );
        assert!(matches!(
            workspace_role(&state, &user(2), &id, Role::Operator).await,
            Err(BrokerError::Forbidden)
        ));
        assert_eq!(
            accessible_workspaces(&state, &user(2)).await.unwrap().len(),
            1
        );
        let headers = session_headers(&state, 2, now().unwrap() + 100);
        assert!(
            authorize(&state, &headers, &id, Role::Reader, false)
                .await
                .is_ok()
        );
        assert!(matches!(
            authorize(&state, &headers, &id, Role::Reader, true).await,
            Err(BrokerError::Forbidden)
        ));
        assert!(matches!(
            require_platform_admin(&state, &headers, false).await,
            Err(BrokerError::Forbidden)
        ));
        let admin_headers = session_headers(&state, 1, now().unwrap() + 100);
        assert!(
            require_platform_admin(&state, &admin_headers, false)
                .await
                .is_ok()
        );
        assert!(matches!(
            workspace_role(&state, &user(1), "invalid", Role::Reader).await,
            Err(BrokerError::BadRequest)
        ));
        state
            .metadata_store
            .delete(Table::Members, &id, "2", &entity.etag)
            .await
            .unwrap();
        assert!(matches!(
            workspace_role(&state, &user(2), &id, Role::Reader).await,
            Err(BrokerError::Forbidden)
        ));
        assert!(
            accessible_workspaces(&state, &user(2))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            me(State(state.clone()), headers).await,
            Err(BrokerError::Forbidden)
        ));
        state
            .metadata_store
            .insert(
                Table::PlatformAdmins,
                PLATFORM_PARTITION,
                "3",
                json!({"id": 3}),
            )
            .await
            .unwrap();
        assert!(is_platform_admin(&state, &user(3)).await.unwrap());
    }

    #[tokio::test]
    async fn github_web_exchange_uses_pkce_and_consumes_user_token() {
        let server = MockServer::start().await;
        let client = mock_client(&server).await;
        let authorization = client
            .authorization_url(
                "https://broker.test/auth/github/callback",
                "state",
                "verifier",
            )
            .unwrap();
        let pairs: std::collections::HashMap<_, _> = authorization.query_pairs().collect();
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(
            pairs["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier"))
        );
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .and(body_string_contains("code_verifier=verifier"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"access_token":"test-token"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(request_header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":1,"login":"admin","avatar_url":""})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/applications/client-id/token"))
            .and(request_header(
                "authorization",
                format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD.encode("client-id:test-secret")
                ),
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let token = client
            .exchange_code(
                "code",
                "https://broker.test/auth/github/callback",
                "verifier",
            )
            .await
            .unwrap();
        assert_eq!(client.consume_token(&token).await.unwrap().id, 1);
    }

    #[tokio::test]
    async fn device_start_and_every_poll_state_are_handled() {
        let server = MockServer::start().await;
        let client = mock_client(&server).await;
        Mock::given(method("POST")).and(path("/login/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code":"device", "user_code":"ABCD-EFGH", "verification_uri":"https://github.com/login/device", "expires_in":900, "interval":5
            }))).mount(&server).await;
        assert_eq!(client.start_device_flow().await.unwrap().interval, 5);
        for (error, kind) in [
            ("authorization_pending", 0),
            ("slow_down", 1),
            ("expired_token", 2),
            ("access_denied", 3),
            ("incorrect_device_code", 4),
            ("device_flow_disabled", 4),
            ("unexpected", 5),
        ] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/login/oauth/access_token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"error":error})))
                .mount(&server)
                .await;
            let reply = client.poll_device_flow("device").await;
            match kind {
                0 => assert!(matches!(reply, Ok(DevicePoll::Pending))),
                1 => assert!(matches!(reply, Ok(DevicePoll::SlowDown))),
                2 => assert!(matches!(reply, Ok(DevicePoll::Expired))),
                3 => assert!(matches!(reply, Ok(DevicePoll::Denied))),
                4 => assert!(matches!(reply, Err(BrokerError::BadRequest))),
                _ => assert!(matches!(reply, Err(BrokerError::Unavailable))),
            }
        }
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"access_token":"test-token"})),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            client.poll_device_flow("device").await,
            Ok(DevicePoll::Authorized(_))
        ));
    }

    #[tokio::test]
    async fn github_invalid_responses_and_revocation_failures_are_explicit() {
        let server = MockServer::start().await;
        let client = mock_client(&server).await;
        assert!(matches!(
            client.lookup_user("../other").await,
            Err(BrokerError::BadRequest)
        ));
        for status in [401, 403, 404, 500] {
            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/user"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
            let result = client.user("token").await;
            match status {
                401 => assert!(matches!(result, Err(BrokerError::Unauthorized))),
                403 => assert!(matches!(result, Err(BrokerError::Forbidden))),
                404 => assert!(matches!(result, Err(BrokerError::NotFound))),
                _ => assert!(matches!(result, Err(BrokerError::Unavailable))),
            }
        }
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/users/octocat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":2,"login":"octocat","avatar_url":""})),
            )
            .mount(&server)
            .await;
        assert_eq!(client.lookup_user("octocat").await.unwrap().id, 2);
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        assert!(matches!(
            client.revoke_token("token").await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("POST")).and(path("/login/device/code")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code":"device","user_code":"ABCD","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0
        }))).mount(&server).await;
        assert!(matches!(
            client.start_device_flow().await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"error":"bad_verification_code"})),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            client
                .exchange_code("code", "https://broker.test/", "verifier")
                .await,
            Err(BrokerError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn oauth_callback_binds_cookie_and_state_and_rejects_replay() {
        let server = MockServer::start().await;
        let mut state = state();
        let mut service = AuthService::from_settings(&state.settings).unwrap();
        service.github = Some(Arc::new(mock_client(&server).await));
        state.auth = Arc::new(service);
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"token"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":1,"login":"admin","avatar_url":""})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/applications/client-id/token"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let app = crate::app(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/github/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 307);
        let authorization =
            Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let returned_state = authorization
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/github/callback?code=code&state=wrong")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), 401);
        let uri = format!("/auth/github/callback?code=code&state={returned_state}");
        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(success.status(), 303);
        assert_eq!(success.headers()[header::LOCATION], "/");
        let replay = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), 401);
    }
}
