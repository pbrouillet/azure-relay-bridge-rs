//! Integration tests mirroring ParameterValidationTests.cs from the C# Azure Relay SDK.

/// Mirror of C# TokenProviderValidation
#[tokio::test]
async fn token_provider_validation() {
    use azure_relay::token_provider::{SharedAccessSignatureTokenProvider, TokenProvider};
    use std::time::Duration;

    // Empty key name
    assert!(SharedAccessSignatureTokenProvider::new("", "key").is_err());
    // Empty key value
    assert!(SharedAccessSignatureTokenProvider::new("name", "").is_err());
    // Key name > 256 chars
    let long = "a".repeat(257);
    assert!(SharedAccessSignatureTokenProvider::new(&long, "key").is_err());
    // Key value > 256 chars
    assert!(SharedAccessSignatureTokenProvider::new("name", &long).is_err());

    // Valid provider, but empty audience
    let provider = SharedAccessSignatureTokenProvider::new("key", "secret").unwrap();
    assert!(provider
        .get_token("", Duration::from_secs(60))
        .await
        .is_err());
    // Zero valid_for
    assert!(provider
        .get_token("http://example.com", Duration::ZERO)
        .await
        .is_err());
}

/// Mirror of C# RelayConnectionStringBuilderValidation
#[test]
fn relay_connection_string_builder_validation() {
    use azure_relay::RelayConnectionStringBuilder;

    // Malformed pair (no '=' delimiter)
    assert!(RelayConnectionStringBuilder::from_connection_string("noequals").is_err());
    // Non-absolute URI endpoint
    assert!(RelayConnectionStringBuilder::from_connection_string("Endpoint=not-a-uri").is_err());
    // Unknown key
    assert!(RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;NOT_A_KEY=value"
    )
    .is_err());
    // Empty value
    assert!(RelayConnectionStringBuilder::from_connection_string("Endpoint=").is_err());
    // Invalid operation timeout
    assert!(RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;OperationTimeout=not_a_time"
    )
    .is_err());
    // Valid edge cases should succeed
    assert!(
        RelayConnectionStringBuilder::from_connection_string(
            "Endpoint=sb://test.servicebus.windows.net"
        )
        .is_ok()
    );
    assert!(RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;EntityPath=hyco"
    )
    .is_ok());
}

/// Mirror of C# ClientValidation
#[test]
fn client_validation() {
    use azure_relay::RelayConnectionStringBuilder;

    // Missing EntityPath — not an error at builder level
    let builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;\
         SharedAccessKeyName=key;\
         SharedAccessKey=value",
    )
    .unwrap();
    assert!(builder.validate().is_ok());
    assert!(builder.entity_path().is_none());

    // Duplicate EntityPath (in connection string AND separately set) — builder stores latest
    let mut builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;EntityPath=one",
    )
    .unwrap();
    builder.set_entity_path("two");
    assert_eq!(builder.entity_path(), Some("two"));

    // SAS key without key name fails validation
    let builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;SharedAccessKey=value",
    )
    .unwrap();
    assert!(builder.validate().is_err());
}

/// Mirror of C# ListenerValidation
#[test]
fn listener_validation() {
    use azure_relay::RelayConnectionStringBuilder;

    // SAS key name without SAS key fails validation
    let builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;SharedAccessKeyName=key",
    )
    .unwrap();
    assert!(builder.validate().is_err());

    // Valid: full SAS credentials
    let builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;\
         SharedAccessKeyName=key;\
         SharedAccessKey=value",
    )
    .unwrap();
    assert!(builder.validate().is_ok());

    // Valid: signature only
    let builder = RelayConnectionStringBuilder::from_connection_string(
        "Endpoint=sb://test.servicebus.windows.net;\
         SharedAccessSignature=SharedAccessSignature sr=foo&sig=bar&se=99999999999&skn=key",
    )
    .unwrap();
    assert!(builder.validate().is_ok());
}
