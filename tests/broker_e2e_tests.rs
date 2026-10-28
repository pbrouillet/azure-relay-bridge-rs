//! Joint broker/CLI regression, entirely offline. Only the cloud transport is
//! substituted: the real config endpoint is projected onto a loopback mock and
//! a RelayManager adapter signs that audience after the normal fake resource lookup.
//! Broker authentication, publishing, entitlements and revocation are unmodified.
use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{body::Body, middleware::Next};
use azbridge_broker::{
    AppState,
    error::BrokerError,
    relay::FakeRelayManager,
    relay::RelayManager,
    settings::{Args, Settings},
};
use azbridge_broker_api::{ProvisioningState, RelayRight, RelayTokenResponse};
use azure_relay::token_provider::{SharedAccessSignatureTokenProvider, TokenProvider};
use clap::Parser;
use reqwest::{Client, Method, StatusCode, header};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Observations {
    configs: tokio::sync::Mutex<Vec<Value>>,
    issued: tokio::sync::Mutex<Vec<(String, String, RelayRight)>>,
}

struct LocalRelay {
    resources: FakeRelayManager,
    observations: Arc<Observations>,
}

#[async_trait]
impl RelayManager for LocalRelay {
    async fn ensure_namespace(
        &self,
        namespace: &str,
        location: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        self.resources.ensure_namespace(namespace, location).await
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<bool, BrokerError> {
        self.resources.delete_namespace(namespace).await
    }

    async fn ensure_connection(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<ProvisioningState, BrokerError> {
        self.resources.ensure_connection(namespace, name).await
    }

    async fn delete_connection(&self, namespace: &str, name: &str) -> Result<bool, BrokerError> {
        self.resources.delete_connection(namespace, name).await
    }

    async fn rotate_keys(&self, namespace: &str, name: &str) -> Result<(), BrokerError> {
        self.resources.rotate_keys(namespace, name).await
    }

    async fn issue_token(
        &self,
        namespace: &str,
        relay_name: &str,
        right: RelayRight,
    ) -> Result<RelayTokenResponse, BrokerError> {
        // Preserve the real fake manager's resource existence and scope checks.
        self.resources
            .issue_token(namespace, relay_name, right)
            .await?;
        let policy = match right {
            RelayRight::Send => "azbridge-send",
            RelayRight::Listen => "azbridge-listen",
        };
        let signer =
            SharedAccessSignatureTokenProvider::new(policy, "bG9jYWwtZTJlLWZpeHR1cmUta2V5")
                .map_err(|_| BrokerError::Unavailable)?;
        let token = signer
            .get_token(
                &format!("http://127.0.0.1/{relay_name}/"),
                Duration::from_secs(3600),
            )
            .await
            .map_err(|_| BrokerError::Unavailable)?;
        self.observations
            .issued
            .lock()
            .await
            .push((namespace.into(), relay_name.into(), right));
        Ok(RelayTokenResponse {
            token: token.token,
            expires_at: token
                .expires_at
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| BrokerError::Unavailable)?
                .as_secs(),
        })
    }
}

struct Harness {
    origin: String,
    http: Client,
    browser: header::HeaderMap,
    directory: PathBuf,
    observations: Arc<Observations>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Harness {
    async fn start(relay_port: u16) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let settings = Settings::from_args(
            Args::try_parse_from([
                "broker",
                "--dev",
                "--listen-addr",
                &address.to_string(),
                "--public-url",
                &origin,
            ])
            .unwrap(),
        )
        .unwrap();
        let mut state = AppState::new(settings).unwrap();
        let observations = Arc::new(Observations::default());
        state.relay_manager = Arc::new(LocalRelay {
            resources: FakeRelayManager::new(),
            observations: observations.clone(),
        });
        let recorded = observations.clone();
        let endpoint = format!("ws://127.0.0.1:{relay_port}");
        // Test-only cloud transport projection; never replace a broker handler,
        // credential check, config lookup, or token authorization decision.
        let app = azbridge_broker::app(state.clone()).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: Next| {
                let endpoint = endpoint.clone();
                let recorded = recorded.clone();
                async move {
                    let config = request.uri().path() == "/api/v1/gateway/config";
                    let response = next.run(request).await;
                    if !config || response.status() != StatusCode::OK {
                        return response;
                    }
                    let (mut parts, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                    recorded.configs.lock().await.push(value.clone());
                    value["relayEndpoint"] = json!(endpoint);
                    parts.headers.remove(header::CONTENT_LENGTH);
                    axum::response::Response::from_parts(parts, Body::from(value.to_string()))
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let worker = tokio::spawn(async move {
            // Keep the shutdown sender alive until the task is aborted by Drop.
            let _shutdown = shutdown;
            azbridge_broker::reconciler::run_reconciler(state, receiver).await;
        });
        let directory = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("broker-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut harness = Self {
            origin,
            http: Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            browser: header::HeaderMap::new(),
            directory,
            observations,
            tasks: vec![server, worker],
        };
        let health = harness
            .http
            .get(format!("{}/healthz", harness.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        assert_eq!(health.json::<Value>().await.unwrap()["mode"], "development");
        let login = harness
            .http
            .post(format!("{}/auth/dev", harness.origin))
            .header(header::ORIGIN, &harness.origin)
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        harness
            .browser
            .insert(header::COOKIE, cookie.parse().unwrap());
        harness
            .browser
            .insert(header::ORIGIN, harness.origin.parse().unwrap());
        let me = harness.call(Method::GET, "/api/v1/me", None, 200).await;
        assert_eq!(me["platformAdmin"], true);
        harness.browser.insert(
            "x-csrf-token",
            me["csrfToken"].as_str().unwrap().parse().unwrap(),
        );
        harness
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>, status: u16) -> Value {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.origin))
            .headers(self.browser.clone());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{path}");
        if status == 204 {
            Value::Null
        } else {
            response.json().await.unwrap()
        }
    }

    async fn ready(&self, path: &str) {
        tokio::time::timeout(DEADLINE, async {
            loop {
                if self.call(Method::GET, path, None, 200).await["state"] == "ready" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("broker resource did not become Ready before deadline");
    }

    fn cli(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_azbridge"));
        command
            .env("APPDATA", &self.directory)
            .env("HOME", &self.directory)
            .env("XDG_CONFIG_HOME", &self.directory)
            .env("PROGRAMDATA", self.directory.join("machine"))
            .env_remove("AZBRIDGE_BROKER_URL")
            .env_remove("AZBRIDGE_GATEWAY_CREDENTIAL")
            .env_remove("AZURE_BRIDGE_CONNECTIONSTRING")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        command
    }

    async fn cli_output(&self, args: &[&str]) -> std::process::Output {
        let output = tokio::time::timeout(DEADLINE, self.cli().args(args).output())
            .await
            .expect("CLI command exceeded deadline")
            .unwrap();
        // Never include captured output in failures: it may contain a credential.
        assert!(output.status.success(), "CLI command failed: {}", args[0]);
        output
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_broker_manages_cli_tunnel_entitlements_and_revocation() {
    let relay = azure_relay::test_utils::MockRelayServer::start().await;
    let mut harness = Harness::start(relay.port).await;
    let anonymous = harness
        .http
        .get(format!("{}/api/v1/gateway/config", harness.origin))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let mut missing_csrf = harness.browser.clone();
    missing_csrf.remove("x-csrf-token");
    let rejected = harness
        .http
        .post(format!("{}/api/v1/workspaces", harness.origin))
        .headers(missing_csrf)
        .json(&json!({"name":"must-not-exist", "location":"westus"}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = echo.local_addr().unwrap();
    harness.tasks.push(tokio::spawn(async move {
        loop {
            let (stream, _) = echo.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            let _ = tokio::io::copy(&mut read, &mut write).await;
        }
    }));
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = reservation.local_addr().unwrap();
    let send_reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let send_port = send_reservation.local_addr().unwrap().port();
    drop(reservation);
    drop(send_reservation);

    let workspace = harness
        .call(
            Method::POST,
            "/api/v1/workspaces",
            Some(json!({
                "name": "E2E workspace", "location": "westus"
            })),
            201,
        )
        .await;
    let ws = workspace["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{ws}");
    harness.ready(&base).await;
    for name in ["echo", "send-only", "unentitled"] {
        harness
            .call(
                Method::POST,
                &format!("{base}/connections"),
                Some(json!({"name":name})),
                201,
            )
            .await;
    }
    tokio::time::timeout(DEADLINE, async {
        loop {
            let connections = harness
                .call(Method::GET, &format!("{base}/connections"), None, 200)
                .await;
            if connections
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["state"] == "ready")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("connections did not become Ready");
    let profile = harness
        .call(
            Method::POST,
            &format!("{base}/profiles"),
            Some(json!({"name":"echo-profile"})),
            201,
        )
        .await;
    let profile_id = profile["id"].as_str().unwrap();
    let yaml = format!(
        "ConnectTimeout: 5\nLocalForward:\n- RelayName: echo\n  BindAddress: 127.0.0.1\n  BindPort: {}\n  PortName: echo\n- RelayName: send-only\n  BindAddress: 127.0.0.1\n  BindPort: {send_port}\nRemoteForward:\n- RelayName: echo\n  Host: 127.0.0.1\n  HostPort: {}\n  PortName: echo\n",
        local.port(),
        target.port()
    );
    let profile_base = format!("{base}/profiles/{profile_id}");
    let revision = harness
        .call(
            Method::POST,
            &format!("{profile_base}/revisions"),
            Some(json!({"yaml":yaml})),
            201,
        )
        .await;
    let revision = revision["revision"].as_u64().unwrap();
    harness
        .call(
            Method::POST,
            &format!("{profile_base}/publish"),
            Some(json!({"revision":revision})),
            200,
        )
        .await;
    let precreated = harness
        .call(
            Method::POST,
            &format!("{base}/gateways"),
            Some(json!({"name":"precreated", "profileId":profile_id})),
            201,
        )
        .await;
    let credential = precreated["credential"].as_str().unwrap();
    let gateway_id = precreated["gateway"]["id"].as_str().unwrap();

    let login = harness
        .cli_output(&[
            "login",
            "--broker",
            &harness.origin,
            "--workspace",
            ws,
            "--profile",
            profile_id,
            "--name",
            "enrolled-cli",
        ])
        .await;
    assert!(String::from_utf8_lossy(&login.stdout).contains("Gateway enrolled"));
    #[cfg(windows)]
    let state_path = harness.directory.join("azbridge").join("gateway.json");
    #[cfg(not(windows))]
    let state_path = harness.directory.join(".azbridge").join("gateway.json");
    let enrolled = azbridge_lib::broker::read_state(&state_path)
        .unwrap()
        .unwrap();
    assert_eq!(enrolled.workspace_id, ws);
    assert_eq!(enrolled.profile, profile_id);
    for output in [&login.stdout, &login.stderr] {
        assert!(!String::from_utf8_lossy(output).contains(&enrolled.credential));
    }
    let status = harness.cli_output(&["status"]).await;
    assert!(String::from_utf8_lossy(&status.stdout).contains(&format!("revision {revision}")));

    let client = azbridge_lib::broker::BrokerClient::new(&harness.origin, credential).unwrap();
    let config = client.config().await.unwrap();
    assert_eq!(config.revision, revision);
    assert_eq!(config.config.local_forward[0].relay_name, "echo");
    let token = client
        .relay_token(&azbridge_broker_api::RelayTokenRequest {
            relay_name: "echo".into(),
            right: RelayRight::Send,
        })
        .await
        .unwrap();
    let fields: std::collections::HashMap<_, _> = url::form_urlencoded::parse(
        token
            .token
            .strip_prefix("SharedAccessSignature ")
            .expect("broker must return an SAS")
            .as_bytes(),
    )
    .into_owned()
    .collect();
    assert!(
        fields
            .get("sr")
            .is_some_and(|v| v == "http://127.0.0.1/echo/")
    );
    assert!(fields.get("skn").is_some_and(|v| v == "azbridge-send"));
    assert!(
        fields
            .get("se")
            .is_some_and(|v| v.parse::<u64>().ok() == Some(token.expires_at))
    );
    assert!(fields.get("sig").is_some_and(|v| !v.is_empty()));
    let mut command = harness.cli();
    let mut child = command
        .arg("-q")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "managed CLI exited before readiness"
            );
            let gateways = harness
                .call(Method::GET, &format!("{base}/gateways"), None, 200)
                .await;
            if gateways.as_array().unwrap().iter().any(|gateway| {
                gateway["id"] == enrolled.gateway_id && gateway["appliedRevision"] == revision
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("managed CLI failed to report applied revision");
    let mut stream = tokio::net::TcpStream::connect(local).await.unwrap();
    stream.write_all(b"joint broker CLI tunnel").await.unwrap();
    let mut echoed = [0; 23];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut echoed))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&echoed, b"joint broker CLI tunnel");
    drop(stream);

    let token_path = format!("{}/api/v1/gateway/relay-tokens", harness.origin);
    for (name, right) in [("unentitled", "send"), ("send-only", "listen")] {
        let response = harness
            .http
            .post(&token_path)
            .bearer_auth(credential)
            .json(&json!({"relayName":name, "right":right}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let issued = harness.observations.issued.lock().await;
    assert!(
        issued
            .iter()
            .any(|(_, name, right)| name == "echo" && *right == RelayRight::Send)
    );
    assert!(
        issued
            .iter()
            .any(|(_, name, right)| name == "echo" && *right == RelayRight::Listen)
    );
    assert!(!issued.iter().any(|(_, name, right)| name == "unentitled"
        || name == "send-only" && *right == RelayRight::Listen));
    assert!(
        issued
            .iter()
            .all(|(namespace, _, _)| namespace == workspace["namespaceName"].as_str().unwrap())
    );
    let issued_count = issued.len();
    drop(issued);
    let configs = harness.observations.configs.lock().await;
    assert!(
        configs.len() >= 3,
        "login status, explicit fetch and CLI bootstrap must fetch real config"
    );
    assert!(configs.iter().all(|config| config["relayEndpoint"]
        == format!(
            "sb://{}.servicebus.windows.net/",
            workspace["namespaceName"].as_str().unwrap()
        )
        && config["revision"] == revision));
    drop(configs);

    for id in [gateway_id, &enrolled.gateway_id] {
        harness
            .call(Method::DELETE, &format!("{base}/gateways/{id}"), None, 204)
            .await;
    }
    for credential in [credential, &enrolled.credential] {
        let response = harness
            .http
            .get(format!("{}/api/v1/gateway/config", harness.origin))
            .bearer_auth(credential)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = harness
            .http
            .post(&token_path)
            .bearer_auth(credential)
            .json(&json!({"relayName":"echo", "right":"send"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let revoked = azbridge_lib::broker::BrokerClient::new(&harness.origin, credential).unwrap();
        assert!(revoked.status(revision).await.is_err());
    }
    assert_eq!(harness.observations.issued.lock().await.len(), issued_count);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    relay.stop().await;
}
