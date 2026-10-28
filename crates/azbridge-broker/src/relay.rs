//! Resource-group-scoped Relay management and HC-scoped SAS issuance.
//!
//! ARM operations are deliberately non-blocking: accepted/incomplete operations return
//! `Provisioning` (or `false` for deletion). The reconciler must repeat them. Operation
//! URLs are validated but never followed; only the configured resource is read again.
//! Keys live only in a five-minute in-memory cache. Rotation affects future
//! authentication, not already established tunnels.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant, UNIX_EPOCH},
};

use async_trait::async_trait;
use azbridge_broker_api::{ProvisioningState, RelayRight, RelayTokenResponse};
use azure_core::credentials::TokenCredential;
use azure_identity::{ManagedIdentityCredential, ManagedIdentityCredentialOptions, UserAssignedId};
use azure_relay::{
    build_relay_address,
    protocol::normalize_audience,
    token_provider::{SharedAccessSignatureTokenProvider, TokenProvider},
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use url::Url;

use crate::{error::BrokerError, settings::CloudSettings};

const API_VERSION: &str = "2026-01-01";
const KEY_TTL: Duration = Duration::from_secs(300);
const TOKEN_TTL: Duration = Duration::from_secs(3600);
const MAX_CACHED_KEYS: usize = 1024;

/// All names are single resource-name segments, never caller-supplied resource IDs.
/// Delete returns `true` only once absence or synchronous completion is confirmed.
#[async_trait]
pub trait RelayManager: Send + Sync {
    async fn ensure_namespace(
        &self,
        namespace: &str,
        location: &str,
    ) -> Result<ProvisioningState, BrokerError>;
    async fn delete_namespace(&self, namespace: &str) -> Result<bool, BrokerError>;
    async fn ensure_connection(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<ProvisioningState, BrokerError>;
    async fn delete_connection(&self, namespace: &str, name: &str) -> Result<bool, BrokerError>;
    async fn rotate_keys(&self, namespace: &str, name: &str) -> Result<(), BrokerError>;
    async fn issue_token(
        &self,
        namespace: &str,
        relay_name: &str,
        right: RelayRight,
    ) -> Result<RelayTokenResponse, BrokerError>;
}

struct CachedKey {
    key: String,
    expires: Instant,
}

type KeyId = (String, String, RelayRight);

/// Production implementation. Intentionally no Debug: credentials and keys are private.
pub struct ArmRelayManager {
    client: reqwest::Client,
    credential: Arc<dyn TokenCredential>,
    endpoint: Url,
    resource_prefix: String,
    subscription_prefix: String,
    locations: BTreeSet<String>,
    // Serializes signing, rotation and deletion so an in-flight key read cannot
    // repopulate a stale cache after rotation/deletion.
    keys: Mutex<HashMap<KeyId, CachedKey>>,
}

impl ArmRelayManager {
    /// Uses only managed identity, never a CLI/developer credential or fake fallback.
    pub fn from_cloud(settings: &CloudSettings) -> Result<Self, BrokerError> {
        let credential = ManagedIdentityCredential::new(Some(ManagedIdentityCredentialOptions {
            user_assigned_id: settings
                .azure_client_id
                .clone()
                .map(UserAssignedId::ClientId),
            ..Default::default()
        }))
        .map_err(|_| BrokerError::Unavailable)?;
        Self::with_credential(
            settings,
            credential,
            Url::parse("https://management.azure.com/").map_err(|_| BrokerError::Unavailable)?,
        )
    }

    /// Injectable credential and ARM origin for protocol tests. Only public ARM HTTPS
    /// or a loopback test server is permitted; the endpoint cannot contain a path,
    /// user info, query, or fragment. Cloud configuration still scopes every resource.
    pub fn with_credential(
        settings: &CloudSettings,
        credential: Arc<dyn TokenCredential>,
        endpoint: Url,
    ) -> Result<Self, BrokerError> {
        let loopback = match endpoint.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(host)) => host == "localhost",
            None => false,
        };
        let public_arm = endpoint.scheme() == "https"
            && endpoint.host_str() == Some("management.azure.com")
            && endpoint.port_or_known_default() == Some(443);
        if (!public_arm && !(loopback && matches!(endpoint.scheme(), "http" | "https")))
            || endpoint.path() != "/"
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || uuid::Uuid::parse_str(&settings.subscription_id).is_err()
            || !valid_group(&settings.relay_resource_group)
            || settings.allowed_locations.is_empty()
            || settings
                .allowed_locations
                .iter()
                .any(|s| !valid_location(s))
        {
            return Err(BrokerError::BadRequest);
        }
        let subscription_prefix = format!("/subscriptions/{}/", settings.subscription_id);
        let resource_prefix = format!(
            "{}resourceGroups/{}/providers/Microsoft.Relay/namespaces",
            subscription_prefix, settings.relay_resource_group
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| BrokerError::Unavailable)?;
        Ok(Self {
            client,
            credential,
            endpoint,
            resource_prefix,
            subscription_prefix,
            locations: settings.allowed_locations.clone(),
            keys: Mutex::new(HashMap::new()),
        })
    }

    fn namespace_path(&self, namespace: &str) -> Result<String, BrokerError> {
        validate_namespace(namespace)?;
        Ok(format!("{}/{namespace}", self.resource_prefix))
    }

    fn connection_path(&self, namespace: &str, name: &str) -> Result<String, BrokerError> {
        validate_connection(name)?;
        Ok(format!(
            "{}/hybridConnections/{name}",
            self.namespace_path(namespace)?
        ))
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<ArmResponse, BrokerError> {
        let token = tokio::time::timeout(
            Duration::from_secs(30),
            self.credential
                .get_token(&["https://management.azure.com/.default"], None),
        )
        .await
        .map_err(|_| BrokerError::Unavailable)?
        .map_err(|_| BrokerError::Unavailable)?;
        let mut url = self.endpoint.clone();
        url.set_path(path);
        url.query_pairs_mut()
            .append_pair("api-version", API_VERSION);
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(token.token.secret());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| BrokerError::Unavailable)?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(ArmResponse {
                status,
                asynchronous: false,
                body: Value::Null,
            });
        }
        if !status.is_success() {
            return Err(match status {
                StatusCode::CONFLICT => BrokerError::Conflict,
                StatusCode::TOO_MANY_REQUESTS => BrokerError::TooManyRequests,
                _ => BrokerError::Unavailable,
            });
        }
        let mut asynchronous = status == StatusCode::ACCEPTED;
        for header in ["operation-location", "azure-asyncoperation", "location"] {
            if let Some(value) = response.headers().get(header) {
                let value = value.to_str().map_err(|_| BrokerError::Unavailable)?;
                let operation = self
                    .endpoint
                    .join(value)
                    .map_err(|_| BrokerError::Unavailable)?;
                // Even though operation URLs are never followed, fail closed on
                // unexpected origins/subscriptions and path escape sequences.
                if operation.origin() != self.endpoint.origin()
                    || !operation.username().is_empty()
                    || operation.password().is_some()
                    || operation.fragment().is_some()
                    || !operation.path().starts_with(&self.subscription_prefix)
                    || operation.path().contains('%')
                    || operation.path().contains('\\')
                {
                    return Err(BrokerError::Unavailable);
                }
                asynchronous = true;
            }
        }
        // Retry-After is intentionally not slept here: incomplete work is returned
        // to the reconciler rather than occupying a request with an unbounded poll.
        let bytes = response
            .bytes()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| BrokerError::Unavailable)?
        };
        Ok(ArmResponse {
            status,
            asynchronous,
            body,
        })
    }

    async fn namespace_ready(&self, namespace: &str) -> Result<bool, BrokerError> {
        let response = self
            .request(Method::GET, &self.namespace_path(namespace)?, None)
            .await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(BrokerError::NotFound);
        }
        Ok(response.state(true) == ProvisioningState::Ready)
    }

    async fn connection_ready(&self, namespace: &str, name: &str) -> Result<bool, BrokerError> {
        if !self.namespace_ready(namespace).await? {
            return Ok(false);
        }
        let response = self
            .request(Method::GET, &self.connection_path(namespace, name)?, None)
            .await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(BrokerError::NotFound);
        }
        Ok(response.state(false) == ProvisioningState::Ready
            && response.body["properties"]["requiresClientAuthorization"] == true)
    }

    async fn delete(&self, path: &str) -> Result<bool, BrokerError> {
        let response = self.request(Method::DELETE, path, None).await?;
        if response.status == StatusCode::NOT_FOUND {
            return Ok(true);
        }
        match response.body["properties"]["provisioningState"].as_str() {
            Some("Failed" | "Canceled" | "Cancelled") => Err(BrokerError::Unavailable),
            _ if response.asynchronous => Ok(false),
            None | Some("Succeeded") => Ok(true),
            _ => Ok(false),
        }
    }
}

struct ArmResponse {
    status: StatusCode,
    asynchronous: bool,
    body: Value,
}

impl ArmResponse {
    fn state(&self, namespace: bool) -> ProvisioningState {
        let state = self.body["properties"]["provisioningState"].as_str();
        match state {
            Some("Failed" | "Canceled" | "Cancelled") => ProvisioningState::Failed,
            Some("Deleting") => ProvisioningState::Deleting,
            _ if self.asynchronous => ProvisioningState::Provisioning,
            Some("Succeeded") => ProvisioningState::Ready,
            None if !namespace && self.body.is_object() => ProvisioningState::Ready,
            _ => ProvisioningState::Provisioning,
        }
    }
}

#[async_trait]
impl RelayManager for ArmRelayManager {
    async fn ensure_namespace(
        &self,
        namespace: &str,
        location: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        let path = self.namespace_path(namespace)?;
        if !self.locations.contains(location) {
            return Err(BrokerError::BadRequest);
        }
        let response = self.request(Method::GET, &path, None).await?;
        if response.status != StatusCode::NOT_FOUND {
            return Ok(response.state(true));
        }
        let response = self
            .request(
                Method::PUT,
                &path,
                Some(json!({
                    "location": location,
                    "sku": {"name": "Standard", "tier": "Standard"},
                    "properties": {"minimumTlsVersion": "1.2"}
                })),
            )
            .await?;
        if response.status == StatusCode::NOT_FOUND {
            return Err(BrokerError::NotFound);
        }
        Ok(response.state(true))
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<bool, BrokerError> {
        let path = self.namespace_path(namespace)?;
        let mut keys = self.keys.lock().await;
        keys.retain(|(ns, _, _), _| !ns.eq_ignore_ascii_case(namespace));
        self.delete(&path).await
    }

    async fn ensure_connection(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        let path = self.connection_path(namespace, name)?;
        if !self.namespace_ready(namespace).await? {
            return Ok(ProvisioningState::Provisioning);
        }
        let mut response = self.request(Method::GET, &path, None).await?;
        if response.status != StatusCode::NOT_FOUND
            && response.state(false) != ProvisioningState::Ready
        {
            return Ok(response.state(false));
        }
        if response.status == StatusCode::NOT_FOUND
            || response.body["properties"]["requiresClientAuthorization"] != true
        {
            response = self
                .request(
                    Method::PUT,
                    &path,
                    Some(json!({"properties": {"requiresClientAuthorization": true}})),
                )
                .await?;
        }
        if response.status == StatusCode::NOT_FOUND {
            return Err(BrokerError::NotFound);
        }
        if response.state(false) != ProvisioningState::Ready {
            return Ok(response.state(false));
        }
        if response.body["properties"]["requiresClientAuthorization"] != true {
            return Ok(ProvisioningState::Provisioning);
        }
        let mut state = ProvisioningState::Ready;
        for right in [RelayRight::Send, RelayRight::Listen] {
            let rule_path = format!("{path}/authorizationRules/{}", rule(right));
            let mut response = self.request(Method::GET, &rule_path, None).await?;
            if response.status != StatusCode::NOT_FOUND
                && response.state(false) != ProvisioningState::Ready
            {
                return Ok(response.state(false));
            }
            let expected = json!([arm_right(right)]);
            if response.status == StatusCode::NOT_FOUND
                || response.body["properties"]["rights"] != expected
            {
                response = self
                    .request(
                        Method::PUT,
                        &rule_path,
                        Some(json!({"properties": {"rights": expected}})),
                    )
                    .await?;
            }
            if response.status == StatusCode::NOT_FOUND {
                return Err(BrokerError::NotFound);
            }
            let next = response.state(false);
            if next == ProvisioningState::Failed {
                return Ok(next);
            }
            if next != ProvisioningState::Ready {
                state = next;
            } else if response.body["properties"]["rights"] != expected {
                state = ProvisioningState::Provisioning;
            }
        }
        Ok(state)
    }

    async fn delete_connection(&self, namespace: &str, name: &str) -> Result<bool, BrokerError> {
        let path = self.connection_path(namespace, name)?;
        let mut keys = self.keys.lock().await;
        keys.retain(|(ns, hc, _), _| {
            !ns.eq_ignore_ascii_case(namespace) || !hc.eq_ignore_ascii_case(name)
        });
        self.delete(&path).await
    }

    async fn rotate_keys(&self, namespace: &str, name: &str) -> Result<(), BrokerError> {
        let path = self.connection_path(namespace, name)?;
        let mut keys = self.keys.lock().await;
        // Invalidate both rights even if ARM fails after rotating only one.
        keys.retain(|(ns, hc, _), _| {
            !ns.eq_ignore_ascii_case(namespace) || !hc.eq_ignore_ascii_case(name)
        });
        for right in [RelayRight::Send, RelayRight::Listen] {
            let response = self
                .request(
                    Method::POST,
                    &format!("{path}/authorizationRules/{}/regenerateKeys", rule(right)),
                    Some(json!({"keyType": "PrimaryKey"})),
                )
                .await?;
            if response.status == StatusCode::NOT_FOUND {
                return Err(BrokerError::NotFound);
            }
            if response.asynchronous
                || response.body["primaryKey"]
                    .as_str()
                    .is_none_or(|key| key.is_empty() || key.len() > 256)
            {
                return Err(BrokerError::Unavailable);
            }
        }
        Ok(())
    }

    async fn issue_token(
        &self,
        namespace: &str,
        relay_name: &str,
        right: RelayRight,
    ) -> Result<RelayTokenResponse, BrokerError> {
        let path = self.connection_path(namespace, relay_name)?;
        let mut keys = self.keys.lock().await;
        if !self.connection_ready(namespace, relay_name).await? {
            return Err(BrokerError::Conflict);
        }
        let rule_path = format!("{path}/authorizationRules/{}", rule(right));
        let authorization = self.request(Method::GET, &rule_path, None).await?;
        if authorization.status == StatusCode::NOT_FOUND {
            return Err(BrokerError::NotFound);
        }
        if authorization.state(false) != ProvisioningState::Ready
            || authorization.body["properties"]["rights"] != json!([arm_right(right)])
        {
            return Err(BrokerError::Conflict);
        }
        keys.retain(|_, entry| entry.expires > Instant::now());
        let id = (
            namespace.to_ascii_lowercase(),
            relay_name.to_ascii_lowercase(),
            right,
        );
        if !keys.contains_key(&id) {
            let response = self
                .request(
                    Method::POST,
                    &format!("{rule_path}/listKeys"),
                    Some(json!({})),
                )
                .await?;
            if response.status == StatusCode::NOT_FOUND {
                return Err(BrokerError::NotFound);
            }
            if response.asynchronous {
                return Err(BrokerError::Unavailable);
            }
            let key = response.body["primaryKey"]
                .as_str()
                .filter(|key| !key.is_empty() && key.len() <= 256)
                .ok_or(BrokerError::Unavailable)?;
            if keys.len() >= MAX_CACHED_KEYS {
                keys.clear();
            }
            keys.insert(
                id.clone(),
                CachedKey {
                    key: key.to_owned(),
                    expires: Instant::now() + KEY_TTL,
                },
            );
        }
        sign(namespace, relay_name, right, &keys[&id].key).await
    }
}

fn rule(right: RelayRight) -> &'static str {
    match right {
        RelayRight::Send => "azbridge-send",
        RelayRight::Listen => "azbridge-listen",
    }
}

fn arm_right(right: RelayRight) -> &'static str {
    match right {
        RelayRight::Send => "Send",
        RelayRight::Listen => "Listen",
    }
}

fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 90
        && !group.ends_with('.')
        && group
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'(' | b')'))
}

fn valid_location(location: &str) -> bool {
    !location.is_empty()
        && location
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn validate_namespace(namespace: &str) -> Result<(), BrokerError> {
    if !(6..=50).contains(&namespace.len())
        || !namespace.starts_with(|c: char| c.is_ascii_alphabetic())
        || !namespace.ends_with(|c: char| c.is_ascii_alphanumeric())
        || !namespace
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

fn validate_connection(name: &str) -> Result<(), BrokerError> {
    // Deliberately a single segment, even though Relay permits hierarchical paths.
    if name.is_empty()
        || name.len() > 260
        || !name.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !name.ends_with(|c: char| c.is_ascii_alphanumeric())
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

async fn sign(
    namespace: &str,
    name: &str,
    right: RelayRight,
    key: &str,
) -> Result<RelayTokenResponse, BrokerError> {
    let endpoint = Url::parse(&format!("sb://{namespace}.servicebus.windows.net/"))
        .map_err(|_| BrokerError::BadRequest)?;
    let address = build_relay_address(&endpoint, name).map_err(|_| BrokerError::BadRequest)?;
    // .NET normalizes the whole audience to lowercase, including the entity path.
    let audience = normalize_audience(&address).to_ascii_lowercase();
    let provider = SharedAccessSignatureTokenProvider::new(rule(right), key)
        .map_err(|_| BrokerError::Unavailable)?;
    let token = provider
        .get_token(&audience, TOKEN_TTL)
        .await
        .map_err(|_| BrokerError::Unavailable)?;
    Ok(RelayTokenResponse {
        token: token.token,
        expires_at: token
            .expires_at
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BrokerError::Unavailable)?
            .as_secs(),
    })
}

/// Explicit development-only manager, never selected after a cloud failure.
/// Initial keys are a known, non-secret test fixture, not Azure credentials.
#[derive(Default)]
pub struct FakeRelayManager {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    namespaces: BTreeSet<String>,
    connections: BTreeMap<(String, String), [String; 2]>,
}

impl FakeRelayManager {
    /// Select only when startup has explicitly validated `--dev`.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RelayManager for FakeRelayManager {
    async fn ensure_namespace(
        &self,
        namespace: &str,
        location: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        validate_namespace(namespace)?;
        if !valid_location(location) {
            return Err(BrokerError::BadRequest);
        }
        self.state
            .lock()
            .await
            .namespaces
            .insert(namespace.to_ascii_lowercase());
        Ok(ProvisioningState::Ready)
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<bool, BrokerError> {
        validate_namespace(namespace)?;
        let mut state = self.state.lock().await;
        state.namespaces.remove(&namespace.to_ascii_lowercase());
        state
            .connections
            .retain(|(ns, _), _| !ns.eq_ignore_ascii_case(namespace));
        Ok(true)
    }

    async fn ensure_connection(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        validate_namespace(namespace)?;
        validate_connection(name)?;
        let mut state = self.state.lock().await;
        if !state.namespaces.contains(&namespace.to_ascii_lowercase()) {
            return Err(BrokerError::NotFound);
        }
        state
            .connections
            .entry((namespace.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .or_insert_with(|| {
                [
                    "ZGV2ZWxvcG1lbnQtb25seS1zZW5kLWtleQ==".into(),
                    "ZGV2ZWxvcG1lbnQtb25seS1saXN0ZW4ta2V5".into(),
                ]
            });
        Ok(ProvisioningState::Ready)
    }

    async fn delete_connection(&self, namespace: &str, name: &str) -> Result<bool, BrokerError> {
        validate_namespace(namespace)?;
        validate_connection(name)?;
        self.state
            .lock()
            .await
            .connections
            .remove(&(namespace.to_ascii_lowercase(), name.to_ascii_lowercase()));
        Ok(true)
    }

    async fn rotate_keys(&self, namespace: &str, name: &str) -> Result<(), BrokerError> {
        validate_namespace(namespace)?;
        validate_connection(name)?;
        let mut state = self.state.lock().await;
        let keys = state
            .connections
            .get_mut(&(namespace.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .ok_or(BrokerError::NotFound)?;
        *keys = std::array::from_fn(|_| uuid::Uuid::new_v4().to_string());
        Ok(())
    }

    async fn issue_token(
        &self,
        namespace: &str,
        relay_name: &str,
        right: RelayRight,
    ) -> Result<RelayTokenResponse, BrokerError> {
        validate_namespace(namespace)?;
        validate_connection(relay_name)?;
        let state = self.state.lock().await;
        let keys = state
            .connections
            .get(&(
                namespace.to_ascii_lowercase(),
                relay_name.to_ascii_lowercase(),
            ))
            .ok_or(BrokerError::NotFound)?;
        sign(
            namespace,
            relay_name,
            right,
            &keys[usize::from(right == RelayRight::Listen)],
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azure_core::credentials::{AccessToken, TokenRequestOptions};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path_regex},
    };

    #[derive(Debug)]
    struct Credential;

    #[async_trait]
    impl TokenCredential for Credential {
        async fn get_token(
            &self,
            _: &[&str],
            _: Option<TokenRequestOptions<'_>>,
        ) -> azure_core::Result<AccessToken> {
            Ok(AccessToken::new(
                "test",
                azure_core::time::OffsetDateTime::now_utc() + azure_core::time::Duration::hours(1),
            ))
        }
    }

    #[tokio::test]
    async fn expired_primary_key_is_refetched_and_cache_has_bounded_size() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("/namespaces/azb-test$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"properties": {"provisioningState": "Succeeded"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("/hybridConnections/connection$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"properties": {"requiresClientAuthorization": true}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex("/authorizationRules/azbridge-send/listKeys$"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"primaryKey": "fresh-key"})),
            )
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("/authorizationRules/azbridge-send$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"properties": {"rights": ["Send"]}})),
            )
            .mount(&server)
            .await;
        let id = ("azb-test".into(), "connection".into(), RelayRight::Send);
        let mut cache = HashMap::new();
        cache.insert(
            id.clone(),
            CachedKey {
                key: "expired-key".into(),
                expires: Instant::now() - Duration::from_secs(1),
            },
        );
        let manager = ArmRelayManager {
            client: reqwest::Client::new(),
            credential: Arc::new(Credential),
            endpoint: Url::parse(&server.uri()).unwrap(),
            resource_prefix:
                "/subscriptions/test/resourceGroups/test/providers/Microsoft.Relay/namespaces"
                    .into(),
            subscription_prefix: "/subscriptions/test/".into(),
            locations: BTreeSet::from(["westus".into()]),
            keys: Mutex::new(cache),
        };
        manager
            .issue_token("azb-test", "connection", RelayRight::Send)
            .await
            .unwrap();
        {
            let mut keys = manager.keys.lock().await;
            let entry = &keys[&id];
            assert_eq!(entry.key, "fresh-key");
            assert!(entry.expires <= Instant::now() + Duration::from_secs(300));
            keys.clear();
            for i in 0..MAX_CACHED_KEYS {
                keys.insert(
                    ("azb-test".into(), i.to_string(), RelayRight::Send),
                    CachedKey {
                        key: "live-key".into(),
                        expires: Instant::now() + KEY_TTL,
                    },
                );
            }
        }
        manager
            .issue_token("azb-test", "connection", RelayRight::Send)
            .await
            .unwrap();
        assert_eq!(manager.keys.lock().await.len(), 1);
    }
}
