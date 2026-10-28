use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use azbridge_broker::{
    error::BrokerError,
    relay::{ArmRelayManager, FakeRelayManager, RelayManager},
    settings::CloudSettings,
};
use azbridge_broker_api::{ProvisioningState as State, RelayRight, RelayTokenResponse};
use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

const SUB: &str = "00000000-0000-0000-0000-000000000001";
const NS: &str = "azb-test";
const HC: &str = "MyConnection";
const KEY: &str = "c2lnbmluZy1rZXktbm90LWJhc2U2NC1kZWNvZGVk";

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
        assert_eq!(scopes, ["https://management.azure.com/.default"]);
        if self.fail {
            return Err(azure_core::Error::with_message(
                azure_core::error::ErrorKind::Credential,
                "never-disclose-credential",
            ));
        }
        Ok(AccessToken::new(
            "arm-test-token",
            azure_core::time::OffsetDateTime::now_utc() + azure_core::time::Duration::hours(1),
        ))
    }
}

fn settings() -> CloudSettings {
    CloudSettings {
        github_client_id: String::new(),
        github_client_secret: String::new(),
        cookie_key: [0; 32],
        bootstrap_admin_ids: BTreeSet::new(),
        storage_account: "test".into(),
        subscription_id: SUB.into(),
        relay_resource_group: "relay-rg".into(),
        allowed_locations: BTreeSet::from(["westus".into()]),
        azure_client_id: None,
    }
}

fn manager(server: &MockServer) -> ArmRelayManager {
    ArmRelayManager::with_credential(
        &settings(),
        Arc::new(Credential { fail: false }),
        Url::parse(&server.uri()).unwrap(),
    )
    .unwrap()
}

fn ns_path() -> String {
    format!(
        "/subscriptions/{SUB}/resourceGroups/relay-rg/providers/Microsoft.Relay/namespaces/{NS}"
    )
}

fn hc_path() -> String {
    format!("{}/hybridConnections/{HC}", ns_path())
}

fn rule_path(right: RelayRight) -> String {
    format!(
        "{}/authorizationRules/azbridge-{}",
        hc_path(),
        if right == RelayRight::Send {
            "send"
        } else {
            "listen"
        }
    )
}

async fn response(server: &MockServer, verb: &str, resource: &str, status: u16, body: Value) {
    Mock::given(method(verb))
        .and(path(resource))
        .and(query_param("api-version", "2026-01-01"))
        .and(header("authorization", "Bearer arm-test-token"))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

async fn ready(server: &MockServer) {
    response(
        server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    response(
        server,
        "GET",
        &hc_path(),
        200,
        json!({"properties": {"requiresClientAuthorization": true}}),
    )
    .await;
    for (right, arm_right) in [(RelayRight::Send, "Send"), (RelayRight::Listen, "Listen")] {
        Mock::given(method("GET"))
            .and(path(rule_path(right)))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"properties": {"rights": [arm_right]}})),
            )
            .with_priority(10)
            .mount(server)
            .await;
    }
}

fn assert_token(token: &RelayTokenResponse, key: &str, right: RelayRight) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(token.expires_at > now + 3500 && token.expires_at <= now + 3600);
    let audience = "http%3A%2F%2Fazb-test.servicebus.windows.net%2Fmyconnection%2F";
    let signed = format!("{audience}\n{}", token.expires_at);
    // .NET signs UTF-8 key text, not its base64-decoded bytes.
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(signed.as_bytes());
    let signature =
        azure_relay::token_provider::url_encode(&STANDARD.encode(mac.finalize().into_bytes()));
    let rule = if right == RelayRight::Send {
        "azbridge-send"
    } else {
        "azbridge-listen"
    };
    assert_eq!(
        token.token,
        format!(
            "SharedAccessSignature sr={audience}&sig={signature}&se={}&skn={rule}",
            token.expires_at
        )
    );
    assert!(!token.token.contains(key));
    assert!(!token.token.contains("arm-test-token"));
}

#[tokio::test]
async fn namespace_create_lro_reconcile_and_delete_are_not_falsely_ready() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    response(&server, "GET", &ns_path(), 404, json!({})).await;
    Mock::given(method("PUT"))
        .and(path(ns_path()))
        .and(body_json(json!({
            "location": "westus",
            "sku": {"name": "Standard", "tier": "Standard"},
            "properties": {"minimumTlsVersion": "1.2"}
        })))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header(
                    "operation-location",
                    format!("{}/subscriptions/{SUB}/operations/one", server.uri()),
                )
                .insert_header("retry-after", "120")
                .set_body_json(json!({"properties": {"provisioningState": "Succeeded"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        manager.ensure_namespace(NS, "westus").await.unwrap(),
        State::Provisioning
    );
    server.verify().await;
    server.reset().await;
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Creating"}}),
    )
    .await;
    assert_eq!(
        manager.ensure_namespace(NS, "westus").await.unwrap(),
        State::Provisioning
    );
    server.reset().await;
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    assert_eq!(
        manager.ensure_namespace(NS, "westus").await.unwrap(),
        State::Ready
    );
    response(&server, "DELETE", &ns_path(), 202, json!({})).await;
    assert!(!manager.delete_namespace(NS).await.unwrap());
    server.reset().await;
    response(&server, "DELETE", &ns_path(), 404, json!({})).await;
    assert!(manager.delete_namespace(NS).await.unwrap());
    // No request was ever made to the operation URL.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn namespace_statuses_and_synchronous_create() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for (body, expected) in [
        (json!({}), State::Provisioning),
        (
            json!({"properties": {"provisioningState": "Accepted"}}),
            State::Provisioning,
        ),
        (
            json!({"properties": {"provisioningState": "Failed"}}),
            State::Failed,
        ),
        (
            json!({"properties": {"provisioningState": "Canceled"}}),
            State::Failed,
        ),
        (
            json!({"properties": {"provisioningState": "Deleting"}}),
            State::Deleting,
        ),
        (
            json!({"properties": {"provisioningState": "unknown"}}),
            State::Provisioning,
        ),
    ] {
        server.reset().await;
        response(&server, "GET", &ns_path(), 200, body).await;
        assert_eq!(
            manager.ensure_namespace(NS, "westus").await.unwrap(),
            expected
        );
    }
    server.reset().await;
    response(&server, "GET", &ns_path(), 404, json!({})).await;
    response(
        &server,
        "PUT",
        &ns_path(),
        201,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    assert_eq!(
        manager.ensure_namespace(NS, "westus").await.unwrap(),
        State::Ready
    );
}

#[tokio::test]
async fn hybrid_connection_crud_requires_exact_send_listen_rules() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    response(&server, "GET", &hc_path(), 404, json!({})).await;
    Mock::given(method("PUT"))
        .and(path(hc_path()))
        .and(body_json(
            json!({"properties": {"requiresClientAuthorization": true}}),
        ))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"properties": {"requiresClientAuthorization": true}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    for (right, arm_right) in [(RelayRight::Send, "Send"), (RelayRight::Listen, "Listen")] {
        response(&server, "GET", &rule_path(right), 404, json!({})).await;
        Mock::given(method("PUT"))
            .and(path(rule_path(right)))
            .and(body_json(json!({"properties": {"rights": [arm_right]}})))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({"properties": {"rights": [arm_right]}})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Ready
    );
    server.verify().await;
    server.reset().await;
    ready(&server).await;
    for (right, arm_right) in [(RelayRight::Send, "Send"), (RelayRight::Listen, "Listen")] {
        response(
            &server,
            "GET",
            &rule_path(right),
            200,
            json!({"properties": {"rights": [arm_right]}}),
        )
        .await;
    }
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Ready
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
    response(&server, "DELETE", &hc_path(), 200, json!({})).await;
    assert!(manager.delete_connection(NS, HC).await.unwrap());
    server.reset().await;
    response(&server, "DELETE", &hc_path(), 202, json!({})).await;
    assert!(!manager.delete_connection(NS, HC).await.unwrap());
    server.reset().await;
    response(&server, "DELETE", &hc_path(), 404, json!({})).await;
    assert!(manager.delete_connection(NS, HC).await.unwrap());
}

#[tokio::test]
async fn token_scope_signature_cache_and_primary_rotation_for_both_rights() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    ready(&server).await;
    for right in [RelayRight::Send, RelayRight::Listen] {
        Mock::given(method("POST"))
            .and(path(format!("{}/listKeys", rule_path(right))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "primaryKey": KEY, "secondaryKey": "never-return-secondary",
                "primaryConnectionString": "never-return-connection-string"
            })))
            .expect(1)
            .mount(&server)
            .await;
        for _ in 0..2 {
            let token = manager.issue_token(NS, HC, right).await.unwrap();
            assert_token(&token, KEY, right);
        }
    }
    for right in [RelayRight::Send, RelayRight::Listen] {
        Mock::given(method("POST"))
            .and(path(format!("{}/regenerateKeys", rule_path(right))))
            .and(body_json(json!({"keyType": "PrimaryKey"})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"primaryKey": "rotated"})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    manager.rotate_keys(NS, HC).await.unwrap();
    server.verify().await;
    server.reset().await;
    ready(&server).await;
    for right in [RelayRight::Send, RelayRight::Listen] {
        response(
            &server,
            "POST",
            &format!("{}/listKeys", rule_path(right)),
            200,
            json!({"primaryKey": "rotated"}),
        )
        .await;
        assert_token(
            &manager.issue_token(NS, HC, right).await.unwrap(),
            "rotated",
            right,
        );
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| { r.url.path().starts_with(&hc_path()) || r.url.path() == ns_path() })
    );
}

#[tokio::test]
async fn incomplete_resources_or_rules_never_become_ready_or_issue_tokens() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Creating"}}),
    )
    .await;
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Provisioning
    );
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::Conflict)
    ));
    server.reset().await;
    ready(&server).await;
    response(
        &server,
        "GET",
        &rule_path(RelayRight::Send),
        200,
        json!({"properties": {"provisioningState": "Failed"}}),
    )
    .await;
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Failed
    );
    server.reset().await;
    ready(&server).await;
    response(
        &server,
        "GET",
        &rule_path(RelayRight::Send),
        202,
        json!({"properties": {"rights": ["Send"]}}),
    )
    .await;
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Provisioning
    );
    server.reset().await;
    response(&server, "GET", &ns_path(), 404, json!({})).await;
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::NotFound)
    ));
}

#[tokio::test]
async fn arm_errors_are_sanitized_and_never_fall_back_to_fake() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for (status, expected) in [
        (400, "service unavailable"),
        (401, "service unavailable"),
        (403, "service unavailable"),
        (409, "resource conflict"),
        (429, "too many requests"),
        (500, "service unavailable"),
        (302, "service unavailable"),
    ] {
        server.reset().await;
        response(
            &server,
            "GET",
            &ns_path(),
            status,
            json!({"error": "never-disclose"}),
        )
        .await;
        let error = manager.ensure_namespace(NS, "westus").await.unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(!format!("{error:?}").contains("never-disclose"));
    }
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("never-disclose"))
        .mount(&server)
        .await;
    assert!(matches!(
        manager.ensure_namespace(NS, "westus").await,
        Err(BrokerError::Unavailable)
    ));
    let failing = ArmRelayManager::with_credential(
        &settings(),
        Arc::new(Credential { fail: true }),
        Url::parse(&server.uri()).unwrap(),
    )
    .unwrap();
    let error = failing.ensure_namespace(NS, "westus").await.unwrap_err();
    assert_eq!(error.to_string(), "service unavailable");
    assert!(!format!("{error:?}").contains("never-disclose"));
}

#[tokio::test]
async fn operation_headers_cannot_redirect_credentials() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for location in [
        "https://evil.example/subscriptions/x/operations/one".to_owned(),
        format!(
            "http://user:secret@{}/subscriptions/{SUB}/operations/one",
            Url::parse(&server.uri()).unwrap().authority()
        ),
        format!("{}/subscriptions/another/operations/one", server.uri()),
        format!("{}/subscriptions/{SUB}/%2f%2fevil", server.uri()),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(202).insert_header("operation-location", location))
            .mount(&server)
            .await;
        assert!(matches!(
            manager.ensure_namespace(NS, "westus").await,
            Err(BrokerError::Unavailable)
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn all_names_and_cloud_scope_are_validated_before_network() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for name in [
        "",
        "short",
        "../evil",
        "azb/test",
        "azb%2ftest",
        "azb\\test",
        "azb-test?",
        "-start",
        "azb-test-",
    ] {
        assert!(matches!(
            manager.ensure_namespace(name, "westus").await,
            Err(BrokerError::BadRequest)
        ));
        assert!(matches!(
            manager.delete_namespace(name).await,
            Err(BrokerError::BadRequest)
        ));
    }
    for name in [
        "",
        "..",
        "../evil",
        "/resource/id",
        "hc%2Fname",
        "a?b",
        "a#b",
        "a\\b",
        "a/b",
    ] {
        assert!(matches!(
            manager.ensure_connection(NS, name).await,
            Err(BrokerError::BadRequest)
        ));
        assert!(matches!(
            manager.delete_connection(NS, name).await,
            Err(BrokerError::BadRequest)
        ));
        assert!(matches!(
            manager.rotate_keys(NS, name).await,
            Err(BrokerError::BadRequest)
        ));
        assert!(matches!(
            manager.issue_token(NS, name, RelayRight::Send).await,
            Err(BrokerError::BadRequest)
        ));
    }
    assert!(matches!(
        manager.ensure_namespace(NS, "eastus").await,
        Err(BrokerError::BadRequest)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    for endpoint in [
        "http://management.azure.com/",
        "https://evil.example/",
        "https://management.azure.com/other",
        "https://management.azure.com/?secret=1",
    ] {
        assert!(matches!(
            ArmRelayManager::with_credential(
                &settings(),
                Arc::new(Credential { fail: false }),
                Url::parse(endpoint).unwrap()
            ),
            Err(BrokerError::BadRequest)
        ));
    }
    let mut invalid = settings();
    invalid.subscription_id = "../another".into();
    assert!(matches!(
        ArmRelayManager::with_credential(
            &invalid,
            Arc::new(Credential { fail: false }),
            Url::parse(&server.uri()).unwrap()
        ),
        Err(BrokerError::BadRequest)
    ));
    invalid = settings();
    invalid.relay_resource_group = "../other".into();
    assert!(matches!(
        ArmRelayManager::with_credential(
            &invalid,
            Arc::new(Credential { fail: false }),
            Url::parse(&server.uri()).unwrap()
        ),
        Err(BrokerError::BadRequest)
    ));
}

#[tokio::test]
async fn fake_is_explicit_object_safe_stateful_and_signs_real_test_sas() {
    let manager: Arc<dyn RelayManager> = Arc::new(FakeRelayManager::new());
    assert!(matches!(
        manager.ensure_connection(NS, HC).await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        manager.rotate_keys(NS, HC).await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        manager.ensure_namespace(NS, "../location").await,
        Err(BrokerError::BadRequest)
    ));
    assert_eq!(
        manager.ensure_namespace(NS, "westus").await.unwrap(),
        State::Ready
    );
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Ready
    );
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Ready
    );
    let mut before = HashMap::new();
    for (right, key) in [
        (RelayRight::Send, "ZGV2ZWxvcG1lbnQtb25seS1zZW5kLWtleQ=="),
        (RelayRight::Listen, "ZGV2ZWxvcG1lbnQtb25seS1saXN0ZW4ta2V5"),
    ] {
        let token = manager.issue_token(NS, HC, right).await.unwrap();
        assert_token(&token, key, right);
        before.insert(right, token.token);
    }
    manager.rotate_keys(NS, HC).await.unwrap();
    for right in [RelayRight::Send, RelayRight::Listen] {
        assert_ne!(
            before[&right],
            manager.issue_token(NS, HC, right).await.unwrap().token
        );
    }
    assert!(manager.delete_connection(NS, HC).await.unwrap());
    assert!(manager.delete_connection(NS, HC).await.unwrap());
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Listen).await,
        Err(BrokerError::NotFound)
    ));
    manager.ensure_connection(NS, HC).await.unwrap();
    assert!(manager.delete_namespace(NS).await.unwrap());
    assert!(manager.delete_namespace(NS).await.unwrap());
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::NotFound)
    ));
    let _default = FakeRelayManager::default();
}

#[test]
fn production_constructor_uses_managed_identity_without_network() {
    // Credential construction can reject unsupported managed-identity hosts, but
    // must never perform ARM traffic or substitute a development manager.
    match ArmRelayManager::from_cloud(&settings()) {
        Ok(_) => {}
        Err(error) => assert!(matches!(error, BrokerError::Unavailable)),
    }
}

#[tokio::test]
async fn missing_or_invalid_primary_keys_and_rotation_failures_are_safe_errors() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for body in [
        json!({}),
        json!({"primaryKey": ""}),
        json!({"primaryKey": "x".repeat(257)}),
        json!({"secondaryKey": "never-use-secondary"}),
    ] {
        server.reset().await;
        ready(&server).await;
        response(
            &server,
            "POST",
            &format!("{}/listKeys", rule_path(RelayRight::Send)),
            200,
            body.clone(),
        )
        .await;
        assert!(matches!(
            manager.issue_token(NS, HC, RelayRight::Send).await,
            Err(BrokerError::Unavailable)
        ));
        response(
            &server,
            "POST",
            &format!("{}/regenerateKeys", rule_path(RelayRight::Send)),
            200,
            body,
        )
        .await;
        assert!(matches!(
            manager.rotate_keys(NS, HC).await,
            Err(BrokerError::Unavailable)
        ));
    }
    for (action, status) in [
        ("listKeys", 404),
        ("listKeys", 202),
        ("regenerateKeys", 404),
        ("regenerateKeys", 202),
    ] {
        server.reset().await;
        ready(&server).await;
        response(
            &server,
            "POST",
            &format!("{}/{action}", rule_path(RelayRight::Send)),
            status,
            json!({"primaryKey": "never-disclose-key"}),
        )
        .await;
        let error = if action == "listKeys" {
            match manager.issue_token(NS, HC, RelayRight::Send).await {
                Ok(_) => panic!("unexpected token"),
                Err(error) => error,
            }
        } else {
            manager.rotate_keys(NS, HC).await.unwrap_err()
        };
        assert_eq!(
            error.to_string(),
            if status == 404 {
                "resource not found"
            } else {
                "service unavailable"
            }
        );
        assert!(!format!("{error:?}").contains("never-disclose"));
    }
}

#[tokio::test]
async fn failed_partial_rotation_invalidates_both_cached_rights() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    ready(&server).await;
    for right in [RelayRight::Send, RelayRight::Listen] {
        response(
            &server,
            "POST",
            &format!("{}/listKeys", rule_path(right)),
            200,
            json!({"primaryKey": KEY}),
        )
        .await;
        manager.issue_token(NS, HC, right).await.unwrap();
    }
    response(
        &server,
        "POST",
        &format!("{}/regenerateKeys", rule_path(RelayRight::Send)),
        200,
        json!({"primaryKey": "rotated"}),
    )
    .await;
    response(
        &server,
        "POST",
        &format!("{}/regenerateKeys", rule_path(RelayRight::Listen)),
        500,
        json!({"error": "never-disclose"}),
    )
    .await;
    assert!(matches!(
        manager.rotate_keys(NS, HC).await,
        Err(BrokerError::Unavailable)
    ));
    server.reset().await;
    ready(&server).await;
    for right in [RelayRight::Send, RelayRight::Listen] {
        Mock::given(method("POST"))
            .and(path(format!("{}/listKeys", rule_path(right))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"primaryKey": "fresh"})))
            .expect(1)
            .mount(&server)
            .await;
        assert_token(
            &manager.issue_token(NS, HC, right).await.unwrap(),
            "fresh",
            right,
        );
    }
}

#[tokio::test]
async fn deletes_and_missing_resources_preserve_failure_states() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    for (state, result) in [
        ("Failed", None),
        ("Deleting", Some(false)),
        ("Accepted", Some(false)),
        ("Pending", Some(false)),
        ("unknown", Some(false)),
        ("Succeeded", Some(true)),
    ] {
        server.reset().await;
        response(
            &server,
            "DELETE",
            &ns_path(),
            200,
            json!({"properties": {"provisioningState": state}}),
        )
        .await;
        let actual = manager.delete_namespace(NS).await;
        match result {
            Some(expected) => assert_eq!(actual.unwrap(), expected),
            None => assert!(matches!(actual, Err(BrokerError::Unavailable))),
        }
    }
    server.reset().await;
    response(&server, "GET", &ns_path(), 404, json!({})).await;
    response(&server, "PUT", &ns_path(), 404, json!({})).await;
    assert!(matches!(
        manager.ensure_namespace(NS, "westus").await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        manager.ensure_connection(NS, HC).await,
        Err(BrokerError::NotFound)
    ));
    server.reset().await;
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    response(&server, "GET", &hc_path(), 404, json!({})).await;
    response(&server, "PUT", &hc_path(), 404, json!({})).await;
    assert!(matches!(
        manager.ensure_connection(NS, HC).await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::NotFound)
    ));
}

#[tokio::test]
async fn disabled_client_auth_and_manage_rules_are_repaired_without_issuing_tokens() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    response(
        &server,
        "GET",
        &ns_path(),
        200,
        json!({"properties": {"provisioningState": "Succeeded"}}),
    )
    .await;
    response(
        &server,
        "GET",
        &hc_path(),
        200,
        json!({"properties": {"requiresClientAuthorization": false}}),
    )
    .await;
    assert!(matches!(
        manager.issue_token(NS, HC, RelayRight::Send).await,
        Err(BrokerError::Conflict)
    ));
    response(
        &server,
        "PUT",
        &hc_path(),
        200,
        json!({"properties": {"requiresClientAuthorization": true}}),
    )
    .await;
    for (right, arm_right) in [(RelayRight::Send, "Send"), (RelayRight::Listen, "Listen")] {
        response(
            &server,
            "GET",
            &rule_path(right),
            200,
            json!({"properties": {"rights": ["Manage", "Send", "Listen"]}}),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path(rule_path(right)))
            .and(body_json(json!({"properties": {"rights": [arm_right]}})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"properties": {"rights": [arm_right]}})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    assert_eq!(
        manager.ensure_connection(NS, HC).await.unwrap(),
        State::Ready
    );
}

#[tokio::test]
async fn cached_keys_cannot_issue_tokens_after_rights_change_or_rule_disappears() {
    let server = MockServer::start().await;
    let manager = manager(&server);
    ready(&server).await;
    response(
        &server,
        "POST",
        &format!("{}/listKeys", rule_path(RelayRight::Send)),
        200,
        json!({"primaryKey": KEY}),
    )
    .await;
    manager.issue_token(NS, HC, RelayRight::Send).await.unwrap();
    for (status, rights) in [
        (200, json!(["Manage", "Send", "Listen"])),
        (200, json!(["Listen"])),
        (202, json!(["Send"])),
        (404, json!(null)),
    ] {
        server.reset().await;
        ready(&server).await;
        response(
            &server,
            "GET",
            &rule_path(RelayRight::Send),
            status,
            json!({"properties": {"rights": rights}}),
        )
        .await;
        let error = match manager.issue_token(NS, HC, RelayRight::Send).await {
            Ok(_) => panic!("unexpected token"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            if status == 404 {
                "resource not found"
            } else {
                "resource conflict"
            }
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|r| r.method == "GET")
        );
    }
}
