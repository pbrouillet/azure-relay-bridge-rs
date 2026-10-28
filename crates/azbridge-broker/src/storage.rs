//! ETag-conditional metadata and immutable YAML revisions.
//!
//! Production tables are provisioned by infrastructure, not lazily created on access.
//! Missing tables/containers and authentication failures are dependency errors, never
//! an empty collection or a fallback to memory. Revision blobs live in
//! `configs/profiles/{workspace}/{profile}/{revision:06}.yaml`.
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_identity::{ManagedIdentityCredential, ManagedIdentityCredentialOptions, UserAssignedId};
use azure_storage_blob::{
    clients::{BlobClient, BlobClientOptions},
    models::BlobClientUploadOptions,
};
use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use url::Url;

use crate::{error::BrokerError, settings::Settings};

const STORAGE_SCOPE: &str = "https://storage.azure.com/.default";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Exact Azure Table Storage names; only these tables can be addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    Workspaces,
    Members,
    UserWorkspaces,
    PlatformAdmins,
    Connections,
    Profiles,
    Gateways,
    Enrollments,
    Audit,
    RevisionMetadata,
}

impl Table {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspaces => "Workspaces",
            Self::Members => "Members",
            Self::UserWorkspaces => "UserWorkspaces",
            Self::PlatformAdmins => "PlatformAdmins",
            Self::Connections => "Connections",
            Self::Profiles => "Profiles",
            Self::Gateways => "Gateways",
            Self::Enrollments => "Enrollments",
            Self::Audit => "Audit",
            Self::RevisionMetadata => "RevisionMetadata",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub partition_key: String,
    pub row_key: String,
    pub data: Value,
    pub etag: String,
}

/// Conditional mutations return `Conflict` for stale ETags and `NotFound` for
/// absent entities. Wildcard/empty ETags are rejected; callers must read first.
#[async_trait]
pub trait MetadataStore: Send + Sync {
    async fn get(
        &self,
        table: Table,
        partition: &str,
        row: &str,
    ) -> Result<Option<Entity>, BrokerError>;
    async fn insert(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
    ) -> Result<Entity, BrokerError>;
    async fn replace(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
        etag: &str,
    ) -> Result<Entity, BrokerError>;
    async fn delete(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        etag: &str,
    ) -> Result<(), BrokerError>;
    /// Returns every page in the partition, or an error (never a partial result).
    async fn list(&self, table: Table, partition: &str) -> Result<Vec<Entity>, BrokerError>;
}

#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Create-only. An existing revision is a conflict, even for identical YAML.
    async fn put_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
        yaml: &str,
    ) -> Result<(), BrokerError>;
    async fn get_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
    ) -> Result<Option<String>, BrokerError>;
}

fn validate_key(key: &str) -> Result<(), BrokerError> {
    if key.is_empty()
        || key.len() > 1024
        || key.chars().any(|c| c.is_control() || "/\\#?".contains(c))
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

fn validate_etag(etag: &str) -> Result<(), BrokerError> {
    let quoted = etag.strip_prefix("W/").unwrap_or(etag);
    if quoted.len() < 2
        || !quoted.starts_with('"')
        || !quoted.ends_with('"')
        || quoted[1..quoted.len() - 1].contains('"')
        || reqwest::header::HeaderValue::from_str(etag).is_err()
    {
        return Err(BrokerError::BadRequest);
    }
    Ok(())
}

#[derive(Default)]
struct MemoryMetadata {
    entities: BTreeMap<(Table, String, String), Entity>,
    version: u64,
}

/// Explicit development/test implementation. A single lock makes compare-and-swap
/// atomic, and versions never reset on delete (preventing ABA).
#[derive(Default)]
pub struct InMemoryMetadataStore {
    state: RwLock<MemoryMetadata>,
}

impl InMemoryMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn memory_entity(
    state: &mut MemoryMetadata,
    partition: &str,
    row: &str,
    data: Value,
) -> Result<Entity, BrokerError> {
    state.version = state
        .version
        .checked_add(1)
        .ok_or(BrokerError::Unavailable)?;
    Ok(Entity {
        partition_key: partition.into(),
        row_key: row.into(),
        data,
        etag: format!("\"{}\"", state.version),
    })
}

#[async_trait]
impl MetadataStore for InMemoryMetadataStore {
    async fn get(
        &self,
        table: Table,
        partition: &str,
        row: &str,
    ) -> Result<Option<Entity>, BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        Ok(self
            .state
            .read()
            .await
            .entities
            .get(&(table, partition.into(), row.into()))
            .cloned())
    }

    async fn insert(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
    ) -> Result<Entity, BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        let mut state = self.state.write().await;
        let key = (table, partition.into(), row.into());
        if state.entities.contains_key(&key) {
            return Err(BrokerError::Conflict);
        }
        let entity = memory_entity(&mut state, partition, row, data)?;
        state.entities.insert(key, entity.clone());
        Ok(entity)
    }

    async fn replace(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
        etag: &str,
    ) -> Result<Entity, BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        validate_etag(etag)?;
        let mut state = self.state.write().await;
        let key = (table, partition.into(), row.into());
        let existing = state.entities.get(&key).ok_or(BrokerError::NotFound)?;
        if existing.etag != etag {
            return Err(BrokerError::Conflict);
        }
        let entity = memory_entity(&mut state, partition, row, data)?;
        state.entities.insert(key, entity.clone());
        Ok(entity)
    }

    async fn delete(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        etag: &str,
    ) -> Result<(), BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        validate_etag(etag)?;
        let mut state = self.state.write().await;
        let key = (table, partition.into(), row.into());
        let existing = state.entities.get(&key).ok_or(BrokerError::NotFound)?;
        if existing.etag != etag {
            return Err(BrokerError::Conflict);
        }
        state.entities.remove(&key);
        Ok(())
    }

    async fn list(&self, table: Table, partition: &str) -> Result<Vec<Entity>, BrokerError> {
        validate_key(partition)?;
        Ok(self
            .state
            .read()
            .await
            .entities
            .iter()
            .filter(|((t, p, _), _)| *t == table && p == partition)
            .map(|(_, entity)| entity.clone())
            .collect())
    }
}

#[derive(Default)]
pub struct InMemoryConfigStore {
    revisions: RwLock<BTreeMap<String, String>>,
}

impl InMemoryConfigStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn revision_path(workspace: &str, profile: &str, revision: &str) -> Result<String, BrokerError> {
    for part in [workspace, profile] {
        if part.is_empty()
            || part.len() > 128
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(BrokerError::BadRequest);
        }
    }
    let revision = revision
        .parse::<u64>()
        .map_err(|_| BrokerError::BadRequest)?;
    Ok(format!("profiles/{workspace}/{profile}/{revision:06}.yaml"))
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn put_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
        yaml: &str,
    ) -> Result<(), BrokerError> {
        let path = revision_path(workspace, profile, revision)?;
        let mut revisions = self.revisions.write().await;
        if revisions.contains_key(&path) {
            return Err(BrokerError::Conflict);
        }
        revisions.insert(path, yaml.into());
        Ok(())
    }

    async fn get_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
    ) -> Result<Option<String>, BrokerError> {
        let path = revision_path(workspace, profile, revision)?;
        Ok(self.revisions.read().await.get(&path).cloned())
    }
}

fn account_endpoint(account: &str, service: &str) -> Result<Url, BrokerError> {
    if !(3..=24).contains(&account.len())
        || !account
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(BrokerError::BadRequest);
    }
    Url::parse(&format!("https://{account}.{service}.core.windows.net/"))
        .map_err(|_| BrokerError::BadRequest)
}

/// Minimal Azure Table REST adapter. Endpoints are scoped to a validated account;
/// injected credentials never receive arbitrary caller-supplied URLs.
pub struct AzureMetadataStore {
    endpoint: Url,
    credential: Arc<dyn TokenCredential>,
    client: Client,
    request_timeout: Duration,
}

impl AzureMetadataStore {
    pub fn new(account: &str, credential: Arc<dyn TokenCredential>) -> Result<Self, BrokerError> {
        Self::from_endpoint(account_endpoint(account, "table")?, credential)
    }

    fn from_endpoint(
        endpoint: Url,
        credential: Arc<dyn TokenCredential>,
    ) -> Result<Self, BrokerError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| BrokerError::Unavailable)?;
        Ok(Self {
            endpoint,
            credential,
            client,
            request_timeout: REQUEST_TIMEOUT,
        })
    }

    fn entity_url(&self, table: Table, partition: &str, row: &str) -> Result<Url, BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        let segment = format!(
            "{}(PartitionKey='{}',RowKey='{}')",
            table.as_str(),
            partition.replace('\'', "''"),
            row.replace('\'', "''")
        );
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| BrokerError::Unavailable)?
            .pop_if_empty()
            .push(&segment);
        Ok(url)
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
        etag: Option<&str>,
    ) -> Result<Response, BrokerError> {
        // The timeout includes credential acquisition, not only network I/O.
        tokio::time::timeout(self.request_timeout, async {
            let token = self
                .credential
                .get_token(&[STORAGE_SCOPE], None)
                .await
                .map_err(|_| BrokerError::Unavailable)?;
            let mut request = self
                .client
                .request(method, url)
                .bearer_auth(token.token.secret())
                .header("x-ms-version", "2019-02-02")
                .header("x-ms-date", httpdate::fmt_http_date(SystemTime::now()))
                .header("DataServiceVersion", "3.0;NetFx")
                .header("MaxDataServiceVersion", "3.0;NetFx")
                .header("Accept", "application/json;odata=minimalmetadata");
            if let Some(body) = body {
                request = request.header("Prefer", "return-no-content").json(&body);
            }
            if let Some(etag) = etag {
                request = request.header("If-Match", etag);
            }
            request.send().await.map_err(|_| BrokerError::Unavailable)
        })
        .await
        .map_err(|_| BrokerError::Unavailable)?
    }
}

fn table_body(partition: &str, row: &str, data: &Value) -> Value {
    json!({"PartitionKey": partition, "RowKey": row, "Data": data.to_string()})
}

fn response_etag(response: &Response) -> Result<String, BrokerError> {
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or(BrokerError::Unavailable)?;
    validate_etag(&etag).map_err(|_| BrokerError::Unavailable)?;
    Ok(etag)
}

fn parse_entity(value: Value, etag: Option<String>) -> Result<Entity, BrokerError> {
    let string = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(BrokerError::Unavailable)
    };
    let entity = Entity {
        partition_key: string("PartitionKey")?,
        row_key: string("RowKey")?,
        data: serde_json::from_str(&string("Data")?).map_err(|_| BrokerError::Unavailable)?,
        etag: etag.unwrap_or(string("odata.etag").unwrap_or_default()),
    };
    validate_key(&entity.partition_key).map_err(|_| BrokerError::Unavailable)?;
    validate_key(&entity.row_key).map_err(|_| BrokerError::Unavailable)?;
    validate_etag(&entity.etag).map_err(|_| BrokerError::Unavailable)?;
    Ok(entity)
}

/// Only recognized entity-not-found errors become absence. A missing table,
/// malformed response or an expired credential is a dependency failure.
async fn table_error(response: Response) -> BrokerError {
    let status = response.status();
    let header_code = response
        .headers()
        .get("x-ms-error-code")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let code = header_code.as_deref().or_else(|| {
        body.pointer("/odata.error/code")
            .or_else(|| body.pointer("/error/code"))
            .and_then(Value::as_str)
    });
    match (status, code) {
        (StatusCode::NOT_FOUND, Some("ResourceNotFound" | "EntityNotFound")) => {
            BrokerError::NotFound
        }
        (StatusCode::CONFLICT, Some("EntityAlreadyExists")) => BrokerError::Conflict,
        (StatusCode::PRECONDITION_FAILED, Some("UpdateConditionNotSatisfied")) => {
            BrokerError::Conflict
        }
        _ => BrokerError::Unavailable,
    }
}

#[async_trait]
impl MetadataStore for AzureMetadataStore {
    async fn get(
        &self,
        table: Table,
        partition: &str,
        row: &str,
    ) -> Result<Option<Entity>, BrokerError> {
        let response = self
            .send(
                Method::GET,
                self.entity_url(table, partition, row)?,
                None,
                None,
            )
            .await?;
        if response.status() != StatusCode::OK {
            return match table_error(response).await {
                BrokerError::NotFound => Ok(None),
                error => Err(error),
            };
        }
        let etag = response_etag(&response).ok();
        let value = response
            .json()
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        let entity = parse_entity(value, etag)?;
        if entity.partition_key != partition || entity.row_key != row {
            return Err(BrokerError::Unavailable);
        }
        Ok(Some(entity))
    }

    async fn insert(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
    ) -> Result<Entity, BrokerError> {
        validate_key(partition)?;
        validate_key(row)?;
        let url = self
            .endpoint
            .join(table.as_str())
            .map_err(|_| BrokerError::Unavailable)?;
        let response = self
            .send(
                Method::POST,
                url,
                Some(table_body(partition, row, &data)),
                None,
            )
            .await?;
        if !matches!(
            response.status(),
            StatusCode::CREATED | StatusCode::NO_CONTENT
        ) {
            return Err(table_error(response).await);
        }
        Ok(Entity {
            partition_key: partition.into(),
            row_key: row.into(),
            data,
            etag: response_etag(&response)?,
        })
    }

    async fn replace(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
        etag: &str,
    ) -> Result<Entity, BrokerError> {
        validate_etag(etag)?;
        let response = self
            .send(
                Method::PUT,
                self.entity_url(table, partition, row)?,
                Some(table_body(partition, row, &data)),
                Some(etag),
            )
            .await?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(table_error(response).await);
        }
        Ok(Entity {
            partition_key: partition.into(),
            row_key: row.into(),
            data,
            etag: response_etag(&response)?,
        })
    }

    async fn delete(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        etag: &str,
    ) -> Result<(), BrokerError> {
        validate_etag(etag)?;
        let response = self
            .send(
                Method::DELETE,
                self.entity_url(table, partition, row)?,
                None,
                Some(etag),
            )
            .await?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(table_error(response).await);
        }
        Ok(())
    }

    async fn list(&self, table: Table, partition: &str) -> Result<Vec<Entity>, BrokerError> {
        validate_key(partition)?;
        let mut base = self
            .endpoint
            .join(&format!("{}()", table.as_str()))
            .map_err(|_| BrokerError::Unavailable)?;
        base.query_pairs_mut().append_pair(
            "$filter",
            &format!("PartitionKey eq '{}'", partition.replace('\'', "''")),
        );
        let mut url = base.clone();
        let mut entities = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let response = self.send(Method::GET, url, None, None).await?;
            if response.status() != StatusCode::OK {
                // A collection must exist, even when it is empty.
                return Err(BrokerError::Unavailable);
            }
            let continuation = |name| -> Result<Option<String>, BrokerError> {
                response
                    .headers()
                    .get(name)
                    .map(|h| {
                        h.to_str()
                            .map(str::to_owned)
                            .map_err(|_| BrokerError::Unavailable)
                    })
                    .transpose()
            };
            let next_partition = continuation("x-ms-continuation-NextPartitionKey")?;
            let next_row = continuation("x-ms-continuation-NextRowKey")?;
            let body: Value = response
                .json()
                .await
                .map_err(|_| BrokerError::Unavailable)?;
            let page = body
                .get("value")
                .and_then(Value::as_array)
                .ok_or(BrokerError::Unavailable)?;
            for value in page {
                let entity = parse_entity(value.clone(), None)?;
                if entity.partition_key != partition {
                    return Err(BrokerError::Unavailable);
                }
                entities.push(entity);
            }
            if next_partition.is_none() && next_row.is_none() {
                return Ok(entities);
            }
            if !seen.insert((next_partition.clone(), next_row.clone())) {
                return Err(BrokerError::Unavailable);
            }
            url = base.clone();
            if let Some(token) = next_partition {
                url.query_pairs_mut()
                    .append_pair("NextPartitionKey", &token);
            }
            if let Some(token) = next_row {
                url.query_pairs_mut().append_pair("NextRowKey", &token);
            }
        }
    }
}

pub struct AzureBlobConfigStore {
    endpoint: Url,
    credential: Arc<dyn TokenCredential>,
    options: BlobClientOptions,
    request_timeout: Duration,
}

impl AzureBlobConfigStore {
    pub fn new(account: &str, credential: Arc<dyn TokenCredential>) -> Result<Self, BrokerError> {
        Ok(Self {
            endpoint: account_endpoint(account, "blob")?,
            credential,
            options: BlobClientOptions::default(),
            request_timeout: REQUEST_TIMEOUT,
        })
    }

    fn blob(&self, path: &str) -> Result<BlobClient, BrokerError> {
        let url = self
            .endpoint
            .join(&format!("configs/{path}"))
            .map_err(|_| BrokerError::Unavailable)?;
        BlobClient::new(
            url,
            Some(self.credential.clone()),
            Some(self.options.clone()),
        )
        .map_err(|_| BrokerError::Unavailable)
    }
}

fn blob_error(error: azure_core::Error) -> BrokerError {
    use azure_core::{error::ErrorKind, http::StatusCode};
    match error.kind() {
        ErrorKind::HttpResponse {
            status: StatusCode::NotFound,
            error_code: Some(code),
            ..
        } if code == "BlobNotFound" => BrokerError::NotFound,
        ErrorKind::HttpResponse {
            status: StatusCode::Conflict | StatusCode::PreconditionFailed,
            error_code: Some(code),
            ..
        } if matches!(code.as_str(), "BlobAlreadyExists" | "ConditionNotMet") => {
            BrokerError::Conflict
        }
        _ => BrokerError::Unavailable,
    }
}

#[async_trait]
impl ConfigStore for AzureBlobConfigStore {
    async fn put_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
        yaml: &str,
    ) -> Result<(), BrokerError> {
        let path = revision_path(workspace, profile, revision)?;
        let blob = self.blob(&path)?;
        tokio::time::timeout(self.request_timeout, async {
            blob.upload(
                azure_core::Bytes::from(yaml.as_bytes().to_vec()).into(),
                Some(BlobClientUploadOptions {
                    blob_content_type: Some("application/yaml; charset=utf-8".into()),
                    ..BlobClientUploadOptions::default().if_not_exists()
                }),
            )
            .await
            .map_err(blob_error)?;
            Ok(())
        })
        .await
        .map_err(|_| BrokerError::Unavailable)?
    }

    async fn get_revision(
        &self,
        workspace: &str,
        profile: &str,
        revision: &str,
    ) -> Result<Option<String>, BrokerError> {
        let path = revision_path(workspace, profile, revision)?;
        let blob = self.blob(&path)?;
        tokio::time::timeout(self.request_timeout, async {
            match blob.download(None).await {
                Ok(response) => {
                    let bytes = response.body.collect().await.map_err(blob_error)?;
                    let yaml =
                        String::from_utf8(bytes.to_vec()).map_err(|_| BrokerError::Unavailable)?;
                    Ok(Some(yaml))
                }
                Err(error) => match blob_error(error) {
                    BrokerError::NotFound => Ok(None),
                    error => Err(error),
                },
            }
        })
        .await
        .map_err(|_| BrokerError::Unavailable)?
    }
}

pub type Stores = (Arc<dyn MetadataStore>, Arc<dyn ConfigStore>);

/// No developer credential chain in production. Dev is explicitly in-memory;
/// cloud initialization errors propagate instead of downgrading the backend.
pub async fn storage_factory(settings: &Settings) -> Result<Stores, BrokerError> {
    if settings.dev {
        return Ok((
            Arc::new(InMemoryMetadataStore::new()),
            Arc::new(InMemoryConfigStore::new()),
        ));
    }
    let cloud = settings.cloud.as_ref().ok_or(BrokerError::Unavailable)?;
    let credential = ManagedIdentityCredential::new(Some(ManagedIdentityCredentialOptions {
        user_assigned_id: cloud.azure_client_id.clone().map(UserAssignedId::ClientId),
        ..Default::default()
    }))
    .map_err(|_| BrokerError::Unavailable)?;
    Ok((
        Arc::new(AzureMetadataStore::new(
            &cloud.storage_account,
            credential.clone(),
        )?),
        Arc::new(AzureBlobConfigStore::new(
            &cloud.storage_account,
            credential,
        )?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use azure_core::credentials::{AccessToken, TokenRequestOptions};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    #[derive(Debug)]
    struct Credential {
        fail: bool,
    }

    #[async_trait]
    impl TokenCredential for Credential {
        async fn get_token(
            &self,
            scopes: &[&str],
            _: Option<TokenRequestOptions<'_>>,
        ) -> azure_core::Result<AccessToken> {
            assert_eq!(scopes, [STORAGE_SCOPE]);
            if self.fail {
                return Err(azure_core::Error::with_message(
                    azure_core::error::ErrorKind::Credential,
                    "never-log-this-secret",
                ));
            }
            Ok(AccessToken::new(
                "storage-test-token",
                azure_core::time::OffsetDateTime::now_utc() + azure_core::time::Duration::hours(1),
            ))
        }
    }

    fn credential() -> Arc<dyn TokenCredential> {
        Arc::new(Credential { fail: false })
    }

    fn settings(dev: bool) -> Settings {
        Settings {
            dev,
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            public_url: Url::parse("http://localhost:8080").unwrap(),
            static_dir: "nonexistent-storage-test-assets".into(),
            cloud: None,
        }
    }

    fn metadata(server: &MockServer) -> AzureMetadataStore {
        // Only this private unit-test helper permits a loopback HTTP endpoint.
        let endpoint = Url::parse(&format!("{}/", server.uri())).unwrap();
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        AzureMetadataStore::from_endpoint(endpoint, credential()).unwrap()
    }

    fn wire_entity(partition: &str, row: &str, etag: &str) -> Value {
        json!({
            "PartitionKey": partition,
            "RowKey": row,
            "Data": r#"{"name":"value","nested":[1,true]}"#,
            "odata.etag": etag
        })
    }

    fn error_response(status: u16, code: &str) -> ResponseTemplate {
        ResponseTemplate::new(status)
            .insert_header("x-ms-error-code", code)
            .set_body_json(json!({"odata.error":{"code":code,"message":{"value":"not logged"}}}))
    }

    #[tokio::test]
    async fn storage_memory_metadata_all_operations_and_isolation() {
        let store: Arc<dyn MetadataStore> = Arc::new(InMemoryMetadataStore::new());
        assert!(
            store
                .get(Table::Workspaces, "p", "r")
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.list(Table::Workspaces, "p").await.unwrap().is_empty());
        let data = json!({"nested": [true, null, "é"], "number": 42});
        let entity = store
            .insert(Table::Workspaces, "p", "r", data.clone())
            .await
            .unwrap();
        assert_eq!(entity.data, data);
        assert_eq!(
            store.get(Table::Workspaces, "p", "r").await.unwrap(),
            Some(entity.clone())
        );
        let encoded = serde_json::to_string(&entity).unwrap();
        assert_eq!(serde_json::from_str::<Entity>(&encoded).unwrap(), entity);
        assert!(matches!(
            store.insert(Table::Workspaces, "p", "r", Value::Null).await,
            Err(BrokerError::Conflict)
        ));
        for (table, partition, row) in [
            (Table::Members, "p", "r"),
            (Table::Workspaces, "other", "r"),
            (Table::Workspaces, "p", "other"),
        ] {
            store
                .insert(table, partition, row, Value::Null)
                .await
                .unwrap();
        }
        assert_eq!(store.list(Table::Workspaces, "p").await.unwrap().len(), 2);
        let updated = store
            .replace(
                Table::Workspaces,
                "p",
                "r",
                json!(["replaced"]),
                &entity.etag,
            )
            .await
            .unwrap();
        assert_ne!(updated.etag, entity.etag);
        assert_eq!(updated.data, json!(["replaced"]));
        assert!(matches!(
            store
                .replace(Table::Workspaces, "p", "r", Value::Null, &entity.etag)
                .await,
            Err(BrokerError::Conflict)
        ));
        assert!(matches!(
            store
                .delete(Table::Workspaces, "p", "r", &entity.etag)
                .await,
            Err(BrokerError::Conflict)
        ));
        store
            .delete(Table::Workspaces, "p", "r", &updated.etag)
            .await
            .unwrap();
        assert!(
            store
                .get(Table::Workspaces, "p", "r")
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            store
                .delete(Table::Workspaces, "p", "r", &updated.etag)
                .await,
            Err(BrokerError::NotFound)
        ));
        assert!(matches!(
            store
                .replace(Table::Workspaces, "p", "r", Value::Null, &updated.etag)
                .await,
            Err(BrokerError::NotFound)
        ));
        let reinserted = store
            .insert(Table::Workspaces, "p", "r", Value::Null)
            .await
            .unwrap();
        assert_ne!(reinserted.etag, updated.etag);
    }

    #[tokio::test]
    async fn storage_memory_compare_and_swap_is_atomic_between_tasks() {
        let store = Arc::new(InMemoryMetadataStore::new());
        let entity = store
            .insert(Table::Profiles, "p", "r", json!(0))
            .await
            .unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        for n in 0..32 {
            let store = store.clone();
            let etag = entity.etag.clone();
            tasks.spawn(async move {
                store
                    .replace(Table::Profiles, "p", "r", json!(n), &etag)
                    .await
            });
        }
        let mut successes = 0;
        while let Some(result) = tasks.join_next().await {
            match result.unwrap() {
                Ok(_) => successes += 1,
                Err(BrokerError::Conflict) => {}
                other => panic!("unexpected CAS result: {other:?}"),
            }
        }
        assert_eq!(successes, 1);
        store.state.write().await.version = u64::MAX;
        assert!(matches!(
            store
                .insert(Table::Profiles, "p", "overflow", Value::Null)
                .await,
            Err(BrokerError::Unavailable)
        ));
        assert!(
            store
                .get(Table::Profiles, "p", "overflow")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn storage_config_revisions_immutable_and_thread_safe() {
        let store: Arc<dyn ConfigStore> = Arc::new(InMemoryConfigStore::new());
        assert!(
            store
                .get_revision("workspace", "profile", "1")
                .await
                .unwrap()
                .is_none()
        );
        let yaml = "Name: café\nRules: []\n";
        store
            .put_revision("workspace", "profile", "1", yaml)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_revision("workspace", "profile", "1")
                .await
                .unwrap()
                .as_deref(),
            Some(yaml)
        );
        assert!(matches!(
            store
                .put_revision("workspace", "profile", "1", "overwrite")
                .await,
            Err(BrokerError::Conflict)
        ));
        assert!(
            store
                .get_revision("other", "profile", "1")
                .await
                .unwrap()
                .is_none()
        );
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let store = store.clone();
            tasks.spawn(async move { store.put_revision("workspace", "profile", "2", "x").await });
        }
        let mut successes = 0;
        while let Some(result) = tasks.join_next().await {
            match result.unwrap() {
                Ok(()) => successes += 1,
                Err(BrokerError::Conflict) => {}
                other => panic!("unexpected immutable write: {other:?}"),
            }
        }
        assert_eq!(successes, 1);
    }

    #[tokio::test]
    async fn storage_invalid_keys_and_wildcard_etags_are_rejected() {
        let store = InMemoryMetadataStore::new();
        for key in [
            "",
            "bad/key",
            "bad\\key",
            "bad#key",
            "bad?key",
            "bad\nkey",
            &"a".repeat(1025),
        ] {
            assert!(matches!(
                store.get(Table::Audit, key, "r").await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                store.insert(Table::Audit, "p", key, Value::Null).await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                store.list(Table::Audit, key).await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                store
                    .replace(Table::Audit, key, "r", Value::Null, "\"1\"")
                    .await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                store.delete(Table::Audit, "p", key, "\"1\"").await,
                Err(BrokerError::BadRequest)
            ));
        }
        for etag in ["", "*", " * ", "\"a\", *", "invalid", "\"bad\nvalue\""] {
            assert!(matches!(
                store
                    .replace(Table::Audit, "p", "r", Value::Null, etag)
                    .await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                store.delete(Table::Audit, "p", "r", etag).await,
                Err(BrokerError::BadRequest)
            ));
        }
        let configs = InMemoryConfigStore::new();
        for bad in ["", "..", "a/b", "a?b", "a%b", &"x".repeat(129)] {
            assert!(matches!(
                configs.put_revision(bad, "p", "1", "").await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                configs.get_revision("w", bad, "1").await,
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                configs.get_revision("w", "p", bad).await,
                Err(BrokerError::BadRequest)
            ));
        }
    }

    #[test]
    fn storage_table_names_and_cloud_endpoints_are_exact_and_scoped() {
        let tables = [
            (Table::Workspaces, "Workspaces"),
            (Table::Members, "Members"),
            (Table::UserWorkspaces, "UserWorkspaces"),
            (Table::PlatformAdmins, "PlatformAdmins"),
            (Table::Connections, "Connections"),
            (Table::Profiles, "Profiles"),
            (Table::Gateways, "Gateways"),
            (Table::Enrollments, "Enrollments"),
            (Table::Audit, "Audit"),
            (Table::RevisionMetadata, "RevisionMetadata"),
        ];
        for (table, name) in tables {
            assert_eq!(table.as_str(), name);
        }
        let metadata = AzureMetadataStore::new("broker123", credential()).unwrap();
        assert_eq!(
            metadata.endpoint.as_str(),
            "https://broker123.table.core.windows.net/"
        );
        let blobs = AzureBlobConfigStore::new("broker123", credential()).unwrap();
        assert_eq!(
            blobs.endpoint.as_str(),
            "https://broker123.blob.core.windows.net/"
        );
        for account in ["x", "UpperCase", "bad/account", "abc.evil", &"x".repeat(25)] {
            assert!(matches!(
                AzureMetadataStore::new(account, credential()),
                Err(BrokerError::BadRequest)
            ));
            assert!(matches!(
                AzureBlobConfigStore::new(account, credential()),
                Err(BrokerError::BadRequest)
            ));
        }
    }

    #[tokio::test]
    async fn storage_azure_table_crud_headers_and_json_wire_format() {
        let server = MockServer::start().await;
        let store = metadata(&server);
        let item_path = "/Workspaces(PartitionKey='p',RowKey='r')";
        let data = json!({"nested":[1,true],"name":"value"});
        Mock::given(method("POST"))
            .and(path("/Workspaces"))
            .and(header("authorization", "Bearer storage-test-token"))
            .and(header("x-ms-version", "2019-02-02"))
            .and(header("DataServiceVersion", "3.0;NetFx"))
            .and(header("MaxDataServiceVersion", "3.0;NetFx"))
            .and(header("accept", "application/json;odata=minimalmetadata"))
            .and(header("prefer", "return-no-content"))
            .and(body_json(table_body("p", "r", &data)))
            .respond_with(ResponseTemplate::new(204).insert_header("etag", "W/\"one\""))
            .expect(1)
            .mount(&server)
            .await;
        let inserted = store
            .insert(Table::Workspaces, "p", "r", data.clone())
            .await
            .unwrap();
        assert_eq!(inserted.etag, "W/\"one\"");
        Mock::given(method("GET"))
            .and(path(item_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"one\"")
                    .set_body_json(wire_entity("p", "r", "W/\"one\"")),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            store.get(Table::Workspaces, "p", "r").await.unwrap(),
            Some(inserted)
        );
        Mock::given(method("PUT"))
            .and(path(item_path))
            .and(header("if-match", "W/\"one\""))
            .and(body_json(table_body("p", "r", &json!("changed"))))
            .respond_with(ResponseTemplate::new(204).insert_header("etag", "\"two\""))
            .expect(1)
            .mount(&server)
            .await;
        let updated = store
            .replace(Table::Workspaces, "p", "r", json!("changed"), "W/\"one\"")
            .await
            .unwrap();
        assert_eq!(updated.etag, "\"two\"");
        Mock::given(method("DELETE"))
            .and(path(item_path))
            .and(header("if-match", "\"two\""))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        store
            .delete(Table::Workspaces, "p", "r", "\"two\"")
            .await
            .unwrap();
        for request in server.received_requests().await.unwrap() {
            assert_eq!(
                request.headers["authorization"],
                "Bearer storage-test-token"
            );
            let date = request.headers["x-ms-date"].to_str().unwrap();
            let parsed = httpdate::parse_http_date(date).unwrap();
            assert!(SystemTime::now().duration_since(parsed).unwrap() < Duration::from_secs(5));
        }
    }

    #[tokio::test]
    async fn storage_azure_table_missing_conflicts_and_dependency_failures() {
        let server = MockServer::start().await;
        let store = metadata(&server);
        for (status, code) in [
            (404, "ResourceNotFound"),
            (404, "TableNotFound"),
            (404, "Unknown"),
            (401, "AuthenticationFailed"),
            (403, "AuthorizationPermissionMismatch"),
            (500, "InternalError"),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(error_response(status, code))
                .mount(&server)
                .await;
            let result = store.get(Table::Members, "p", "r").await;
            if code == "ResourceNotFound" {
                assert!(result.unwrap().is_none());
            } else {
                assert!(
                    matches!(result, Err(BrokerError::Unavailable)),
                    "{status}/{code}"
                );
            }
        }
        for (verb, status, code, conflict) in [
            ("POST", 409, "EntityAlreadyExists", true),
            ("PUT", 412, "UpdateConditionNotSatisfied", true),
            ("DELETE", 412, "UpdateConditionNotSatisfied", true),
            ("PUT", 404, "ResourceNotFound", false),
            ("DELETE", 404, "EntityNotFound", false),
        ] {
            server.reset().await;
            Mock::given(method(verb))
                .respond_with(error_response(status, code))
                .mount(&server)
                .await;
            let result = match verb {
                "POST" => store
                    .insert(Table::Members, "p", "r", Value::Null)
                    .await
                    .map(|_| ()),
                "PUT" => store
                    .replace(Table::Members, "p", "r", Value::Null, "\"old\"")
                    .await
                    .map(|_| ()),
                _ => store.delete(Table::Members, "p", "r", "\"old\"").await,
            };
            if conflict {
                assert!(matches!(result, Err(BrokerError::Conflict)));
            } else {
                assert!(matches!(result, Err(BrokerError::NotFound)));
            }
        }
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(error_response(404, "TableNotFound"))
            .mount(&server)
            .await;
        assert!(matches!(
            store.list(Table::Members, "p").await,
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn storage_azure_table_list_follows_all_continuations_and_escapes_queries() {
        let server = MockServer::start().await;
        let store = metadata(&server);
        let partition = "p' or RowKey eq 'injected%20";
        let filter = format!("PartitionKey eq '{}'", partition.replace('\'', "''"));
        Mock::given(method("GET"))
            .and(path("/Profiles()"))
            .and(query_param("$filter", filter.clone()))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ms-continuation-NextPartitionKey", "next+p'%")
                    .insert_header("x-ms-continuation-NextRowKey", "row+%'")
                    .set_body_json(json!({"value":[wire_entity(partition, "one", "\"1\"")]})),
            )
            .with_priority(10)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Profiles()"))
            .and(query_param("$filter", filter.clone()))
            .and(query_param("NextPartitionKey", "next+p'%"))
            .and(query_param("NextRowKey", "row+%'"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ms-continuation-NextPartitionKey", "last")
                    .set_body_json(json!({"value":[]})),
            )
            .with_priority(5)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Profiles()"))
            .and(query_param("$filter", filter))
            .and(query_param("NextPartitionKey", "last"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value":[wire_entity(partition, "two", "\"2\"")]})),
            )
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        let entities = store.list(Table::Profiles, partition).await.unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].row_key, "one");
        assert_eq!(entities[1].row_key, "two");
        let url = store
            .entity_url(Table::Audit, "abc'percent%2F", "row'quote")
            .unwrap();
        assert_eq!(url.query(), None);
        assert!(url.as_str().contains("PartitionKey='abc''percent%252F'"));
        assert!(url.as_str().contains("RowKey='row''quote'"));
    }

    #[tokio::test]
    async fn storage_azure_table_bad_pages_never_return_partial_success() {
        for body in [
            json!({}),
            json!({"value":[{"PartitionKey":"p","RowKey":"r","Data":"not json","odata.etag":"\"1\""}]}),
            json!({"value":[{"PartitionKey":"p","RowKey":"r","Data":"{}"}]}),
            json!({"value":[wire_entity("different", "r", "\"1\"")]}),
        ] {
            let server = MockServer::start().await;
            let store = metadata(&server);
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            assert!(matches!(
                store.list(Table::Audit, "p").await,
                Err(BrokerError::Unavailable)
            ));
        }
        let server = MockServer::start().await;
        let store = metadata(&server);
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ms-continuation-NextPartitionKey", "same")
                    .set_body_json(json!({"value":[]})),
            )
            .expect(2)
            .mount(&server)
            .await;
        assert!(matches!(
            store.list(Table::Audit, "p").await,
            Err(BrokerError::Unavailable)
        ));
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ms-continuation-NextPartitionKey", "next")
                    .set_body_json(json!({"value":[wire_entity("p", "r", "\"1\"")]})),
            )
            .with_priority(10)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(query_param("NextPartitionKey", "next"))
            .respond_with(ResponseTemplate::new(503))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            store.list(Table::Audit, "p").await,
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn storage_azure_rejects_missing_etags_bad_json_redirects_and_credentials() {
        let server = MockServer::start().await;
        let store = metadata(&server);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        assert!(matches!(
            store.insert(Table::Workspaces, "p", "r", Value::Null).await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        assert!(matches!(
            store
                .replace(Table::Workspaces, "p", "r", Value::Null, "\"1\"")
                .await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        assert!(matches!(
            store.get(Table::Workspaces, "p", "r").await,
            Err(BrokerError::Unavailable)
        ));
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/Workspaces(PartitionKey='p',RowKey='r')"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/redirected", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/redirected"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        assert!(matches!(
            store.get(Table::Workspaces, "p", "r").await,
            Err(BrokerError::Unavailable)
        ));
        let failing = AzureMetadataStore::from_endpoint(
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            Arc::new(Credential { fail: true }),
        )
        .unwrap();
        assert!(matches!(
            failing.get(Table::Workspaces, "p", "r").await,
            Err(BrokerError::Unavailable)
        ));
    }

    // SDK bearer policy requires HTTPS. This test-only transport rewrites the
    // fixed account URL to wiremock after auth, without disabling SDK safeguards.
    #[derive(Debug)]
    struct LoopbackTransport {
        endpoint: Url,
        inner: Arc<dyn azure_core::http::HttpClient>,
    }

    #[async_trait]
    impl azure_core::http::HttpClient for LoopbackTransport {
        async fn execute_request(
            &self,
            request: &azure_core::http::Request,
        ) -> azure_core::Result<azure_core::http::AsyncRawResponse> {
            assert_eq!(request.url().scheme(), "https");
            assert_eq!(
                request.url().host_str(),
                Some("broker123.blob.core.windows.net")
            );
            let mut forwarded = request.clone();
            let mut url = self.endpoint.clone();
            url.set_path(request.url().path());
            url.set_query(request.url().query());
            *forwarded.url_mut() = url;
            self.inner.execute_request(&forwarded).await
        }
    }

    fn blobs(server: &MockServer) -> AzureBlobConfigStore {
        let mut store = AzureBlobConfigStore::new("broker123", credential()).unwrap();
        let endpoint = Url::parse(&server.uri()).unwrap();
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        store.options.client_options.transport = Some(azure_core::http::Transport::new(Arc::new(
            LoopbackTransport {
                endpoint,
                inner: azure_core::http::new_http_client(None),
            },
        )));
        store.options.client_options.retry = azure_core::http::RetryOptions::none();
        store
    }

    fn blob_response(status: u16) -> ResponseTemplate {
        ResponseTemplate::new(status)
            .insert_header("etag", "\"blob-etag\"")
            .insert_header("last-modified", "Wed, 14 Oct 2026 10:00:00 GMT")
            .insert_header("x-ms-version", "2025-11-05")
            .insert_header("x-ms-request-id", "test-request")
            .insert_header("x-ms-blob-type", "BlockBlob")
    }

    fn blob_failure(status: u16, code: &str) -> ResponseTemplate {
        ResponseTemplate::new(status)
            .insert_header("x-ms-error-code", code)
            .set_body_raw(
                format!("<Error><Code>{code}</Code><Message>not logged</Message></Error>"),
                "application/xml",
            )
    }

    #[tokio::test]
    async fn storage_azure_blob_immutable_upload_and_real_utf8_download() {
        let server = MockServer::start().await;
        let store = blobs(&server);
        let yaml = "Name: café\nRules: []\n";
        Mock::given(method("PUT"))
            .and(path("/configs/profiles/workspace/profile/000001.yaml"))
            .and(header("authorization", "Bearer storage-test-token"))
            .and(header("if-none-match", "*"))
            .and(header("x-ms-blob-type", "BlockBlob"))
            .and(wiremock::matchers::body_bytes(yaml.as_bytes().to_vec()))
            .respond_with(blob_response(201))
            .expect(1)
            .mount(&server)
            .await;
        store
            .put_revision("workspace", "profile", "1", yaml)
            .await
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/configs/profiles/workspace/profile/000001.yaml"))
            .and(header("authorization", "Bearer storage-test-token"))
            .respond_with(blob_response(200).set_body_bytes(yaml.as_bytes().to_vec()))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            store
                .get_revision("workspace", "profile", "1")
                .await
                .unwrap()
                .as_deref(),
            Some(yaml)
        );
    }

    #[tokio::test]
    async fn storage_azure_blob_errors_distinguish_missing_blob_from_dependency_failure() {
        let server = MockServer::start().await;
        let store = blobs(&server);
        for (status, code) in [
            (404, "BlobNotFound"),
            (404, "ContainerNotFound"),
            (403, "AuthorizationPermissionMismatch"),
            (401, "AuthenticationFailed"),
            (500, "InternalError"),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(blob_failure(status, code))
                .mount(&server)
                .await;
            let result = store.get_revision("w", "p", "1").await;
            if code == "BlobNotFound" {
                assert!(result.unwrap().is_none());
            } else {
                assert!(
                    matches!(result, Err(BrokerError::Unavailable)),
                    "{status}/{code}"
                );
            }
        }
        for (status, code) in [(409, "BlobAlreadyExists"), (412, "ConditionNotMet")] {
            server.reset().await;
            Mock::given(method("PUT"))
                .respond_with(blob_failure(status, code))
                .mount(&server)
                .await;
            assert!(matches!(
                store.put_revision("w", "p", "1", "").await,
                Err(BrokerError::Conflict)
            ));
        }
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(blob_response(200).set_body_bytes(vec![0xff, 0xfe]))
            .mount(&server)
            .await;
        assert!(matches!(
            store.get_revision("w", "p", "1").await,
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn storage_azure_timeouts_bound_real_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(wire_entity("p", "r", "\"one\""))
                    .set_delay(Duration::from_secs(1)),
            )
            .mount(&server)
            .await;
        let mut store = metadata(&server);
        store.request_timeout = Duration::from_millis(5);
        assert!(matches!(
            store.get(Table::Workspaces, "p", "r").await,
            Err(BrokerError::Unavailable)
        ));
        let mut store = blobs(&server);
        store.request_timeout = Duration::from_millis(5);
        assert!(matches!(
            store.get_revision("w", "p", "1").await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("PUT"))
            .respond_with(blob_response(201).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        assert!(matches!(
            store.put_revision("w", "p", "1", "x").await,
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn storage_azure_blob_redirects_credentials_and_write_errors_fail_closed() {
        let server = MockServer::start().await;
        let mut store = blobs(&server);
        Mock::given(method("GET"))
            .and(path("/configs/profiles/w/p/000001.yaml"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/redirected", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/redirected"))
            .respond_with(blob_response(200).set_body_string("not reached"))
            .expect(0)
            .mount(&server)
            .await;
        assert!(matches!(
            store.get_revision("w", "p", "1").await,
            Err(BrokerError::Unavailable)
        ));
        Mock::given(method("PUT"))
            .respond_with(blob_failure(404, "ContainerNotFound"))
            .mount(&server)
            .await;
        assert!(matches!(
            store.put_revision("w", "p", "1", "x").await,
            Err(BrokerError::Unavailable)
        ));
        store.credential = Arc::new(Credential { fail: true });
        assert!(matches!(
            store.get_revision("w", "p", "1").await,
            Err(BrokerError::Unavailable)
        ));
        assert!(matches!(
            store.put_revision("w", "p", "1", "x").await,
            Err(BrokerError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn storage_azure_rejects_corrupt_entities_and_canonicalizes_revision_names() {
        let server = MockServer::start().await;
        let store = metadata(&server);
        for body in [
            wire_entity("wrong", "r", "\"one\""),
            wire_entity("p", "wrong", "\"one\""),
            wire_entity("p", "r", "*"),
            wire_entity("p", "r", ""),
            json!({"PartitionKey":"p", "Data":"{}", "odata.etag":"\"one\""}),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            assert!(matches!(
                store.get(Table::Audit, "p", "r").await,
                Err(BrokerError::Unavailable)
            ));
        }
        assert_eq!(
            revision_path("workspace", "profile", "1").unwrap(),
            "profiles/workspace/profile/000001.yaml"
        );
        assert_eq!(
            revision_path("workspace", "profile", "1000000").unwrap(),
            "profiles/workspace/profile/1000000.yaml"
        );
        let configs = InMemoryConfigStore::new();
        configs.put_revision("w", "p", "1", "first").await.unwrap();
        assert!(matches!(
            configs.put_revision("w", "p", "000001", "different").await,
            Err(BrokerError::Conflict)
        ));
        assert_eq!(
            configs
                .get_revision("w", "p", "000001")
                .await
                .unwrap()
                .as_deref(),
            Some("first")
        );
    }

    #[tokio::test]
    async fn storage_factory_dev_and_app_state_do_not_downgrade_production() {
        let state = crate::AppState::initialize(settings(true)).await.unwrap();
        state
            .metadata_store
            .insert(Table::PlatformAdmins, "p", "1", json!(true))
            .await
            .unwrap();
        state
            .config_store
            .put_revision("w", "p", "1", "Name: test")
            .await
            .unwrap();
        assert!(matches!(
            crate::AppState::new(settings(false)),
            Err(BrokerError::Unavailable)
        ));
        assert!(matches!(
            storage_factory(&settings(false)).await,
            Err(BrokerError::Unavailable)
        ));
    }
}
