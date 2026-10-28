use azbridge_lib::{
    broker::{BrokerClient, GatewayState, read_state, validate_managed_cli, write_state},
    cli::Cli,
};
use clap::Parser;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct MockBroker {
    origin: String,
    requests: Arc<tokio::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockBroker {
    async fn start(responses: Vec<(u16, serde_json::Value)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let responses = Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::from(
            responses,
        )));
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let recorded = recorded.clone();
                let responses = responses.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let header_len = loop {
                        let mut chunk = [0u8; 4096];
                        let count = stream.read(&mut chunk).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&bytes[..header_len]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while bytes.len() < header_len + length {
                        let mut chunk = [0u8; 4096];
                        let count = stream.read(&mut chunk).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                    }
                    recorded
                        .lock()
                        .await
                        .push(String::from_utf8(bytes).unwrap());
                    let (status, body) = responses
                        .lock()
                        .await
                        .pop_front()
                        .unwrap_or((500, serde_json::json!({})));
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/stolen\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        Self {
            origin,
            requests,
            task,
        }
    }
}

impl Drop for MockBroker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn sas_response(audience: &str, expires_at: u64) -> serde_json::Value {
    serde_json::json!({
        "token":format!("SharedAccessSignature sr={}&sig=signature&se={expires_at}&skn=azbridge-send",
            azure_relay::token_provider::url_encode(audience)),
        "expiresAt":expires_at
    })
}

fn future_expiry(seconds: u64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + seconds
}

#[tokio::test]
async fn broker_tokens_are_scoped_cached_and_right_specific() {
    use azbridge_broker_api::RelayRight;
    use azbridge_lib::broker::BrokerTokenProvider;
    use azure_relay::token_provider::TokenProvider;
    use std::time::Duration;
    let audience = "http://ns.servicebus.windows.net/relay/";
    let broker = MockBroker::start(vec![
        (200, sas_response(audience, future_expiry(3600))),
        (200, sas_response(audience, future_expiry(3600))),
    ])
    .await;
    let client = Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap());
    let endpoint = url::Url::parse("sb://ns.servicebus.windows.net").unwrap();
    let provider = Arc::new(
        BrokerTokenProvider::new(client.clone(), &endpoint, "relay", RelayRight::Send).unwrap(),
    );
    assert!(
        provider
            .get_token(
                "http://ns.servicebus.windows.net/other/",
                Duration::from_secs(3600)
            )
            .await
            .is_err()
    );
    assert!(provider.get_token(audience, Duration::ZERO).await.is_err());
    let (first, second) = tokio::join!(
        provider.get_token(audience, Duration::from_secs(3600)),
        provider.get_token(audience, Duration::from_secs(7200)),
    );
    assert_eq!(first.unwrap().token, second.unwrap().token);
    assert_eq!(broker.requests.lock().await.len(), 1);
    let listen = BrokerTokenProvider::new(client, &endpoint, "relay", RelayRight::Listen).unwrap();
    listen
        .get_token(audience, Duration::from_secs(3600))
        .await
        .unwrap();
    let requests = broker.requests.lock().await;
    assert!(requests[0].contains("\"right\":\"send\""));
    assert!(requests[1].contains("\"right\":\"listen\""));
    assert!(
        requests
            .iter()
            .all(|r| r.contains("\"relayName\":\"relay\""))
    );
    for invalid in ["", "..", "a/../b", "relay?x=1", "relay/"] {
        assert!(
            BrokerTokenProvider::new(
                Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
                &endpoint,
                invalid,
                RelayRight::Send
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn broker_tokens_reject_expiry_scope_format_and_broker_errors() {
    use azbridge_broker_api::RelayRight;
    use azbridge_lib::broker::BrokerTokenProvider;
    use azure_relay::token_provider::TokenProvider;
    use std::time::Duration;
    let audience = "http://ns.servicebus.windows.net/relay/";
    let expires = future_expiry(3600);
    let broker = MockBroker::start(vec![
            (200, sas_response(audience, 1)),
            (200, sas_response(audience, future_expiry(30))),
            (200, sas_response(audience, future_expiry(7200))),
            (200, sas_response("http://ns.servicebus.windows.net/", expires)),
            (200, serde_json::json!({"token":"secret invalid SAS","expiresAt":expires})),
            (200, serde_json::json!({"token":format!("SharedAccessSignature sr={}&sig=s&se={expires}&skn=p",azure_relay::token_provider::url_encode(audience)),"expiresAt":expires+1})),
            (403, serde_json::json!({"message":"secret forbidden"})),
            (200, sas_response(audience, expires)),
        ]).await;
    let provider = BrokerTokenProvider::new(
        Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
        &url::Url::parse("sb://ns.servicebus.windows.net").unwrap(),
        "relay",
        RelayRight::Send,
    )
    .unwrap();
    for _ in 0..7 {
        let error = provider
            .get_token(audience, Duration::from_secs(3600))
            .await
            .err()
            .unwrap();
        assert!(matches!(
            error,
            azure_relay::RelayError::AuthorizationFailed(_)
        ));
        assert!(!error.to_string().contains("secret"));
    }
    provider
        .get_token(audience, Duration::from_secs(3600))
        .await
        .unwrap();
    provider
        .get_token(audience, Duration::from_secs(3600))
        .await
        .unwrap();
    assert_eq!(broker.requests.lock().await.len(), 8);
}

#[tokio::test]
async fn enrollment_variants_config_status_logout_and_redirection() {
    use azbridge_broker_api::{EnrollmentPollResponse, EnrollmentStartRequest};
    let broker = MockBroker::start(vec![
        (200, serde_json::json!({"enrollmentId":"id","userCode":"ABCD","verificationUri":"https://github.com/login/device","interval":1,"expiresIn":600})),
        (200, serde_json::json!({"status":"pending","interval":1})),
        (200, serde_json::json!({"status":"slowDown","interval":5})),
        (200, serde_json::json!({"status":"authorized","credential":"secret","workspaceId":"w","gatewayId":"g","profileId":"p"})),
        (200, serde_json::json!({"revision":3,"relayEndpoint":"sb://ns.servicebus.windows.net/","config":{}})),
        (204, serde_json::json!(null)), (204, serde_json::json!(null)),
        (302, serde_json::json!({"secret":"not returned"})),
        (403, serde_json::json!({"message":"secret must not appear"})),
    ]).await;
    let unauthenticated = BrokerClient::new(&broker.origin, "").unwrap();
    let start = unauthenticated
        .begin_enrollment(&EnrollmentStartRequest {
            workspace: "w".into(),
            profile: "p".into(),
            gateway_name: "host".into(),
        })
        .await
        .unwrap();
    assert_eq!(start.enrollment_id, "id");
    assert!(matches!(
        unauthenticated.poll_enrollment("id").await.unwrap(),
        EnrollmentPollResponse::Pending { .. }
    ));
    assert!(matches!(
        unauthenticated.poll_enrollment("id").await.unwrap(),
        EnrollmentPollResponse::SlowDown { .. }
    ));
    assert!(matches!(
        unauthenticated.poll_enrollment("id").await.unwrap(),
        EnrollmentPollResponse::Authorized { .. }
    ));
    assert!(unauthenticated.poll_enrollment("../escaped").await.is_err());
    let client = BrokerClient::new(&broker.origin, "secret").unwrap();
    assert_eq!(client.config().await.unwrap().revision, 3);
    client.status(3).await.unwrap();
    client.logout().await.unwrap();
    assert!(
        client
            .config()
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("302")
    );
    assert!(
        !client
            .logout()
            .await
            .unwrap_err()
            .to_string()
            .contains("secret")
    );
    let requests = broker.requests.lock().await;
    assert!(requests[0].starts_with("POST /api/v1/enrollments "));
    assert!(!requests[0].contains("Bearer"));
    assert!(requests[3].starts_with("POST /api/v1/enrollments/id/poll "));
    assert!(requests[4].contains("Bearer secret"));
    assert!(requests[5].contains("\"appliedRevision\":3"));
    assert_eq!(requests.len(), 9);
}

async fn managed_echo_host(udp: bool) {
    use azbridge_lib::{
        broker::Bootstrap,
        config::{Config, LocalForward, RemoteForward},
    };
    use azure_relay::test_utils::MockRelayServer;
    use std::time::Duration;
    let relay = MockRelayServer::start().await;
    let audience = "http://127.0.0.1/managed-echo/";
    let broker = MockBroker::start(vec![
        (200, sas_response(audience, future_expiry(3600))),
        (200, sas_response(audience, future_expiry(3600))),
        (204, serde_json::json!(null)),
    ])
    .await;
    let (target, echo_task) = if udp {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut bytes = [0; 65536];
            loop {
                let (length, peer) = socket.recv_from(&mut bytes).await.unwrap();
                socket.send_to(&bytes[..length], peer).await.unwrap();
            }
        });
        (address, task)
    } else {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let (mut read, mut write) = stream.into_split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        (address, task)
    };
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = reservation.local_addr().unwrap();
    drop(reservation);
    let sign = if udp { -1 } else { 1 };
    let bootstrap = Bootstrap {
        config: Config {
            azure_relay_endpoint: Some(format!("ws://127.0.0.1:{}", relay.port)),
            connect_timeout: Some(5),
            keep_alive_interval: Some(30),
            local_forward: vec![LocalForward {
                relay_name: "managed-echo".into(),
                bind_port: Some(sign * i32::from(local.port())),
                port_name: Some("echo".into()),
                ..Default::default()
            }],
            remote_forward: vec![RemoteForward {
                relay_name: "managed-echo".into(),
                host: Some("127.0.0.1".into()),
                host_port: Some(sign * i32::from(target.port())),
                port_name: Some("echo".into()),
                ..Default::default()
            }],
            ..Default::default()
        },
        managed: Some((
            Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
            42,
        )),
    };
    let host = bootstrap.start().await.unwrap();
    if udp {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.send_to(b"managed datagram", local).await.unwrap();
        let mut bytes = [0; 64];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&bytes[..length], b"managed datagram");
    } else {
        let mut stream = tokio::net::TcpStream::connect(local).await.unwrap();
        stream.write_all(b"managed tunnel").await.unwrap();
        let mut bytes = [0; 14];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"managed tunnel");
    }
    let requests = broker.requests.lock().await;
    assert_eq!(
        requests.len(),
        3,
        "per-connection Send must use its cached scoped token"
    );
    assert!(requests[0].contains("\"right\":\"send\""));
    assert!(requests[1].contains("\"right\":\"listen\""));
    assert!(requests[2].starts_with("POST /api/v1/gateway/status "));
    assert!(requests[2].contains("\"appliedRevision\":42"));
    drop(requests);
    host.stop().await;
    echo_task.abort();
    relay.stop().await;
}

#[tokio::test]
async fn managed_host_tcp_echo_with_broker_tokens_and_applied_status() {
    managed_echo_host(false).await;
}

#[tokio::test]
async fn managed_host_udp_echo_with_broker_tokens_and_applied_status() {
    managed_echo_host(true).await;
}

#[tokio::test]
async fn managed_start_failure_never_reports_applied_revision() {
    use azbridge_lib::{
        broker::Bootstrap,
        config::{Config, LocalForward},
    };
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let broker = MockBroker::start(vec![(
        200,
        sas_response(
            "http://ns.servicebus.windows.net/relay/",
            future_expiry(3600),
        ),
    )])
    .await;
    let bootstrap = Bootstrap {
        config: Config {
            azure_relay_endpoint: Some("sb://ns.servicebus.windows.net".into()),
            local_forward: vec![LocalForward {
                relay_name: "relay".into(),
                bind_port: Some(i32::from(occupied.local_addr().unwrap().port())),
                ..Default::default()
            }],
            ..Default::default()
        },
        managed: Some((
            Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
            43,
        )),
    };
    assert!(bootstrap.start().await.is_err());
    let requests = broker.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].contains("/status"));
}

#[tokio::test]
async fn failed_status_report_stops_prebound_local_listener() {
    use azbridge_lib::{
        broker::Bootstrap,
        config::{Config, LocalForward},
    };
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let broker = MockBroker::start(vec![
        (
            200,
            sas_response(
                "http://ns.servicebus.windows.net/relay/",
                future_expiry(3600),
            ),
        ),
        (503, serde_json::json!({})),
    ])
    .await;
    let bootstrap = Bootstrap {
        config: Config {
            azure_relay_endpoint: Some("sb://ns.servicebus.windows.net".into()),
            local_forward: vec![LocalForward {
                relay_name: "relay".into(),
                bind_port: Some(i32::from(address.port())),
                ..Default::default()
            }],
            ..Default::default()
        },
        managed: Some((
            Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
            57,
        )),
    };
    assert!(bootstrap.start().await.is_err());
    assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    assert_eq!(broker.requests.lock().await.len(), 2);
}

#[tokio::test]
async fn managed_http_listener_uses_listen_and_rejects_revoked_tokens() {
    use azbridge_lib::{
        broker::Bootstrap,
        config::{Config, RemoteForward},
    };
    let relay = azure_relay::test_utils::MockRelayServer::start().await;
    let broker = MockBroker::start(vec![
        (
            200,
            sas_response("http://127.0.0.1/http-relay/", future_expiry(3600)),
        ),
        (204, serde_json::json!(null)),
        (403, serde_json::json!({"message":"revoked"})),
    ])
    .await;
    let bootstrap = Bootstrap {
        config: Config {
            azure_relay_endpoint: Some(format!("ws://127.0.0.1:{}", relay.port)),
            connect_timeout: Some(5),
            remote_forward: vec![RemoteForward {
                relay_name: "http-relay".into(),
                host: Some("127.0.0.1".into()),
                host_port: Some(8080),
                http: Some(true),
                port_name: Some("http".into()),
                ..Default::default()
            }],
            ..Default::default()
        },
        managed: Some((
            Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
            44,
        )),
    };
    let host = bootstrap.start().await.unwrap();
    host.stop().await;
    assert!(bootstrap.start().await.is_err());
    let requests = broker.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests[0].contains("\"right\":\"listen\""));
    assert!(!requests[2].contains("/status"));
    drop(requests);
    relay.stop().await;
}

async fn cli_enrollment_flow(machine: bool) {
    let directory = std::env::current_dir()
        .unwrap()
        .join(format!(".gateway-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let broker = MockBroker::start(vec![
        (200, serde_json::json!({"enrollmentId":"id","userCode":"ABCD","verificationUri":"https://github.com/login/device","interval":1,"expiresIn":600})),
        (200, serde_json::json!({"status":"pending","interval":1})),
        (200, serde_json::json!({"status":"slowDown","interval":2})),
        (200, serde_json::json!({"status":"authorized","credential":"never-print-this-credential","workspaceId":"w","gatewayId":"g","profileId":"p"})),
        (200, serde_json::json!({"revision":17,"relayEndpoint":"sb://ns.servicebus.windows.net/","config":{}})),
        (204, serde_json::json!(null)),
    ]).await;
    let command = |args: Vec<String>| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_azbridge"));
        command
            .args(args)
            .env("APPDATA", &directory)
            .env("HOME", &directory)
            .env("PROGRAMDATA", directory.join("machine"))
            .env_remove("AZBRIDGE_BROKER_URL")
            .env_remove("AZBRIDGE_GATEWAY_CREDENTIAL")
            .env_remove("AZURE_BRIDGE_CONNECTIONSTRING");
        if machine {
            command.arg("--machine");
        }
        command
    };
    let login = command(vec![
        "login".into(),
        "--broker".into(),
        broker.origin.clone(),
        "--workspace".into(),
        "workspace".into(),
        "--profile".into(),
        "profile".into(),
        "--name".into(),
        "test-host".into(),
    ])
    .output()
    .await
    .unwrap();
    assert!(
        login.status.success(),
        "{}",
        String::from_utf8_lossy(&login.stderr)
    );
    assert!(String::from_utf8_lossy(&login.stdout).contains("ABCD"));
    assert!(!String::from_utf8_lossy(&login.stdout).contains("never-print-this-credential"));
    #[cfg(windows)]
    let state = if machine {
        directory
            .join("machine")
            .join("Microsoft")
            .join("AzureBridge")
            .join("gateway.json")
    } else {
        directory.join("azbridge").join("gateway.json")
    };
    #[cfg(unix)]
    let state = directory.join(".azbridge").join("gateway.json");
    assert_eq!(read_state(&state).unwrap().unwrap().gateway_id, "g");
    let status = command(vec!["status".into()]).output().await.unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("revision 17"));
    let logout = command(vec!["logout".into()]).output().await.unwrap();
    assert!(
        logout.status.success(),
        "{}",
        String::from_utf8_lossy(&logout.stderr)
    );
    assert!(!state.exists());
    let requests = broker.requests.lock().await;
    assert!(requests[0].contains("\"gatewayName\":\"test-host\""));
    assert!(requests[0].contains("\"workspace\":\"workspace\""));
    assert!(requests[0].contains("\"profile\":\"profile\""));
    assert_eq!(requests.len(), 6);
    drop(requests);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn cli_login_device_flow_status_logout_end_to_end() {
    cli_enrollment_flow(false).await;
}

#[cfg(windows)]
#[tokio::test]
async fn cli_machine_login_status_logout_end_to_end() {
    cli_enrollment_flow(true).await;
}

#[test]
fn managed_host_rejects_credentials_invalid_endpoints_and_relay_scopes() {
    use azbridge_lib::{
        config::{Config, LocalForward},
        host::Host,
    };
    let client = Arc::new(BrokerClient::new("https://broker.example", "credential").unwrap());
    assert!(Host::with_broker(Config::default(), client.clone()).is_err());
    for endpoint in [
        "ws://remote.example",
        "http://remote.example",
        "sb://user:secret@ns",
        "sb://ns/path",
    ] {
        let config = Config {
            azure_relay_endpoint: Some(endpoint.into()),
            ..Default::default()
        };
        assert!(Host::with_broker(config, client.clone()).is_err());
    }
    for relay in ["", "a/../b", "bad?query"] {
        let config = Config {
            azure_relay_endpoint: Some("sb://ns".into()),
            local_forward: vec![LocalForward {
                relay_name: relay.into(),
                bind_port: Some(8080),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(Host::with_broker(config, client.clone()).is_err());
    }
    let config = Config {
        azure_relay_endpoint: Some("sb://ns".into()),
        azure_relay_shared_access_key: Some("namespace-key-must-not-be-used".into()),
        ..Default::default()
    };
    assert!(Host::with_broker(config, client).is_err());
}

#[tokio::test]
async fn managed_cli_process_forwards_tcp_through_mock_relay() {
    use std::time::Duration;
    let relay = azure_relay::test_utils::MockRelayServer::start().await;
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = echo.local_addr().unwrap();
    let echo_task = tokio::spawn(async move {
        let (stream, _) = echo.accept().await.unwrap();
        let (mut read, mut write) = stream.into_split();
        let _ = tokio::io::copy(&mut read, &mut write).await;
    });
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = reservation.local_addr().unwrap();
    drop(reservation);
    let broker = MockBroker::start(vec![
        (200, serde_json::json!({
            "revision":55,"relayEndpoint":format!("ws://127.0.0.1:{}",relay.port),
            "config":{
                "ConnectTimeout":5,
                "LocalForward":[{"RelayName":"cli-echo","BindPort":local.port(),"PortName":"echo"}],
                "RemoteForward":[{"RelayName":"cli-echo","Host":"127.0.0.1","HostPort":target.port(),"PortName":"echo"}]
            }
        })),
        (200, sas_response("http://127.0.0.1/cli-echo/", future_expiry(3600))),
        (200, sas_response("http://127.0.0.1/cli-echo/", future_expiry(3600))),
        (204, serde_json::json!(null)),
    ]).await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_azbridge"))
        .arg("-q")
        .env("AZBRIDGE_BROKER_URL", &broker.origin)
        .env("AZBRIDGE_GATEWAY_CREDENTIAL", "process-credential")
        .env(
            "AZURE_BRIDGE_CONNECTIONSTRING",
            "local-env-must-not-be-used",
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "CLI exited before ready"
            );
            if broker
                .requests
                .lock()
                .await
                .iter()
                .any(|r| r.starts_with("POST /api/v1/gateway/status "))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut stream = tokio::net::TcpStream::connect(local).await.unwrap();
    stream.write_all(b"actual managed CLI").await.unwrap();
    let mut bytes = [0; 18];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"actual managed CLI");
    drop(stream);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    echo_task.abort();
    relay.stop().await;
    let requests = broker.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert!(requests[3].contains("\"appliedRevision\":55"));
}

#[cfg(unix)]
#[tokio::test]
async fn managed_host_unix_socket_echo_uses_broker_auth() {
    use azbridge_lib::{
        broker::Bootstrap,
        config::{Config, LocalForward, RemoteForward},
    };
    use std::time::Duration;
    let directory = std::path::PathBuf::from(format!(".gateway-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let local = directory.join("local.sock");
    let target = directory.join("target.sock");
    let echo = tokio::net::UnixListener::bind(&target).unwrap();
    let echo_task = tokio::spawn(async move {
        let (stream, _) = echo.accept().await.unwrap();
        let (mut read, mut write) = stream.into_split();
        let _ = tokio::io::copy(&mut read, &mut write).await;
    });
    let relay = azure_relay::test_utils::MockRelayServer::start().await;
    let broker = MockBroker::start(vec![
        (
            200,
            sas_response("http://127.0.0.1/socket-relay/", future_expiry(3600)),
        ),
        (
            200,
            sas_response("http://127.0.0.1/socket-relay/", future_expiry(3600)),
        ),
        (204, serde_json::json!(null)),
    ])
    .await;
    let bootstrap = Bootstrap {
        config: Config {
            azure_relay_endpoint: Some(format!("ws://127.0.0.1:{}", relay.port)),
            connect_timeout: Some(5),
            local_forward: vec![LocalForward {
                relay_name: "socket-relay".into(),
                bind_local_socket: Some(local.to_string_lossy().into_owned()),
                port_name: Some("socket".into()),
                ..Default::default()
            }],
            remote_forward: vec![RemoteForward {
                relay_name: "socket-relay".into(),
                local_socket: Some(target.to_string_lossy().into_owned()),
                port_name: Some("socket".into()),
                ..Default::default()
            }],
            ..Default::default()
        },
        managed: Some((
            Arc::new(BrokerClient::new(&broker.origin, "credential").unwrap()),
            56,
        )),
    };
    let host = bootstrap.start().await.unwrap();
    let mut stream = tokio::net::UnixStream::connect(&local).await.unwrap();
    stream.write_all(b"socket tunnel").await.unwrap();
    let mut bytes = [0; 13];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"socket tunnel");
    drop(stream);
    host.stop().await;
    echo_task.abort();
    relay.stop().await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn managed_subcommands_and_existing_local_invocations() {
    for args in [
        vec![
            "azbridge",
            "login",
            "--broker",
            "https://broker.example",
            "--workspace",
            "ws",
            "--profile",
            "p",
            "--machine",
        ],
        vec!["azbridge", "logout", "--machine"],
        vec!["azbridge", "status", "--machine"],
        vec![
            "azbridge",
            "-L",
            "8080:r",
            "-e",
            "sb://ns.servicebus.windows.net",
            "-vvv",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_ok());
    }
    assert!(Cli::try_parse_from(["azbridge", "-L", "8080:r", "status"]).is_err());
    assert!(Cli::try_parse_from(["azbridge", "status", "-x", "secret"]).is_err());
}

#[test]
fn managed_rejects_local_and_cloud_overrides_but_accepts_logging() {
    for args in [
        vec!["-L", "8080:r"],
        vec!["-T", "r:80"],
        vec!["-R", "r:host:80"],
        vec!["-H", "r:http/localhost:80"],
        vec!["-e", "sb://ns"],
        vec!["-x", "secret"],
        vec!["-K", "key"],
        vec!["-k", "secret"],
        vec!["-s", "secret"],
        vec!["-f", "file.yml"],
        vec!["-o", "AzureRelaySharedAccessKey:secret"],
        vec!["-g"],
        vec!["-b", "127.0.0.1"],
        vec!["-a", "30"],
        vec!["--tui"],
    ] {
        let cli = Cli::try_parse_from(std::iter::once("azbridge").chain(args)).unwrap();
        assert!(validate_managed_cli(&cli).is_err());
    }
    let cli = Cli::try_parse_from(["azbridge", "-vvv", "-q", "-l", "local.log"]).unwrap();
    validate_managed_cli(&cli).unwrap();
}

#[test]
fn broker_origin_security() {
    for url in [
        "https://broker.example",
        "http://localhost:1234",
        "http://127.0.0.1:1234",
        "http://[::1]:1234",
    ] {
        assert!(BrokerClient::new(url, "secret").is_ok(), "{url}");
    }
    for url in [
        "http://broker.example",
        "https://user:secret@broker.example",
        "https://broker.example/path",
        "https://broker.example?token=x",
        "file:///a",
    ] {
        assert!(BrokerClient::new(url, "secret").is_err(), "{url}");
    }
}

#[test]
fn restricted_state_roundtrip_and_atomic_replacement() {
    let directory = std::env::current_dir()
        .unwrap()
        .join(format!(".gateway-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("gateway.json");
    assert!(read_state(&path).unwrap().is_none());
    let mut state = GatewayState {
        broker_url: "https://broker.example".into(),
        credential: "first".into(),
        workspace_id: "ws".into(),
        gateway_id: "gw".into(),
        profile: "p".into(),
    };
    write_state(&path, &state).unwrap();
    assert_eq!(read_state(&path).unwrap().unwrap().credential, "first");
    state.credential = "replacement".into();
    write_state(&path, &state).unwrap();
    assert_eq!(
        read_state(&path).unwrap().unwrap().credential,
        "replacement"
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[cfg(windows)]
    {
        let acl = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .unwrap();
        assert!(acl.status.success());
        let text = String::from_utf8_lossy(&acl.stdout);
        assert!(
            !text.contains("(I)"),
            "file ACL must not inherit directory permissions"
        );
        assert!(!text.contains("S-1-1-0"), "Everyone must not have access");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn managed_env_requires_pair_and_startup_does_not_merge_local_layers() {
    use azbridge_lib::broker::{bootstrap, discover_state};
    unsafe {
        std::env::remove_var("AZBRIDGE_GATEWAY_CREDENTIAL");
        std::env::set_var("AZBRIDGE_BROKER_URL", "http://127.0.0.1:1");
    }
    assert!(discover_state(false).is_err());
    let broker = MockBroker::start(vec![
        (
            200,
            serde_json::json!({
                "revision":9,"relayEndpoint":"sb://ns.servicebus.windows.net/",
                "config":{"LocalForward":[{"RelayName":"remote-profile","BindPort":8080}]}
            }),
        ),
        (503, serde_json::json!({})),
    ])
    .await;
    unsafe {
        std::env::set_var("AZBRIDGE_BROKER_URL", &broker.origin);
        std::env::set_var("AZBRIDGE_GATEWAY_CREDENTIAL", "credential");
        std::env::set_var("AZURE_BRIDGE_CONNECTIONSTRING", "must-not-be-used");
    }
    let cli = Cli::try_parse_from(["azbridge"]).unwrap();
    let result = bootstrap(&cli, false).await;
    let unreachable = bootstrap(&cli, false).await;
    unsafe {
        std::env::remove_var("AZBRIDGE_BROKER_URL");
        std::env::remove_var("AZBRIDGE_GATEWAY_CREDENTIAL");
        std::env::remove_var("AZURE_BRIDGE_CONNECTIONSTRING");
    }
    let bootstrap = result.unwrap();
    assert_eq!(bootstrap.managed.as_ref().unwrap().1, 9);
    assert_eq!(bootstrap.config.local_forward.len(), 1);
    assert_eq!(
        bootstrap.config.local_forward[0].relay_name,
        "remote-profile"
    );
    assert!(bootstrap.config.azure_relay_connection_string.is_none());
    assert!(unreachable.is_err());
}

#[cfg(unix)]
#[test]
fn state_rejects_symlinks_and_insecure_permissions() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let directory = std::env::current_dir()
        .unwrap()
        .join(format!(".gateway-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let target = directory.join("target");
    std::fs::write(&target, "unchanged").unwrap();
    let path = directory.join("gateway.json");
    symlink(&target, &path).unwrap();
    assert!(read_state(&path).is_err());
    let state = GatewayState {
        broker_url: "https://broker.example".into(),
        credential: "secret".into(),
        workspace_id: "w".into(),
        gateway_id: "g".into(),
        profile: "p".into(),
    };
    assert!(write_state(&path, &state).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
    std::fs::remove_file(&path).unwrap();
    write_state(&path, &state).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_state(&path).is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(windows)]
#[test]
#[serial_test::serial]
fn service_discovery_ignores_user_state_and_gateway_environment() {
    use azbridge_lib::broker::{discover_state, state_path};
    let directory = std::env::current_dir()
        .unwrap()
        .join(format!(".gateway-test-{}", uuid::Uuid::new_v4()));
    let names = [
        "PROGRAMDATA",
        "APPDATA",
        "AZBRIDGE_BROKER_URL",
        "AZBRIDGE_GATEWAY_CREDENTIAL",
    ];
    let saved: Vec<_> = names.iter().map(std::env::var_os).collect();
    unsafe {
        std::env::set_var("PROGRAMDATA", directory.join("machine"));
        std::env::set_var("APPDATA", directory.join("user"));
        std::env::set_var("AZBRIDGE_BROKER_URL", "invalid user URL must not be read");
        std::env::remove_var("AZBRIDGE_GATEWAY_CREDENTIAL");
    }
    let mut state = GatewayState {
        broker_url: "https://broker.example".into(),
        workspace_id: "workspace".into(),
        gateway_id: "machine".into(),
        profile: "profile".into(),
        credential: "machine-credential".into(),
    };
    write_state(&state_path(true).unwrap(), &state).unwrap();
    state.gateway_id = "user".into();
    write_state(&state_path(false).unwrap(), &state).unwrap();
    let result = discover_state(true);
    unsafe {
        for (name, value) in names.into_iter().zip(saved) {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
    assert_eq!(result.unwrap().unwrap().gateway_id, "machine");
}
