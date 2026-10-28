use azure_relay::{HybridConnectionClient, RelayError, build_relay_address};
use url::Url;

#[test]
fn build_relay_address_matches_upstream_address_format() {
    let endpoint = Url::parse("sb://mynamespace.servicebus.windows.net/").unwrap();
    let address = build_relay_address(&endpoint, "myconn").unwrap();
    assert_eq!(
        address.as_str(),
        "wss://mynamespace.servicebus.windows.net/$hc/myconn"
    );
    assert_eq!(address.port_or_known_default(), Some(443));
}

#[test]
fn build_relay_address_preserves_port_and_nested_entity_path() {
    let endpoint =
        Url::parse("sb://ns.servicebus.windows.net:8443/ignored?old=query#fragment").unwrap();
    assert_eq!(
        build_relay_address(&endpoint, "group/myconn")
            .unwrap()
            .as_str(),
        "wss://ns.servicebus.windows.net:8443/$hc/group/myconn"
    );
}

#[test]
fn build_relay_address_preserves_empty_and_leading_slash_entity_paths() {
    let endpoint = Url::parse("sb://ns.servicebus.windows.net/").unwrap();
    for (entity_path, expected_path) in [("", "/$hc/"), ("/myconn", "/$hc//myconn")] {
        assert_eq!(
            build_relay_address(&endpoint, entity_path).unwrap().path(),
            expected_path
        );
    }
}

#[test]
fn build_relay_address_missing_host_returns_err() {
    let endpoint = Url::parse("sb:///").unwrap();
    assert!(matches!(
        build_relay_address(&endpoint, "myconn"),
        Err(RelayError::InvalidConnectionString(message)) if message == "missing host"
    ));
}

#[test]
fn build_relay_address_invalid_websocket_host_returns_err() {
    let endpoint = Url::parse("sb://bad%20host/").unwrap();
    assert!(matches!(
        build_relay_address(&endpoint, "myconn"),
        Err(RelayError::InvalidConnectionString(_))
    ));
}

#[test]
fn build_relay_address_matches_client_connection_string_constructor() {
    let connection_string = "Endpoint=sb://ns.servicebus.windows.net:8443/;EntityPath=group/myconn;\
                             SharedAccessKeyName=key;SharedAccessKey=dGVzdA==";
    let expected = build_relay_address(
        &Url::parse("sb://ns.servicebus.windows.net:8443/").unwrap(),
        "group/myconn",
    )
    .unwrap();
    let client = HybridConnectionClient::from_connection_string(connection_string).unwrap();
    assert_eq!(client.address(), &expected);
}
