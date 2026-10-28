//! Broker-managed gateway enrollment, restricted state, and HTTP control plane.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use azbridge_broker_api::{
    EnrollmentPollResponse, EnrollmentStartRequest, EnrollmentStartResponse, GatewayConfigResponse,
    GatewayStatusRequest, RelayTokenRequest, RelayTokenResponse,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::cli::{Cli, ManagedCommand};
use crate::config::Config;
use crate::host::Host;

// Match azure-relay's listener renewal margin so a cached token cannot cause
// its renewal loop to repeatedly resend the same near-expiry SAS without sleep.
const TOKEN_RENEWAL_MARGIN: Duration = Duration::from_secs(300);

/// Secret-bearing state intentionally has no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayState {
    pub broker_url: String,
    pub workspace_id: String,
    pub gateway_id: String,
    pub profile: String,
    pub credential: String,
}

/// An auth-bearing HTTP client. Redirects are never followed, even same-origin.
#[derive(Clone)]
pub struct BrokerClient {
    http: reqwest::Client,
    base: Url,
    credential: String,
}

impl BrokerClient {
    pub fn new(broker_url: &str, credential: &str) -> anyhow::Result<Self> {
        let mut base = Url::parse(broker_url).context("Invalid broker URL")?;
        let loopback = base.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !(base.scheme() == "https" || base.scheme() == "http" && loopback)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            bail!("Broker URL must be an HTTPS origin (loopback HTTP is allowed for development)");
        }
        base.set_path("/");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .context("Cannot initialize broker HTTP client")?;
        Ok(Self {
            http,
            base,
            credential: credential.into(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .request(method, self.base.join(path).expect("static API path"));
        if !self.credential.is_empty() {
            request = request.bearer_auth(&self.credential);
        }
        request
    }

    async fn checked(request: reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
        // Do not expose reqwest errors, response bodies, or URLs: they may contain secrets.
        let response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Broker request failed"))?;
        if !response.status().is_success() {
            bail!(
                "Broker request rejected (HTTP {})",
                response.status().as_u16()
            );
        }
        Ok(response)
    }

    async fn json<T: serde::de::DeserializeOwned>(
        request: reqwest::RequestBuilder,
    ) -> anyhow::Result<T> {
        Self::checked(request)
            .await?
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Broker returned an invalid response"))
    }

    pub async fn config(&self) -> anyhow::Result<GatewayConfigResponse> {
        Self::json(self.request(reqwest::Method::GET, "api/v1/gateway/config")).await
    }

    pub async fn relay_token(
        &self,
        request: &RelayTokenRequest,
    ) -> anyhow::Result<RelayTokenResponse> {
        Self::json(
            self.request(reqwest::Method::POST, "api/v1/gateway/relay-tokens")
                .json(request),
        )
        .await
    }

    pub async fn status(&self, revision: u64) -> anyhow::Result<()> {
        Self::checked(
            self.request(reqwest::Method::POST, "api/v1/gateway/status")
                .json(&GatewayStatusRequest {
                    applied_revision: revision,
                    version: env!("CARGO_PKG_VERSION").into(),
                    hostname: gateway_hostname(),
                }),
        )
        .await?;
        Ok(())
    }

    pub async fn logout(&self) -> anyhow::Result<()> {
        Self::checked(self.request(reqwest::Method::POST, "api/v1/gateway/logout")).await?;
        Ok(())
    }

    pub async fn begin_enrollment(
        &self,
        request: &EnrollmentStartRequest,
    ) -> anyhow::Result<EnrollmentStartResponse> {
        Self::json(
            self.request(reqwest::Method::POST, "api/v1/enrollments")
                .json(request),
        )
        .await
    }

    pub async fn poll_enrollment(
        &self,
        enrollment_id: &str,
    ) -> anyhow::Result<EnrollmentPollResponse> {
        // IDs are a single URL segment, not arbitrary paths or query strings.
        if enrollment_id.is_empty()
            || !enrollment_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("Broker returned an invalid enrollment ID");
        }
        Self::json(self.request(
            reqwest::Method::POST,
            &format!("api/v1/enrollments/{enrollment_id}/poll"),
        ))
        .await
    }
}

fn gateway_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "azbridge".into())
}

pub fn state_path(machine: bool) -> anyhow::Result<PathBuf> {
    let config = if machine {
        crate::config_loader::machine_config_path()
    } else {
        crate::config_loader::user_config_path()
    }
    .context("Cannot determine gateway state directory")?;
    Ok(config
        .parent()
        .context("Invalid config directory")?
        .join("gateway.json"))
}

fn reject_links(path: &Path) -> anyhow::Result<()> {
    for component in path.ancestors() {
        match std::fs::symlink_metadata(component) {
            Ok(meta) => {
                #[cfg(windows)]
                let link = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0 // all reparse points, including junctions
                };
                #[cfg(not(windows))]
                let link = meta.file_type().is_symlink();
                if link {
                    bail!("Gateway state must not use symbolic links or reparse points");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub fn read_state(path: &Path) -> anyhow::Result<Option<GatewayState>> {
    reject_links(path)?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            bail!("Gateway state must have permissions 0600");
        }
    }
    #[cfg(windows)]
    check_restricted_dacl(&file)?;

    #[cfg(windows)]
    fn check_restricted_dacl(file: &std::fs::File) -> anyhow::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                    SE_FILE_OBJECT,
                },
                DACL_SECURITY_INFORMATION,
            },
        };
        unsafe {
            let mut descriptor = std::ptr::null_mut();
            let status = GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            );
            if status != 0 {
                return Err(std::io::Error::from_raw_os_error(status as i32).into());
            }
            let mut text = std::ptr::null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut(),
            );
            LocalFree(descriptor);
            if ok == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            // Accept only the protected ACL written by restricted_create. Inherited
            // or additional principals require a fresh enrollment, never silent repair.
            if sddl != "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)" {
                bail!("Gateway state must use a restricted DACL; enroll again");
            }
        }
        Ok(())
    }
    let state: GatewayState = serde_json::from_reader(file)
        .map_err(|_| anyhow::anyhow!("Gateway state is invalid; enroll again"))?;
    BrokerClient::new(&state.broker_url, &state.credential)?;
    if state.credential.is_empty() {
        bail!("Gateway state has no credential");
    }
    Ok(Some(state))
}

/// Create the replacement with restricted permissions *before* writing secrets,
/// then atomically rename. Never truncate/follow an existing state file.
pub fn write_state(path: &Path, state: &GatewayState) -> anyhow::Result<()> {
    use std::io::Write;
    BrokerClient::new(&state.broker_url, &state.credential)?;
    if state.credential.is_empty() {
        bail!("Gateway state has no credential");
    }
    reject_links(path)?;
    let parent = path
        .parent()
        .context("Gateway state needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    reject_links(parent)?;
    let staging = parent.join(format!(".gateway-{}.pending", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = restricted_create(&staging)?;
        let bytes = serde_json::to_vec(state).context("Cannot encode gateway state")?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        reject_links(path)?;
        std::fs::rename(&staging, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result
}

#[cfg(unix)]
fn restricted_create(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn restricted_create(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::{ffi::OsStrExt, io::FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE},
    };
    let sddl: Vec<u16> = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor);
        if handle == INVALID_HANDLE_VALUE {
            return Err(error);
        }
        Ok(std::fs::File::from_raw_handle(handle))
    }
}

fn environment_state() -> anyhow::Result<Option<GatewayState>> {
    let url = std::env::var("AZBRIDGE_BROKER_URL");
    let credential = std::env::var("AZBRIDGE_GATEWAY_CREDENTIAL");
    match (url, credential) {
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => Ok(None),
        (Ok(broker_url), Ok(credential)) if !broker_url.is_empty() && !credential.is_empty() => {
            BrokerClient::new(&broker_url, &credential)?;
            Ok(Some(GatewayState {
                broker_url,
                credential,
                workspace_id: String::new(),
                gateway_id: String::new(),
                profile: String::new(),
            }))
        }
        _ => bail!("AZBRIDGE_BROKER_URL and AZBRIDGE_GATEWAY_CREDENTIAL must both be nonempty"),
    }
}

pub fn discover_state(machine: bool) -> anyhow::Result<Option<GatewayState>> {
    // Services deliberately ignore the interactive user's environment and state.
    if !machine {
        if let Some(state) = environment_state()? {
            return Ok(Some(state));
        }
        if let Some(state) = read_state(&state_path(false)?)? {
            return Ok(Some(state));
        }
    }
    read_state(&state_path(true)?)
}

pub fn validate_managed_cli(cli: &Cli) -> anyhow::Result<()> {
    if cli.config_file.is_some()
        || cli.endpoint_uri.is_some()
        || cli.connection_string.is_some()
        || cli.shared_access_key_name.is_some()
        || cli.shared_access_key.is_some()
        || cli.signature.is_some()
        || !cli.local_forward.is_empty()
        || !cli.remote_forward.is_empty()
        || !cli.remote_forward_legacy.is_empty()
        || !cli.remote_http_forward.is_empty()
        || !cli.option.is_empty()
        || cli.gateway_ports
        || cli.bind_address.is_some()
        || cli.keep_alive_interval.is_some()
        || cli.tui
    {
        bail!(
            "Managed mode accepts only local logging flags; forwarding, credentials, config overrides, network flags and TUI are broker-controlled"
        );
    }
    Ok(())
}

pub async fn handle_command(command: &ManagedCommand) -> anyhow::Result<()> {
    match command {
        ManagedCommand::Login {
            broker,
            workspace,
            profile,
            name,
            machine,
        } => {
            let path = state_path(*machine)?;
            if read_state(&path)?.is_some() {
                bail!("Gateway is already enrolled in this scope; logout before enrolling again");
            }
            let client = BrokerClient::new(broker, "")?;
            let start = client
                .begin_enrollment(&EnrollmentStartRequest {
                    workspace: workspace.clone(),
                    profile: profile.clone(),
                    gateway_name: name.clone().unwrap_or_else(gateway_hostname),
                })
                .await?;
            println!(
                "Open {} and enter code {}",
                start.verification_uri, start.user_code
            );
            let deadline =
                tokio::time::Instant::now() + Duration::from_secs(start.expires_in.min(3600));
            let mut interval = start.interval.clamp(1, 3600);
            loop {
                if tokio::time::Instant::now() + Duration::from_secs(interval) >= deadline {
                    bail!("Enrollment expired; run login again");
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
                match client.poll_enrollment(&start.enrollment_id).await? {
                    EnrollmentPollResponse::Pending { interval: next }
                    | EnrollmentPollResponse::SlowDown { interval: next } => {
                        interval = interval.max(next).clamp(1, 3600)
                    }
                    EnrollmentPollResponse::Authorized {
                        credential,
                        workspace_id,
                        gateway_id,
                        profile_id,
                    } => {
                        let state = GatewayState {
                            broker_url: broker.clone(),
                            workspace_id,
                            gateway_id,
                            profile: profile_id,
                            credential,
                        };
                        if let Err(error) = write_state(&path, &state) {
                            let _ = BrokerClient::new(broker, &state.credential)?.logout().await;
                            return Err(error);
                        }
                        println!(
                            "Gateway enrolled. Restart azbridge to fetch its assigned profile."
                        );
                        return Ok(());
                    }
                    EnrollmentPollResponse::Expired => bail!("Enrollment expired; run login again"),
                    EnrollmentPollResponse::Denied => bail!("Enrollment denied"),
                }
            }
        }
        ManagedCommand::Logout { machine } => {
            let state = discover_state(*machine)?.context("Gateway is not enrolled")?;
            BrokerClient::new(&state.broker_url, &state.credential)?
                .logout()
                .await?;
            // Environment overrides do not describe the on-disk credential.
            if *machine || environment_state()?.is_none() {
                let path = if *machine || read_state(&state_path(false)?)?.is_none() {
                    state_path(true)?
                } else {
                    state_path(false)?
                };
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
            println!(
                "Gateway revoked. Remove gateway credential environment overrides if configured."
            );
            Ok(())
        }
        ManagedCommand::Status { machine } => {
            let Some(state) = discover_state(*machine)? else {
                println!("Local mode (not enrolled)");
                return Ok(());
            };
            let config = BrokerClient::new(&state.broker_url, &state.credential)?
                .config()
                .await?;
            println!(
                "Managed gateway {}: broker reachable, assigned revision {} (restart to apply)",
                state.gateway_id, config.revision
            );
            Ok(())
        }
    }
}

pub struct Bootstrap {
    pub config: Config,
    pub managed: Option<(Arc<BrokerClient>, u64)>,
}

/// Shared fail-fast configuration bootstrap for foreground and Windows service.
pub async fn bootstrap(cli: &Cli, machine: bool) -> anyhow::Result<Bootstrap> {
    if let Some(state) = discover_state(machine)? {
        validate_managed_cli(cli)?;
        let client = Arc::new(BrokerClient::new(&state.broker_url, &state.credential)?);
        let response = client.config().await?;
        let mut config = response.config;
        validate_managed_config(&config)?;
        Url::parse(&response.relay_endpoint)
            .context("Broker returned an invalid Relay endpoint")?;
        if config.local_forward.is_empty() && config.remote_forward.is_empty() {
            bail!("Broker profile has no forwarding rules");
        }
        config.azure_relay_endpoint = Some(response.relay_endpoint);
        for forward in &mut config.local_forward {
            forward.normalize();
        }
        for forward in &mut config.remote_forward {
            forward.normalize();
        }
        config.validate()?;
        Ok(Bootstrap {
            config,
            managed: Some((client, response.revision)),
        })
    } else {
        let config = crate::config_loader::load_config(cli)?;
        config.validate()?;
        Ok(Bootstrap {
            config,
            managed: None,
        })
    }
}

fn validate_managed_config(config: &Config) -> anyhow::Result<()> {
    if config.azure_relay_connection_string.is_some()
        || config.azure_relay_endpoint.is_some()
        || config.azure_relay_shared_access_key_name.is_some()
        || config.azure_relay_shared_access_key.is_some()
        || config.azure_relay_shared_access_signature.is_some()
        || config
            .local_forward
            .iter()
            .any(|f| f.connection_string.is_some())
        || config
            .remote_forward
            .iter()
            .any(|f| f.connection_string.is_some())
        || config.log_file_name.is_some()
        || config.log_level.is_some()
    {
        bail!("Broker profile must not contain Relay credentials or endpoint overrides");
    }
    Ok(())
}

impl Bootstrap {
    pub fn host(&self) -> anyhow::Result<Host> {
        if let Some((client, _)) = &self.managed {
            return Host::with_broker(self.config.clone(), client.clone());
        }
        Ok(Host::new(self.config.clone()))
    }
}

/// HC-scoped renewable broker tokens. Each instance belongs to one relay/right;
/// Host shares it across all bindings of that relay/right. Broker lifetimes are
/// capped at one hour, so `valid_for` is advisory (the Relay SDK requests 1h).
/// The returned actual expiry drives the listener's existing renewal loop.
pub struct BrokerTokenProvider {
    client: Arc<BrokerClient>,
    relay_name: String,
    right: azbridge_broker_api::RelayRight,
    audience: String,
    cache: tokio::sync::Mutex<Option<azure_relay::token_provider::SecurityToken>>,
}

impl BrokerTokenProvider {
    pub fn new(
        client: Arc<BrokerClient>,
        endpoint: &Url,
        relay_name: &str,
        right: azbridge_broker_api::RelayRight,
    ) -> azure_relay::Result<Self> {
        if relay_name.is_empty()
            || relay_name
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
            || !relay_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-._".contains(&b))
        {
            return Err(azure_relay::RelayError::invalid_argument(
                "relay_name",
                "invalid broker relay scope",
            ));
        }
        let address = azure_relay::build_relay_address(endpoint, relay_name)?;
        Ok(Self {
            client,
            relay_name: relay_name.into(),
            right,
            audience: azure_relay::protocol::normalize_audience(&address),
            cache: tokio::sync::Mutex::new(None),
        })
    }

    fn validate_token(
        &self,
        response: RelayTokenResponse,
    ) -> azure_relay::Result<azure_relay::token_provider::SecurityToken> {
        use azure_relay::RelayError;
        use std::time::{SystemTime, UNIX_EPOCH};
        let invalid = || {
            RelayError::AuthorizationFailed(
                "Broker returned an invalid or incorrectly scoped Relay token".into(),
            )
        };
        let fields = response
            .token
            .strip_prefix("SharedAccessSignature ")
            .ok_or_else(invalid)?;
        let mut params = std::collections::HashMap::new();
        for (key, value) in url::form_urlencoded::parse(fields.as_bytes()) {
            if params
                .insert(key.into_owned(), value.into_owned())
                .is_some()
            {
                return Err(invalid());
            }
        }
        if params.get("sr") != Some(&self.audience)
            || params.get("se").and_then(|s| s.parse::<u64>().ok()) != Some(response.expires_at)
            || params.get("sig").is_none_or(String::is_empty)
            || params.get("skn").is_none_or(String::is_empty)
        {
            return Err(invalid());
        }
        let expires_at = UNIX_EPOCH
            .checked_add(Duration::from_secs(response.expires_at))
            .ok_or_else(invalid)?;
        let remaining = expires_at
            .duration_since(SystemTime::now())
            .map_err(|_| invalid())?;
        if remaining <= TOKEN_RENEWAL_MARGIN || remaining > Duration::from_secs(3660) {
            return Err(RelayError::AuthorizationFailed(
                "Broker Relay token is expired, near expiry, or exceeds the one-hour lifetime"
                    .into(),
            ));
        }
        Ok(azure_relay::token_provider::SecurityToken {
            token: response.token,
            expires_at,
        })
    }
}

impl azure_relay::token_provider::TokenProvider for BrokerTokenProvider {
    async fn get_token(
        &self,
        audience: &str,
        valid_for: Duration,
    ) -> azure_relay::Result<azure_relay::token_provider::SecurityToken> {
        use azure_relay::RelayError;
        if audience != self.audience {
            return Err(RelayError::AuthorizationFailed(
                "Broker token audience does not match the configured relay".into(),
            ));
        }
        if valid_for.is_zero() {
            return Err(RelayError::invalid_argument(
                "valid_for",
                "must be greater than zero",
            ));
        }
        let mut cache = self.cache.lock().await;
        if let Some(token) = cache.as_ref()
            && token
                .expires_at
                .duration_since(std::time::SystemTime::now())
                .is_ok_and(|remaining| remaining > TOKEN_RENEWAL_MARGIN)
        {
            return Ok(token.clone());
        }
        // Never fall back to stale tokens on a broker error/revocation.
        *cache = None;
        let response = self
            .client
            .relay_token(&RelayTokenRequest {
                relay_name: self.relay_name.clone(),
                right: self.right,
            })
            .await
            .map_err(|error| {
                RelayError::AuthorizationFailed(format!("Broker token request failed: {error}"))
            })?;
        let token = self.validate_token(response)?;
        *cache = Some(token.clone());
        Ok(token)
    }
}

impl Bootstrap {
    pub async fn start(&self) -> anyhow::Result<Arc<Host>> {
        let host = Arc::new(self.host()?);
        if let Err(error) = host.start().await {
            host.stop().await;
            return Err(error);
        }
        if let Some((client, revision)) = &self.managed
            && let Err(error) = client.status(*revision).await
        {
            host.stop().await;
            return Err(error);
        }
        Ok(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use azure_relay::token_provider::{SecurityToken, TokenProvider};

    #[tokio::test]
    async fn cached_token_inside_listener_renewal_margin_is_never_reused() {
        let provider = BrokerTokenProvider::new(
            Arc::new(BrokerClient::new("http://127.0.0.1:1", "credential").unwrap()),
            &Url::parse("sb://ns.servicebus.windows.net").unwrap(),
            "relay",
            azbridge_broker_api::RelayRight::Listen,
        )
        .unwrap();
        *provider.cache.lock().await = Some(SecurityToken {
            token: "stale-token-must-not-be-returned".into(),
            expires_at: std::time::SystemTime::now() + Duration::from_secs(250),
        });
        assert!(
            provider
                .get_token(&provider.audience, Duration::from_secs(3600))
                .await
                .is_err()
        );
        assert!(provider.cache.lock().await.is_none());
    }

    #[test]
    fn managed_profile_rejects_local_credentials_and_logging() {
        let mut config = Config::default();
        validate_managed_config(&config).unwrap();
        config.azure_relay_connection_string = Some("not-allowed".into());
        assert!(validate_managed_config(&config).is_err());
        config.azure_relay_connection_string = None;
        config.azure_relay_endpoint = Some("sb://override".into());
        assert!(validate_managed_config(&config).is_err());
        config.azure_relay_endpoint = None;
        config.azure_relay_shared_access_key_name = Some("key-name".into());
        assert!(validate_managed_config(&config).is_err());
        config.azure_relay_shared_access_key_name = None;
        config.azure_relay_shared_access_key = Some("key".into());
        assert!(validate_managed_config(&config).is_err());
        config.azure_relay_shared_access_key = None;
        config.azure_relay_shared_access_signature = Some("token".into());
        assert!(validate_managed_config(&config).is_err());
        config.azure_relay_shared_access_signature = None;
        config.log_file_name = Some("file".into());
        assert!(validate_managed_config(&config).is_err());
        config.log_file_name = None;
        config.log_level = Some("DEBUG".into());
        assert!(validate_managed_config(&config).is_err());
        config.log_level = None;
        config.local_forward.push(crate::config::LocalForward {
            connection_string: Some("override".into()),
            ..Default::default()
        });
        assert!(validate_managed_config(&config).is_err());
        config.local_forward.clear();
        config.remote_forward.push(crate::config::RemoteForward {
            connection_string: Some("override".into()),
            ..Default::default()
        });
        assert!(validate_managed_config(&config).is_err());
    }
}
