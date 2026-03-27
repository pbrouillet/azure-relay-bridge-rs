use std::collections::HashMap;

use azure_relay::protocol::*;
use url::Url;

// ── AcceptCommand ───────────────────────────────────────────────────────

#[test]
fn accept_command_round_trip() {
    let cmd = AcceptCommand {
        address: "wss://dc-node.servicebus.windows.net:443/$hc/test?sb-hc-action=accept&sb-hc-id=abc".into(),
        id: "4cb542c3-047a-4d40-a19f-bdc66441e736".into(),
        connect_headers: HashMap::from([
            ("Host".into(), "contoso.servicebus.windows.net".into()),
            ("Sec-WebSocket-Protocol".into(), "wssubprotocol".into()),
        ]),
        remote_endpoint: Some(RemoteEndpoint {
            address: "10.0.0.1".into(),
            port: 54321,
        }),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: AcceptCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

#[test]
fn accept_command_serialization_field_names() {
    let cmd = AcceptCommand {
        address: "wss://example.com".into(),
        id: "id-1".into(),
        connect_headers: HashMap::from([("Host".into(), "example.com".into())]),
        remote_endpoint: None,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"connectHeaders\""), "expected camelCase key `connectHeaders`");
    assert!(!json.contains("\"connect_headers\""), "should not contain snake_case key");
    assert!(!json.contains("\"remoteEndpoint\""), "None remote_endpoint should be omitted");
}

#[test]
fn accept_command_deserialization_from_service_json() {
    let json = r#"{
        "address":"wss://dc-node.servicebus.windows.net:443/$hc/test?sb-hc-action=accept&sb-hc-id=abc",
        "id":"4cb542c3-047a-4d40-a19f-bdc66441e736",
        "connectHeaders":{
            "Host":"contoso.servicebus.windows.net",
            "Sec-WebSocket-Protocol":"wssubprotocol"
        }
    }"#;
    let cmd: AcceptCommand = serde_json::from_str(json).unwrap();
    assert_eq!(cmd.id, "4cb542c3-047a-4d40-a19f-bdc66441e736");
    assert_eq!(cmd.connect_headers.get("Host").unwrap(), "contoso.servicebus.windows.net");
    assert!(cmd.remote_endpoint.is_none());
}

#[test]
fn accept_command_empty_headers_omitted() {
    let cmd = AcceptCommand {
        address: "wss://x".into(),
        id: "id".into(),
        connect_headers: HashMap::new(),
        remote_endpoint: None,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(!json.contains("connectHeaders"), "empty map should be skipped");
}

// ── RequestCommand ──────────────────────────────────────────────────────

#[test]
fn request_command_round_trip() {
    let cmd = RequestCommand {
        address: "wss://rendezvous".into(),
        id: "req-001".into(),
        request_target: "/api/data?page=1".into(),
        method: "POST".into(),
        request_headers: HashMap::from([("Content-Type".into(), "application/json".into())]),
        body: true,
        remote_endpoint: Some(RemoteEndpoint {
            address: "192.168.1.5".into(),
            port: 8080,
        }),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: RequestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

#[test]
fn request_command_serialization_field_names() {
    let cmd = RequestCommand {
        address: "wss://a".into(),
        id: "id".into(),
        request_target: "/".into(),
        method: "GET".into(),
        request_headers: HashMap::new(),
        body: false,
        remote_endpoint: None,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"requestTarget\""));
    assert!(json.contains("\"method\""));
    assert!(!json.contains("\"requestHeaders\""), "empty map should be skipped");
    assert!(!json.contains("\"remoteEndpoint\""), "None should be skipped");
}

// ── ResponseCommand ─────────────────────────────────────────────────────

#[test]
fn response_command_round_trip() {
    let cmd = ResponseCommand {
        request_id: "req-001".into(),
        status_code: 200,
        status_description: Some("OK".into()),
        response_headers: HashMap::from([("Content-Length".into(), "42".into())]),
        body: true,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: ResponseCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

#[test]
fn response_command_optional_fields_omitted() {
    let cmd = ResponseCommand {
        request_id: "req-002".into(),
        status_code: 404,
        status_description: None,
        response_headers: HashMap::new(),
        body: false,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(!json.contains("statusDescription"), "None should be omitted");
    assert!(!json.contains("responseHeaders"), "empty map should be omitted");
    assert!(json.contains("\"requestId\""));
    assert!(json.contains("\"statusCode\""));
}

// ── RenewTokenCommand ───────────────────────────────────────────────────

#[test]
fn renew_token_round_trip() {
    let cmd = RenewTokenCommand {
        token: "SharedAccessSignature sr=...".into(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"token\""));
    let deserialized: RenewTokenCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

// ── ListenerCommand enum ────────────────────────────────────────────────

#[test]
fn listener_command_accept_variant_serialization() {
    let cmd = ListenerCommand::Accept(AcceptCommand {
        address: "wss://node".into(),
        id: "guid-1".into(),
        connect_headers: HashMap::new(),
        remote_endpoint: None,
    });
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"accept\""), "enum variant should serialize as `accept`");
    let deserialized: ListenerCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

#[test]
fn listener_command_request_variant_serialization() {
    let cmd = ListenerCommand::Request(RequestCommand {
        address: "wss://node".into(),
        id: "req-99".into(),
        request_target: "/hello".into(),
        method: "GET".into(),
        request_headers: HashMap::new(),
        body: false,
        remote_endpoint: None,
    });
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"request\""), "enum variant should serialize as `request`");
    let deserialized: ListenerCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deserialized);
}

#[test]
fn listener_command_deserialization_accept_from_json() {
    let json = r#"{"accept":{"address":"wss://dc-node.servicebus.windows.net:443/$hc/test?sb-hc-action=accept&sb-hc-id=abc","id":"4cb542c3-047a-4d40-a19f-bdc66441e736","connectHeaders":{"Host":"contoso.servicebus.windows.net","Sec-WebSocket-Protocol":"wssubprotocol"}}}"#;
    let cmd: ListenerCommand = serde_json::from_str(json).unwrap();
    match cmd {
        ListenerCommand::Accept(accept) => {
            assert_eq!(accept.id, "4cb542c3-047a-4d40-a19f-bdc66441e736");
            assert_eq!(accept.connect_headers.len(), 2);
        }
        _ => panic!("expected Accept variant"),
    }
}

#[test]
fn listener_command_deserialization_request_from_json() {
    let json = r#"{"request":{"address":"wss://rv","id":"r1","requestTarget":"/path","method":"POST","requestHeaders":{"Content-Type":"text/plain"},"body":true,"remoteEndpoint":{"address":"10.1.2.3","port":9999}}}"#;
    let cmd: ListenerCommand = serde_json::from_str(json).unwrap();
    match cmd {
        ListenerCommand::Request(req) => {
            assert_eq!(req.method, "POST");
            assert!(req.body);
            let ep = req.remote_endpoint.unwrap();
            assert_eq!(ep.address, "10.1.2.3");
            assert_eq!(ep.port, 9999);
        }
        _ => panic!("expected Request variant"),
    }
}

// ── ListenerResponse enum ───────────────────────────────────────────────

#[test]
fn listener_response_response_variant() {
    let resp = ListenerResponse::Response(ResponseCommand {
        request_id: "r1".into(),
        status_code: 200,
        status_description: Some("OK".into()),
        response_headers: HashMap::from([("X-Custom".into(), "value".into())]),
        body: false,
    });
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"response\""));
    let deserialized: ListenerResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(resp, deserialized);
}

#[test]
fn listener_response_renew_token_variant() {
    let resp = ListenerResponse::RenewToken(RenewTokenCommand {
        token: "sas-token-here".into(),
    });
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"renewToken\""), "should serialize as camelCase `renewToken`");
    let deserialized: ListenerResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(resp, deserialized);
}

// ── URI construction ────────────────────────────────────────────────────

#[test]
fn build_uri_basic() {
    let url = build_uri("contoso.servicebus.windows.net", 443, "myconn", "listen", "tracking-123");
    assert_eq!(url.scheme(), "wss");
    assert_eq!(url.host_str(), Some("contoso.servicebus.windows.net"));
    // Port 443 is the default for wss://, so the url crate normalises it away.
    assert!(url.port().is_none() || url.port() == Some(443));
    assert!(url.path().contains("$hc/myconn"));

    let pairs: HashMap<_, _> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs.get(query_params::ACTION).unwrap(), "listen");
    assert_eq!(pairs.get(query_params::ID).unwrap(), "tracking-123");
}

#[test]
fn build_uri_preserves_path_with_slashes() {
    let url = build_uri("host.example.com", 443, "a/b/c", "connect", "id-1");
    assert!(url.path().ends_with("$hc/a/b/c"));
}

#[test]
fn build_uri_with_token_includes_token() {
    let url = build_uri_with_token(
        "contoso.servicebus.windows.net",
        443,
        "myconn",
        "listen",
        "id-1",
        "SharedAccessSignature sr=http%3a%2f%2fcontoso",
    );
    let pairs: HashMap<_, _> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs.get(query_params::ACTION).unwrap(), "listen");
    assert_eq!(pairs.get(query_params::ID).unwrap(), "id-1");
    assert_eq!(
        pairs.get(query_params::TOKEN).unwrap(),
        "SharedAccessSignature sr=http%3a%2f%2fcontoso"
    );
}

#[test]
fn build_uri_with_token_has_three_query_params() {
    let url = build_uri_with_token("h", 443, "p", "accept", "i", "t");
    let count = url.query_pairs().count();
    assert_eq!(count, 3, "should have action, id, and token");
}

// ── filter_hybrid_connection_query_params ───────────────────────────────

#[test]
fn filter_query_params_removes_all_sb_hc() {
    let url = Url::parse("wss://host:443/$hc/path?sb-hc-action=listen&sb-hc-id=id1&sb-hc-token=tok").unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    assert!(filtered.query().is_none() || filtered.query() == Some(""));
}

#[test]
fn filter_query_params_preserves_custom_params() {
    let url = Url::parse("wss://host:443/$hc/path?sb-hc-action=listen&custom=value&foo=bar").unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    let pairs: HashMap<_, _> = filtered.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs.get("custom").unwrap(), "value");
    assert_eq!(pairs.get("foo").unwrap(), "bar");
    assert!(!pairs.contains_key("sb-hc-action"));
}

#[test]
fn filter_query_params_no_query_string() {
    let url = Url::parse("wss://host:443/$hc/path").unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    assert!(filtered.query().is_none());
}

#[test]
fn filter_query_params_only_custom_params_unchanged() {
    let url = Url::parse("wss://host:443/path?a=1&b=2").unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    let pairs: HashMap<_, _> = filtered.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs.get("a").unwrap(), "1");
    assert_eq!(pairs.get("b").unwrap(), "2");
}

#[test]
fn filter_query_params_mixed_sb_hc_and_custom() {
    let url = Url::parse(
        "wss://host:443/$hc/p?before=1&sb-hc-action=connect&middle=2&sb-hc-id=x&after=3"
    ).unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    let pairs: HashMap<_, _> = filtered.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs.get("before").unwrap(), "1");
    assert_eq!(pairs.get("middle").unwrap(), "2");
    assert_eq!(pairs.get("after").unwrap(), "3");
}

#[test]
fn filter_query_params_preserves_scheme_host_path() {
    let url = Url::parse("wss://myhost:9443/$hc/mypath?sb-hc-action=listen&keep=yes").unwrap();
    let filtered = filter_hybrid_connection_query_params(&url);
    assert_eq!(filtered.scheme(), "wss");
    assert_eq!(filtered.host_str(), Some("myhost"));
    assert_eq!(filtered.port(), Some(9443));
    assert_eq!(filtered.path(), "/$hc/mypath");
}

// ── Constants ───────────────────────────────────────────────────────────

#[test]
fn hc_path_prefix_value() {
    assert_eq!(HC_PATH_PREFIX, "$hc/");
}

#[test]
fn query_param_constants() {
    assert_eq!(query_params::ACTION, "sb-hc-action");
    assert_eq!(query_params::ID, "sb-hc-id");
    assert_eq!(query_params::TOKEN, "sb-hc-token");
    assert_eq!(query_params::STATUS_CODE, "sb-hc-statusCode");
    assert_eq!(query_params::STATUS_DESCRIPTION, "sb-hc-statusDescription");
}

#[test]
fn action_constants() {
    assert_eq!(actions::LISTEN, "listen");
    assert_eq!(actions::CONNECT, "connect");
    assert_eq!(actions::ACCEPT, "accept");
    assert_eq!(actions::REQUEST, "request");
}

// ── Header Constants ────────────────────────────────────────────────────

#[test]
fn header_constants() {
    assert_eq!(headers::SERVICE_BUS_AUTHORIZATION, "ServiceBusAuthorization");
    assert_eq!(headers::RELAY_USER_AGENT, "Relay-User-Agent");
}

// ── Audience Normalization ──────────────────────────────────────────────

#[test]
fn normalize_audience_sb_scheme_to_http() {
    let url = Url::parse("sb://contoso.servicebus.windows.net/hyco").unwrap();
    assert_eq!(
        normalize_audience(&url),
        "http://contoso.servicebus.windows.net/hyco/"
    );
}

#[test]
fn normalize_audience_wss_with_hc_prefix() {
    let url = Url::parse("wss://contoso.servicebus.windows.net/$hc/myconn").unwrap();
    assert_eq!(
        normalize_audience(&url),
        "http://contoso.servicebus.windows.net/myconn/"
    );
}

#[test]
fn normalize_audience_uppercase_host_lowercased() {
    let url = Url::parse("wss://CONTOSO.ServiceBus.Windows.NET/$hc/MyConn").unwrap();
    assert_eq!(
        normalize_audience(&url),
        "http://contoso.servicebus.windows.net/MyConn/"
    );
}

#[test]
fn normalize_audience_already_has_trailing_slash() {
    let url = Url::parse("wss://host.example.com/$hc/path/").unwrap();
    assert_eq!(
        normalize_audience(&url),
        "http://host.example.com/path/"
    );
}

#[test]
fn normalize_audience_no_path() {
    let url = Url::parse("wss://host.example.com").unwrap();
    assert_eq!(
        normalize_audience(&url),
        "http://host.example.com/"
    );
}
