use azbridge_broker_api::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const ID: &str = "d8b4c010-0cbe-4ee5-805a-e0194c032a51";
const PROFILE_ID: &str = "e021d8e9-9c35-456b-bdb4-2d56d2f0fba9";

fn user() -> Value {
    json!({"id": 123456, "login": "octocat", "avatarUrl": "https://github.test/avatar"})
}

fn workspace() -> Value {
    json!({
        "id": ID, "name": "test", "namespaceName": "azb-test-123",
        "location": "westus", "state": "failed", "error": "Provisioning failed"
    })
}

fn gateway() -> Value {
    json!({
        "id": ID, "name": "gateway", "profileId": PROFILE_ID, "revoked": false,
        "lastSeenAt": 1791986400_u64, "appliedRevision": 3,
        "version": "0.1.0", "hostname": "gateway-host"
    })
}

fn assert_roundtrip<T: Serialize + DeserializeOwned>(fixture: Value) {
    let value: T = serde_json::from_value(fixture.clone()).unwrap();
    let encoded = serde_json::to_value(value).unwrap();
    assert_eq!(encoded, fixture);
    let decoded: T = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
}

macro_rules! roundtrip_test {
    ($name:ident, $ty:ty, $fixture:expr) => {
        #[test]
        fn $name() {
            assert_roundtrip::<$ty>($fixture);
        }
    };
}

roundtrip_test!(github_user_roundtrip, GitHubUser, user());
roundtrip_test!(workspace_roundtrip, Workspace, workspace());
roundtrip_test!(
    member_roundtrip,
    Member,
    json!({"user": user(), "role": "admin"})
);
roundtrip_test!(
    connection_roundtrip,
    Connection,
    json!({"id": ID, "name": "relay", "state": "failed", "error": "Failed"})
);
roundtrip_test!(
    profile_roundtrip,
    Profile,
    json!({"id": PROFILE_ID, "name": "default", "currentRevision": 3})
);
roundtrip_test!(
    revision_roundtrip,
    ConfigRevision,
    json!({"revision": 3, "createdAt": 1791986400_u64, "yaml": "LocalForward: []\n"})
);
roundtrip_test!(gateway_roundtrip, Gateway, gateway());
roundtrip_test!(
    relay_map_endpoint_roundtrip,
    RelayMapEndpoint,
    json!({
        "id": "local-0-0", "forwardGroup": "local-0", "relayName": "relay",
        "direction": "listener", "protocol": "http", "host": "localhost",
        "port": 8080, "socket": "test.sock", "portName": "web",
        "explicitPortName": true, "disabled": true, "path": "/api"
    })
);
roundtrip_test!(
    relay_map_profile_roundtrip,
    RelayMapProfile,
    json!({
        "profile": {"id": PROFILE_ID, "name": "client", "currentRevision": 1},
        "revision": 2, "endpoints": [], "error": "Invalid saved configuration"
    })
);
roundtrip_test!(
    relay_map_response_roundtrip,
    RelayMapResponse,
    json!({
        "profiles": [{"profile": {"id": PROFILE_ID, "name": "empty"}, "endpoints": []}],
        "connections": [{"id": ID, "name": "relay", "state": "ready"}]
    })
);
roundtrip_test!(
    audit_entry_roundtrip,
    AuditEntry,
    json!({"id": ID, "at": 1791986400_u64, "actor": "123456", "action": "publish", "target": PROFILE_ID})
);
roundtrip_test!(
    me_response_roundtrip,
    MeResponse,
    json!({"user": user(), "platformAdmin": true, "csrfToken": "test-csrf", "workspaces": [workspace()], "allowedLocations": ["uksouth", "westus"]})
);
roundtrip_test!(
    device_login_start_roundtrip,
    DeviceLoginStartResponse,
    json!({
        "userCode": "ABCD-EFGH", "verificationUri": "https://github.com/login/device",
        "interval": 5, "expiresIn": 900, "csrfToken": "test-only"
    })
);

#[test]
fn device_login_poll_roundtrip_all_variants() {
    for fixture in [
        json!({"status": "pending", "interval": 5}),
        json!({"status": "slowDown", "interval": 10}),
        json!({"status": "authorized"}),
        json!({"status": "expired"}),
        json!({"status": "denied"}),
    ] {
        assert_roundtrip::<DeviceLoginPollResponse>(fixture);
    }
}

#[test]
fn me_response_from_older_broker_has_no_selectable_locations() {
    let me: MeResponse = serde_json::from_value(json!({
        "user": user(), "platformAdmin": true, "csrfToken": "test-csrf", "workspaces": []
    }))
    .unwrap();
    assert!(me.allowed_locations.is_empty());
}
roundtrip_test!(
    create_workspace_roundtrip,
    CreateWorkspaceRequest,
    json!({"name": "test", "location": "westus"})
);
roundtrip_test!(
    create_member_roundtrip,
    CreateMemberRequest,
    json!({"login": "octocat", "role": "operator"})
);
roundtrip_test!(
    update_member_roundtrip,
    UpdateMemberRequest,
    json!({"role": "reader"})
);
roundtrip_test!(
    create_connection_roundtrip,
    CreateConnectionRequest,
    json!({"name": "relay"})
);
roundtrip_test!(
    create_profile_roundtrip,
    CreateProfileRequest,
    json!({"name": "default"})
);
roundtrip_test!(
    create_revision_roundtrip,
    CreateRevisionRequest,
    json!({"yaml": "LocalForward: []\n"})
);
roundtrip_test!(
    publish_revision_roundtrip,
    PublishRevisionRequest,
    json!({"revision": 3, "expectedCurrentRevision": 2})
);
roundtrip_test!(
    create_gateway_roundtrip,
    CreateGatewayRequest,
    json!({"name": "gateway", "profileId": PROFILE_ID})
);
roundtrip_test!(
    create_gateway_response_roundtrip,
    CreateGatewayResponse,
    json!({"gateway": gateway(), "credential": "azgw.test.only", "workspaceId": ID})
);
roundtrip_test!(
    enrollment_start_request_roundtrip,
    EnrollmentStartRequest,
    json!({"workspace": "test", "profile": "default", "gatewayName": "gateway"})
);
roundtrip_test!(
    enrollment_start_response_roundtrip,
    EnrollmentStartResponse,
    json!({
        "enrollmentId": ID, "userCode": "ABCD-EFGH", "verificationUri": "https://github.test/device",
        "interval": 5, "expiresIn": 900
    })
);
roundtrip_test!(
    relay_token_request_roundtrip,
    RelayTokenRequest,
    json!({"relayName": "relay", "right": "send"})
);
roundtrip_test!(
    relay_token_response_roundtrip,
    RelayTokenResponse,
    json!({"token": "SharedAccessSignature test-only", "expiresAt": 1791986400_u64})
);
roundtrip_test!(
    gateway_status_request_roundtrip,
    GatewayStatusRequest,
    json!({"appliedRevision": 3, "version": "0.1.0", "hostname": "gateway-host"})
);
roundtrip_test!(
    error_response_roundtrip,
    ErrorResponse,
    json!({"code": "invalid_config", "message": "Unknown relay"})
);
roundtrip_test!(
    validation_response_roundtrip,
    ValidationResponse,
    json!({"valid": false, "errors": ["Unknown relay"]})
);
roundtrip_test!(
    health_response_roundtrip,
    HealthResponse,
    json!({"status": "ok", "mode": "production"})
);

#[test]
fn enums_roundtrip_all_variants() {
    for value in ["listener", "destination"] {
        assert_roundtrip::<RelayEndpointDirection>(json!(value));
    }
    for value in ["tcp", "udp", "socket", "http"] {
        assert_roundtrip::<RelayEndpointProtocol>(json!(value));
    }
    for value in ["reader", "operator", "admin"] {
        assert_roundtrip::<Role>(json!(value));
    }
    for value in ["pending", "provisioning", "ready", "failed", "deleting"] {
        assert_roundtrip::<ProvisioningState>(json!(value));
    }
    for value in ["send", "listen"] {
        assert_roundtrip::<RelayRight>(json!(value));
    }
    for value in ["development", "production"] {
        assert_roundtrip::<BrokerMode>(json!(value));
    }
    for fixture in [
        json!({"status": "pending", "interval": 5}),
        json!({"status": "slowDown", "interval": 10}),
        json!({
            "status": "authorized", "credential": "azgw.test.only",
            "workspaceId": ID, "gatewayId": ID, "profileId": PROFILE_ID
        }),
        json!({"status": "expired"}),
        json!({"status": "denied"}),
    ] {
        assert_roundtrip::<EnrollmentPollResponse>(fixture);
    }
}

#[test]
fn role_permissions_cover_every_pair() {
    for (actual_index, actual) in [Role::Reader, Role::Operator, Role::Admin]
        .into_iter()
        .enumerate()
    {
        for (required_index, required) in [Role::Reader, Role::Operator, Role::Admin]
            .into_iter()
            .enumerate()
        {
            assert_eq!(actual.permits(required), actual_index >= required_index);
        }
    }
}

#[test]
fn optional_fields_may_be_absent_and_are_omitted_when_none() {
    assert_roundtrip::<RelayMapEndpoint>(json!({
        "id": "remote-0-0", "forwardGroup": "remote-0", "relayName": "relay",
        "direction": "destination", "protocol": "tcp", "port": 80,
        "portName": "80", "explicitPortName": false, "disabled": false
    }));
    assert_roundtrip::<Workspace>(json!({
        "id": ID, "name": "test", "namespaceName": "azb-test-123",
        "location": "westus", "state": "ready"
    }));
    assert_roundtrip::<Connection>(json!({"id": ID, "name": "relay", "state": "ready"}));
    assert_roundtrip::<Profile>(json!({"id": PROFILE_ID, "name": "default"}));
    assert_roundtrip::<Gateway>(json!({
        "id": ID, "name": "gateway", "profileId": PROFILE_ID, "revoked": true
    }));
    assert_roundtrip::<PublishRevisionRequest>(json!({"revision": 3}));
    let profile: Profile = serde_json::from_value(json!({
        "id": PROFILE_ID, "name": "default", "currentRevision": null
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(profile).unwrap(),
        json!({"id": PROFILE_ID, "name": "default"})
    );
}

#[test]
fn gateway_config_roundtrip_keeps_nested_pascal_case_fields() {
    let config = azbridge_config::Config {
        local_forward: vec![azbridge_config::LocalForward {
            relay_name: "relay".into(),
            bindings: vec![azbridge_config::LocalForwardBinding {
                bind_address: Some("127.0.0.1".into()),
                bind_port: 8080,
                port_name: Some("web".into()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        remote_forward: vec![azbridge_config::RemoteForward {
            relay_name: "relay".into(),
            bindings: vec![azbridge_config::RemoteForwardBinding {
                host: Some("localhost".into()),
                host_port: 80,
                http: true,
                path: Some("/".into()),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let fixture = json!({
        "revision": 3, "relayEndpoint": "sb://azb-test.servicebus.windows.net/",
        "config": config
    });
    assert_roundtrip::<GatewayConfigResponse>(fixture.clone());
    let embedded = &fixture["config"];
    assert!(embedded.get("LocalForward").is_some());
    assert!(embedded.get("localForward").is_none());
    assert_eq!(embedded["LocalForward"][0]["Bindings"][0]["BindPort"], 8080);
    assert!(embedded["LocalForward"][0].get("BindPort").is_none());
    assert!(embedded["RemoteForward"][0].get("Host").is_none());
    assert_roundtrip::<azbridge_config::Config>(embedded.clone());
    assert_roundtrip::<azbridge_config::LocalForward>(embedded["LocalForward"][0].clone());
    assert_roundtrip::<azbridge_config::LocalForwardBinding>(
        embedded["LocalForward"][0]["Bindings"][0].clone(),
    );
    assert_roundtrip::<azbridge_config::RemoteForward>(embedded["RemoteForward"][0].clone());
    assert_roundtrip::<azbridge_config::RemoteForwardBinding>(
        embedded["RemoteForward"][0]["Bindings"][0].clone(),
    );
}

#[test]
fn rejects_invalid_enums_poll_payloads_and_negative_unsigned_values() {
    assert!(serde_json::from_value::<Role>(json!("owner")).is_err());
    assert!(serde_json::from_value::<ProvisioningState>(json!("unknown")).is_err());
    assert!(serde_json::from_value::<RelayRight>(json!("manage")).is_err());
    for fixture in [
        json!({"status": "unknown"}),
        json!({"status": "pending"}),
        json!({"status": "authorized", "workspaceId": ID}),
        json!({}),
    ] {
        assert!(serde_json::from_value::<EnrollmentPollResponse>(fixture).is_err());
    }
    assert!(
        serde_json::from_value::<GitHubUser>(
            json!({"id": -1, "login": "octocat", "avatarUrl": ""})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<PublishRevisionRequest>(json!({"revision": -1})).is_err());
    assert!(
        serde_json::from_value::<RelayTokenResponse>(json!({
            "token": "test", "expiresAt": -1
        }))
        .is_err()
    );
}

#[test]
fn bindings_are_current_without_writing_files() {
    let generated = typescript_bindings();
    assert_eq!(
        generated,
        include_str!("../../../web/src/api/generated/index.ts"),
        "run cargo run -p azbridge-broker-api --example generate_types"
    );
    assert!(!generated.contains("any"));
    assert!(!generated.contains("bigint"));
    assert!(generated.contains("config: Config"));
    assert!(generated.contains("BindPort?: number | null"));
    assert!(generated.contains("Host?: string | null"));
}
