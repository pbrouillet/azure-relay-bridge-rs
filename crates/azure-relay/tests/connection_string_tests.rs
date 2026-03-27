//! Integration tests mirroring ConnectionStringBuilderTests.cs from the C# Azure Relay SDK.

use azure_relay::{AuthenticationType, RelayConnectionStringBuilder};

/// Mirror of C# ConnectionStringBuilderOperationValidation
#[test]
fn connection_string_builder_operation_validation() {
    // 1. Build via properties, verify ToString output contains each field
    let mut builder = RelayConnectionStringBuilder::new();
    builder.set_endpoint("sb://contoso.servicebus.windows.net".parse().unwrap());
    builder.set_entity_path("hybrid1");
    builder.set_shared_access_key_name("keyName");
    builder.set_shared_access_key("key123");
    let cs = builder.to_string();
    assert!(cs.contains("Endpoint=sb://contoso.servicebus.windows.net"));
    assert!(cs.contains("EntityPath=hybrid1"));
    assert!(cs.contains("SharedAccessKeyName=keyName"));
    assert!(cs.contains("SharedAccessKey=key123"));

    // 2. SAS key name alone (without key) should fail validation
    let mut builder2 = RelayConnectionStringBuilder::new();
    builder2.set_shared_access_key_name("keyName");
    assert!(builder2.validate().is_err());

    // 3. SAS key alone (without key name) should fail validation
    let mut builder3 = RelayConnectionStringBuilder::new();
    builder3.set_shared_access_key("key123");
    assert!(builder3.validate().is_err());

    // 4. SAS signature cannot coexist with SAS key
    let mut builder4 = RelayConnectionStringBuilder::new();
    builder4.set_shared_access_key_name("keyName");
    builder4.set_shared_access_key("key123");
    builder4.set_shared_access_signature(
        "SharedAccessSignature sr=foo&sig=bar&se=99999999999&skn=key",
    );
    assert!(builder4.validate().is_err());
}

/// Mirror of C# CreateConnectionStringBuilderFromConnectionString
#[test]
fn create_connection_string_builder_from_connection_string() {
    let original = "Endpoint=sb://contoso.servicebus.windows.net;\
                    EntityPath=hybrid1;\
                    SharedAccessKeyName=keyName;\
                    SharedAccessKey=key123";
    let builder = RelayConnectionStringBuilder::from_connection_string(original).unwrap();

    // Verify all properties parsed correctly
    // Note: url::Url does not add a trailing slash for non-special schemes like sb://
    assert_eq!(
        builder.endpoint().unwrap().as_str(),
        "sb://contoso.servicebus.windows.net"
    );
    assert_eq!(builder.entity_path(), Some("hybrid1"));
    assert_eq!(builder.shared_access_key_name(), Some("keyName"));
    assert_eq!(builder.shared_access_key(), Some("key123"));

    // Round-trip: serialize back and parse again
    let cs = builder.to_string();
    let builder2 = RelayConnectionStringBuilder::from_connection_string(&cs).unwrap();
    assert_eq!(builder.endpoint(), builder2.endpoint());
    assert_eq!(builder.entity_path(), builder2.entity_path());
    assert_eq!(
        builder.shared_access_key_name(),
        builder2.shared_access_key_name()
    );
    assert_eq!(builder.shared_access_key(), builder2.shared_access_key());
}

/// Mirror of C# ManagedIdentityConnectionStringTest
#[test]
fn managed_identity_connection_string() {
    // "Managed Identity" (with space)
    let cs1 = "Endpoint=sb://contoso.servicebus.windows.net;Authentication=Managed Identity";
    let b1 = RelayConnectionStringBuilder::from_connection_string(cs1).unwrap();
    assert_eq!(b1.authentication(), &AuthenticationType::ManagedIdentity);

    // "ManagedIdentity" (no space)
    let cs2 = "Endpoint=sb://contoso.servicebus.windows.net;Authentication=ManagedIdentity";
    let b2 = RelayConnectionStringBuilder::from_connection_string(cs2).unwrap();
    assert_eq!(b2.authentication(), &AuthenticationType::ManagedIdentity);

    // Managed identity with SAS credentials should fail validation
    let cs3 = "Endpoint=sb://contoso.servicebus.windows.net;\
               Authentication=Managed Identity;\
               SharedAccessKeyName=key;\
               SharedAccessKey=value";
    let b3 = RelayConnectionStringBuilder::from_connection_string(cs3).unwrap();
    assert!(b3.validate().is_err());

    // "Garbage" -> Other
    let cs4 = "Endpoint=sb://contoso.servicebus.windows.net;Authentication=Garbage";
    let b4 = RelayConnectionStringBuilder::from_connection_string(cs4).unwrap();
    assert_eq!(b4.authentication(), &AuthenticationType::Other);
}
