use std::fmt;
use std::str::FromStr;
use std::time::Duration;
use url::Url;

use crate::error::{RelayError, Result};

// Known connection string keys (lowercase for case-insensitive matching).
const KEY_ENDPOINT: &str = "endpoint";
const KEY_ENTITY_PATH: &str = "entitypath";
const KEY_SAS_KEY_NAME: &str = "sharedaccesskeyname";
const KEY_SAS_KEY: &str = "sharedaccesskey";
const KEY_SAS_SIGNATURE: &str = "sharedaccesssignature";
const KEY_OPERATION_TIMEOUT: &str = "operationtimeout";
const KEY_AUTHENTICATION: &str = "authentication";

const KNOWN_KEYS: &[&str] = &[
    KEY_ENDPOINT,
    KEY_ENTITY_PATH,
    KEY_SAS_KEY_NAME,
    KEY_SAS_KEY,
    KEY_SAS_SIGNATURE,
    KEY_OPERATION_TIMEOUT,
    KEY_AUTHENTICATION,
];

/// Authentication type for managed identity connections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AuthenticationType {
    /// No special authentication (uses SAS or no auth).
    #[default]
    None,
    /// Azure Managed Identity authentication.
    ManagedIdentity,
    /// Other/unknown authentication type.
    Other,
}

/// Builds and parses Azure Relay connection strings.
///
/// Connection string format:
/// `Endpoint=sb://namespace.servicebus.windows.net;EntityPath=hybridconnection;SharedAccessKeyName=keyname;SharedAccessKey=key`
///
/// Supported keys:
/// - `Endpoint` (required) — the relay namespace URI (sb:// scheme)
/// - `EntityPath` — the Hybrid Connection name
/// - `SharedAccessKeyName` — SAS policy name
/// - `SharedAccessKey` — SAS policy key (base64)
/// - `SharedAccessSignature` — pre-built SAS token
/// - `OperationTimeout` — timeout as Duration string (e.g., "00:01:00")
/// - `Authentication` — "Managed Identity" or "ManagedIdentity"
#[derive(Debug, Clone)]
pub struct RelayConnectionStringBuilder {
    endpoint: Option<Url>,
    entity_path: Option<String>,
    shared_access_key_name: Option<String>,
    shared_access_key: Option<String>,
    shared_access_signature: Option<String>,
    operation_timeout: Option<Duration>,
    authentication: AuthenticationType,
}

impl Default for RelayConnectionStringBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayConnectionStringBuilder {
    /// Creates an empty builder with no fields set.
    pub fn new() -> Self {
        Self {
            endpoint: None,
            entity_path: None,
            shared_access_key_name: None,
            shared_access_key: None,
            shared_access_signature: None,
            operation_timeout: None,
            authentication: AuthenticationType::None,
        }
    }

    /// Parses a connection string into a builder.
    ///
    /// The connection string is a set of `key=value` pairs separated by `;`.
    /// Keys are matched case-insensitively. Values may contain `=` characters
    /// (only the first `=` in each pair is treated as the delimiter).
    pub fn from_connection_string(s: &str) -> Result<Self> {
        let mut builder = Self::new();

        for part in s.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let (key, value) = part.split_once('=').ok_or_else(|| {
                RelayError::InvalidConnectionString(format!(
                    "Key-value pair missing '=' delimiter: '{part}'"
                ))
            })?;

            let key = key.trim();
            let value = value.trim();

            if value.is_empty() {
                return Err(RelayError::InvalidConnectionString(format!(
                    "Empty value for key '{key}'"
                )));
            }

            let key_lower = key.to_ascii_lowercase();
            if !KNOWN_KEYS.contains(&key_lower.as_str()) {
                return Err(RelayError::InvalidConnectionString(format!(
                    "Unknown key '{key}'"
                )));
            }

            match key_lower.as_str() {
                KEY_ENDPOINT => {
                    let url = Url::parse(value).map_err(|e| {
                        RelayError::InvalidConnectionString(format!(
                            "Invalid Endpoint URI '{value}': {e}"
                        ))
                    })?;
                    builder.endpoint = Some(url);
                }
                KEY_ENTITY_PATH => {
                    builder.entity_path = Some(value.to_string());
                }
                KEY_SAS_KEY_NAME => {
                    builder.shared_access_key_name = Some(value.to_string());
                }
                KEY_SAS_KEY => {
                    builder.shared_access_key = Some(value.to_string());
                }
                KEY_SAS_SIGNATURE => {
                    builder.shared_access_signature = Some(value.to_string());
                }
                KEY_OPERATION_TIMEOUT => {
                    let duration = parse_timespan(value)?;
                    builder.operation_timeout = Some(duration);
                }
                KEY_AUTHENTICATION => {
                    builder.authentication = parse_authentication_type(value);
                }
                _ => unreachable!(),
            }
        }

        Ok(builder)
    }

    // -- Getters --

    /// Returns the relay namespace endpoint URI, if set.
    pub fn endpoint(&self) -> Option<&Url> {
        self.endpoint.as_ref()
    }

    /// Returns the Hybrid Connection entity path, if set.
    pub fn entity_path(&self) -> Option<&str> {
        self.entity_path.as_deref()
    }

    /// Returns the SAS policy name, if set.
    pub fn shared_access_key_name(&self) -> Option<&str> {
        self.shared_access_key_name.as_deref()
    }

    /// Returns the SAS policy key, if set.
    pub fn shared_access_key(&self) -> Option<&str> {
        self.shared_access_key.as_deref()
    }

    /// Returns the pre-built SAS token, if set.
    pub fn shared_access_signature(&self) -> Option<&str> {
        self.shared_access_signature.as_deref()
    }

    /// Returns the operation timeout, if set.
    pub fn operation_timeout(&self) -> Option<Duration> {
        self.operation_timeout
    }

    /// Returns the authentication type.
    pub fn authentication(&self) -> &AuthenticationType {
        &self.authentication
    }

    // -- Setters (builder pattern) --

    /// Sets the relay namespace endpoint URI.
    pub fn set_endpoint(&mut self, endpoint: Url) -> &mut Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// Clears the endpoint.
    pub fn clear_endpoint(&mut self) -> &mut Self {
        self.endpoint = None;
        self
    }

    /// Sets the Hybrid Connection entity path.
    pub fn set_entity_path(&mut self, entity_path: impl Into<String>) -> &mut Self {
        self.entity_path = Some(entity_path.into());
        self
    }

    /// Clears the entity path.
    pub fn clear_entity_path(&mut self) -> &mut Self {
        self.entity_path = None;
        self
    }

    /// Sets the SAS policy name.
    pub fn set_shared_access_key_name(
        &mut self,
        key_name: impl Into<String>,
    ) -> &mut Self {
        self.shared_access_key_name = Some(key_name.into());
        self
    }

    /// Clears the SAS policy name.
    pub fn clear_shared_access_key_name(&mut self) -> &mut Self {
        self.shared_access_key_name = None;
        self
    }

    /// Sets the SAS policy key.
    pub fn set_shared_access_key(&mut self, key: impl Into<String>) -> &mut Self {
        self.shared_access_key = Some(key.into());
        self
    }

    /// Clears the SAS policy key.
    pub fn clear_shared_access_key(&mut self) -> &mut Self {
        self.shared_access_key = None;
        self
    }

    /// Sets a pre-built SAS token.
    pub fn set_shared_access_signature(
        &mut self,
        signature: impl Into<String>,
    ) -> &mut Self {
        self.shared_access_signature = Some(signature.into());
        self
    }

    /// Clears the SAS token.
    pub fn clear_shared_access_signature(&mut self) -> &mut Self {
        self.shared_access_signature = None;
        self
    }

    /// Sets the operation timeout.
    pub fn set_operation_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.operation_timeout = Some(timeout);
        self
    }

    /// Clears the operation timeout.
    pub fn clear_operation_timeout(&mut self) -> &mut Self {
        self.operation_timeout = None;
        self
    }

    /// Sets the authentication type.
    pub fn set_authentication(&mut self, auth: AuthenticationType) -> &mut Self {
        self.authentication = auth;
        self
    }

    // -- Validation --

    /// Validates the builder's state for mutual-exclusion rules.
    ///
    /// Rules:
    /// - `SharedAccessKey` and `SharedAccessKeyName` must both be set or both absent.
    /// - `SharedAccessSignature` cannot coexist with `SharedAccessKey`/`SharedAccessKeyName`.
    /// - `ManagedIdentity` authentication cannot coexist with any SAS credential.
    pub fn validate(&self) -> Result<()> {
        let has_key = self.shared_access_key.is_some();
        let has_key_name = self.shared_access_key_name.is_some();
        let has_signature = self.shared_access_signature.is_some();

        // SharedAccessKey and SharedAccessKeyName must be paired.
        if has_key && !has_key_name {
            return Err(RelayError::InvalidArgument {
                name: "SharedAccessKeyName".into(),
                message: "SharedAccessKeyName is required when SharedAccessKey is set".into(),
            });
        }
        if has_key_name && !has_key {
            return Err(RelayError::InvalidArgument {
                name: "SharedAccessKey".into(),
                message: "SharedAccessKey is required when SharedAccessKeyName is set".into(),
            });
        }

        // SharedAccessSignature is mutually exclusive with key+keyname.
        if has_signature && (has_key || has_key_name) {
            return Err(RelayError::InvalidArgument {
                name: "SharedAccessSignature".into(),
                message: "SharedAccessSignature cannot be used together with SharedAccessKey or SharedAccessKeyName".into(),
            });
        }

        // ManagedIdentity is mutually exclusive with any SAS credential.
        if self.authentication == AuthenticationType::ManagedIdentity
            && (has_key || has_key_name || has_signature)
        {
            return Err(RelayError::InvalidArgument {
                name: "Authentication".into(),
                message: "Managed Identity authentication cannot be combined with SAS credentials".into(),
            });
        }

        Ok(())
    }
}

// -- Display --

impl fmt::Display for RelayConnectionStringBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();

        if let Some(ref endpoint) = self.endpoint {
            parts.push(format!("Endpoint={endpoint}"));
        }
        if let Some(ref entity_path) = self.entity_path {
            parts.push(format!("EntityPath={entity_path}"));
        }
        if let Some(ref key_name) = self.shared_access_key_name {
            parts.push(format!("SharedAccessKeyName={key_name}"));
        }
        if let Some(ref key) = self.shared_access_key {
            parts.push(format!("SharedAccessKey={key}"));
        }
        if let Some(ref sig) = self.shared_access_signature {
            parts.push(format!("SharedAccessSignature={sig}"));
        }
        if let Some(timeout) = self.operation_timeout {
            parts.push(format!("OperationTimeout={}", format_timespan(timeout)));
        }
        match self.authentication {
            AuthenticationType::None => {}
            AuthenticationType::ManagedIdentity => {
                parts.push("Authentication=ManagedIdentity".to_string());
            }
            AuthenticationType::Other => {
                parts.push("Authentication=Other".to_string());
            }
        }

        write!(f, "{}", parts.join(";"))
    }
}

// -- FromStr --

impl FromStr for RelayConnectionStringBuilder {
    type Err = RelayError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::from_connection_string(s)
    }
}

// -- Helper functions --

/// Parses a timespan string into a [`Duration`].
///
/// Supported formats:
/// - `hh:mm:ss` (e.g., `"00:01:30"` → 90 seconds)
/// - `d.hh:mm:ss` (e.g., `"1.02:03:04"` → 1 day + 2h + 3m + 4s)
/// - `hh:mm:ss.fff` (with fractional seconds)
/// - Plain seconds as float (e.g., `"90"` or `"90.5"`)
fn parse_timespan(s: &str) -> Result<Duration> {
    let s = s.trim();

    // Try plain seconds (integer or float).
    if let Ok(secs) = s.parse::<f64>() {
        if secs < 0.0 {
            return Err(RelayError::InvalidConnectionString(format!(
                "OperationTimeout cannot be negative: '{s}'"
            )));
        }
        return Ok(Duration::from_secs_f64(secs));
    }

    // Try d.hh:mm:ss or hh:mm:ss, optionally with fractional seconds.
    let (days, time_part) = if let Some((d, rest)) = s.split_once('.') {
        // Could be "d.hh:mm:ss" or "hh:mm:ss.fff"
        // Distinguish: if the part before '.' contains ':', it's "hh:mm:ss.fff"
        if d.contains(':') {
            // "hh:mm:ss.fff" — no days
            (0u64, s)
        } else if let Ok(day_val) = d.parse::<u64>() {
            (day_val, rest)
        } else {
            return Err(RelayError::InvalidConnectionString(format!(
                "Invalid OperationTimeout format: '{s}'"
            )));
        }
    } else {
        (0, s)
    };

    // Parse hh:mm:ss or hh:mm:ss.fff from time_part.
    let (hms, frac) = if let Some((hms_str, frac_str)) = time_part.split_once('.') {
        // Only split on the last '.' that separates seconds from fractional part.
        // hms_str should look like "hh:mm:ss", frac_str is fractional seconds.
        if hms_str.contains(':') {
            let frac_secs: f64 = format!("0.{frac_str}").parse().map_err(|_| {
                RelayError::InvalidConnectionString(format!(
                    "Invalid fractional seconds in OperationTimeout: '{s}'"
                ))
            })?;
            (hms_str, frac_secs)
        } else {
            return Err(RelayError::InvalidConnectionString(format!(
                "Invalid OperationTimeout format: '{s}'"
            )));
        }
    } else {
        (time_part, 0.0)
    };

    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() != 3 {
        return Err(RelayError::InvalidConnectionString(format!(
            "Invalid OperationTimeout format (expected hh:mm:ss): '{s}'"
        )));
    }

    let hours: u64 = parts[0].parse().map_err(|_| {
        RelayError::InvalidConnectionString(format!("Invalid hours in OperationTimeout: '{s}'"))
    })?;
    let minutes: u64 = parts[1].parse().map_err(|_| {
        RelayError::InvalidConnectionString(format!("Invalid minutes in OperationTimeout: '{s}'"))
    })?;
    let seconds: u64 = parts[2].parse().map_err(|_| {
        RelayError::InvalidConnectionString(format!("Invalid seconds in OperationTimeout: '{s}'"))
    })?;

    let total_secs = days * 86400 + hours * 3600 + minutes * 60 + seconds;
    let duration = Duration::from_secs(total_secs) + Duration::from_secs_f64(frac);

    Ok(duration)
}

/// Formats a [`Duration`] as a `hh:mm:ss` timespan string.
fn format_timespan(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let nanos = d.subsec_nanos();

    if nanos == 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        let frac = nanos as f64 / 1_000_000_000.0;
        let frac_str = format!("{frac:.7}");
        // Strip the leading "0"
        let frac_str = frac_str.trim_start_matches('0');
        format!("{hours:02}:{minutes:02}:{seconds:02}{frac_str}")
    }
}

/// Maps an authentication value string to an [`AuthenticationType`].
fn parse_authentication_type(value: &str) -> AuthenticationType {
    let normalized = value.replace(' ', "").to_ascii_lowercase();
    match normalized.as_str() {
        "managedidentity" => AuthenticationType::ManagedIdentity,
        _ => AuthenticationType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_builder_is_empty() {
        let b = RelayConnectionStringBuilder::new();
        assert!(b.endpoint().is_none());
        assert!(b.entity_path().is_none());
        assert!(b.shared_access_key_name().is_none());
        assert!(b.shared_access_key().is_none());
        assert!(b.shared_access_signature().is_none());
        assert!(b.operation_timeout().is_none());
        assert_eq!(*b.authentication(), AuthenticationType::None);
    }

    #[test]
    fn test_parse_full_connection_string() {
        let cs = "Endpoint=sb://mynamespace.servicebus.windows.net/;\
                   EntityPath=myentity;\
                   SharedAccessKeyName=mykeyname;\
                   SharedAccessKey=bXlrZXk=";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();

        assert_eq!(
            b.endpoint().unwrap().as_str(),
            "sb://mynamespace.servicebus.windows.net/"
        );
        assert_eq!(b.entity_path(), Some("myentity"));
        assert_eq!(b.shared_access_key_name(), Some("mykeyname"));
        assert_eq!(b.shared_access_key(), Some("bXlrZXk="));
    }

    #[test]
    fn test_parse_case_insensitive_keys() {
        let cs = "endpoint=sb://ns.servicebus.windows.net;ENTITYPATH=ep";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert!(b.endpoint().is_some());
        assert_eq!(b.entity_path(), Some("ep"));
    }

    #[test]
    fn test_parse_value_with_equals() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;\
                   SharedAccessSignature=SharedAccessSignature sr=sb%3a%2f%2f&sig=abc%3d%3d&se=123";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(
            b.shared_access_signature(),
            Some("SharedAccessSignature sr=sb%3a%2f%2f&sig=abc%3d%3d&se=123")
        );
    }

    #[test]
    fn test_parse_unknown_key_errors() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;BadKey=value";
        let err = RelayConnectionStringBuilder::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn test_parse_empty_value_errors() {
        let cs = "Endpoint=";
        let err = RelayConnectionStringBuilder::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn test_parse_invalid_endpoint_errors() {
        let cs = "Endpoint=not a url";
        let err = RelayConnectionStringBuilder::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn test_parse_operation_timeout_hms() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;OperationTimeout=00:01:30";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(b.operation_timeout(), Some(Duration::from_secs(90)));
    }

    #[test]
    fn test_parse_operation_timeout_seconds() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;OperationTimeout=90";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(b.operation_timeout(), Some(Duration::from_secs(90)));
    }

    #[test]
    fn test_parse_authentication_managed_identity() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;Authentication=Managed Identity";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(*b.authentication(), AuthenticationType::ManagedIdentity);
    }

    #[test]
    fn test_parse_authentication_other() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;Authentication=SomeOtherType";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(*b.authentication(), AuthenticationType::Other);
    }

    #[test]
    fn test_display_round_trip() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net/;\
                   EntityPath=myentity;\
                   SharedAccessKeyName=keyname;\
                   SharedAccessKey=a2V5";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        let output = b.to_string();

        let b2 = RelayConnectionStringBuilder::from_connection_string(&output).unwrap();
        assert_eq!(
            b.endpoint().unwrap().as_str(),
            b2.endpoint().unwrap().as_str()
        );
        assert_eq!(b.entity_path(), b2.entity_path());
        assert_eq!(b.shared_access_key_name(), b2.shared_access_key_name());
        assert_eq!(b.shared_access_key(), b2.shared_access_key());
    }

    #[test]
    fn test_display_omits_none_fields() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_endpoint(Url::parse("sb://ns.servicebus.windows.net").unwrap());
        let s = b.to_string();
        assert!(s.starts_with("Endpoint=sb://ns.servicebus.windows.net"));
        assert!(!s.contains("EntityPath"));
    }

    #[test]
    fn test_from_str_trait() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;EntityPath=ep";
        let b: RelayConnectionStringBuilder = cs.parse().unwrap();
        assert_eq!(b.entity_path(), Some("ep"));
    }

    #[test]
    fn test_validate_key_without_key_name() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key("mykey");
        assert!(b.validate().is_err());
    }

    #[test]
    fn test_validate_key_name_without_key() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key_name("mykeyname");
        assert!(b.validate().is_err());
    }

    #[test]
    fn test_validate_signature_with_key() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key_name("name")
            .set_shared_access_key("key")
            .set_shared_access_signature("sig");
        assert!(b.validate().is_err());
    }

    #[test]
    fn test_validate_managed_identity_with_sas() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_authentication(AuthenticationType::ManagedIdentity)
            .set_shared_access_key_name("name")
            .set_shared_access_key("key");
        assert!(b.validate().is_err());
    }

    #[test]
    fn test_validate_valid_key_pair() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key_name("name")
            .set_shared_access_key("key");
        assert!(b.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_signature_only() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_signature("sig");
        assert!(b.validate().is_ok());
    }

    #[test]
    fn test_validate_managed_identity_alone() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_authentication(AuthenticationType::ManagedIdentity);
        assert!(b.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_builder() {
        let b = RelayConnectionStringBuilder::new();
        assert!(b.validate().is_ok());
    }

    #[test]
    fn test_setters_and_getters() {
        let mut b = RelayConnectionStringBuilder::new();
        let url = Url::parse("sb://test.servicebus.windows.net").unwrap();
        b.set_endpoint(url.clone())
            .set_entity_path("my-hc")
            .set_shared_access_key_name("policy")
            .set_shared_access_key("secret")
            .set_operation_timeout(Duration::from_secs(60))
            .set_authentication(AuthenticationType::ManagedIdentity);

        assert_eq!(b.endpoint().unwrap().as_str(), url.as_str());
        assert_eq!(b.entity_path(), Some("my-hc"));
        assert_eq!(b.shared_access_key_name(), Some("policy"));
        assert_eq!(b.shared_access_key(), Some("secret"));
        assert_eq!(b.operation_timeout(), Some(Duration::from_secs(60)));
        assert_eq!(*b.authentication(), AuthenticationType::ManagedIdentity);
    }

    #[test]
    fn test_clear_methods() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_endpoint(Url::parse("sb://ns.servicebus.windows.net").unwrap())
            .set_entity_path("ep")
            .set_shared_access_key_name("name")
            .set_shared_access_key("key")
            .set_shared_access_signature("sig")
            .set_operation_timeout(Duration::from_secs(30));

        b.clear_endpoint()
            .clear_entity_path()
            .clear_shared_access_key_name()
            .clear_shared_access_key()
            .clear_shared_access_signature()
            .clear_operation_timeout();

        assert!(b.endpoint().is_none());
        assert!(b.entity_path().is_none());
        assert!(b.shared_access_key_name().is_none());
        assert!(b.shared_access_key().is_none());
        assert!(b.shared_access_signature().is_none());
        assert!(b.operation_timeout().is_none());
    }

    #[test]
    fn test_parse_timespan_hms() {
        assert_eq!(parse_timespan("01:30:00").unwrap(), Duration::from_secs(5400));
    }

    #[test]
    fn test_parse_timespan_with_days() {
        // 1 day + 2 hours + 3 minutes + 4 seconds
        assert_eq!(
            parse_timespan("1.02:03:04").unwrap(),
            Duration::from_secs(86400 + 7200 + 180 + 4)
        );
    }

    #[test]
    fn test_parse_timespan_float_seconds() {
        let d = parse_timespan("1.5").unwrap();
        assert_eq!(d, Duration::from_secs_f64(1.5));
    }

    #[test]
    fn test_format_timespan_round_trip() {
        let d = Duration::from_secs(3661); // 1h 1m 1s
        let s = format_timespan(d);
        assert_eq!(s, "01:01:01");
        let d2 = parse_timespan(&s).unwrap();
        assert_eq!(d, d2);
    }

    #[test]
    fn test_trailing_semicolons_ignored() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;EntityPath=ep;";
        let b = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
        assert_eq!(b.entity_path(), Some("ep"));
    }

    #[test]
    fn test_display_with_timeout_and_auth() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_endpoint(Url::parse("sb://ns.servicebus.windows.net").unwrap())
            .set_operation_timeout(Duration::from_secs(60))
            .set_authentication(AuthenticationType::ManagedIdentity);
        let s = b.to_string();
        assert!(s.contains("OperationTimeout=00:01:00"));
        assert!(s.contains("Authentication=ManagedIdentity"));
    }

    #[test]
    fn test_shared_access_signature_getter() {
        let mut b = RelayConnectionStringBuilder::new();
        assert!(b.shared_access_signature().is_none());
        b.set_shared_access_signature("sig-token-value");
        assert_eq!(b.shared_access_signature(), Some("sig-token-value"));
    }

    #[test]
    fn test_operation_timeout_getter() {
        let mut b = RelayConnectionStringBuilder::new();
        assert!(b.operation_timeout().is_none());
        b.set_operation_timeout(Duration::from_secs(120));
        assert_eq!(b.operation_timeout(), Some(Duration::from_secs(120)));
    }

    #[test]
    fn test_clear_endpoint_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_endpoint(Url::parse("sb://ns.servicebus.windows.net").unwrap());
        assert!(b.endpoint().is_some());
        b.clear_endpoint();
        assert!(b.endpoint().is_none());
    }

    #[test]
    fn test_clear_entity_path_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_entity_path("ep");
        assert_eq!(b.entity_path(), Some("ep"));
        b.clear_entity_path();
        assert!(b.entity_path().is_none());
    }

    #[test]
    fn test_clear_shared_access_key_name_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key_name("keyname");
        assert_eq!(b.shared_access_key_name(), Some("keyname"));
        b.clear_shared_access_key_name();
        assert!(b.shared_access_key_name().is_none());
    }

    #[test]
    fn test_clear_shared_access_key_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_key("secret");
        assert_eq!(b.shared_access_key(), Some("secret"));
        b.clear_shared_access_key();
        assert!(b.shared_access_key().is_none());
    }

    #[test]
    fn test_clear_shared_access_signature_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_shared_access_signature("sig");
        assert_eq!(b.shared_access_signature(), Some("sig"));
        b.clear_shared_access_signature();
        assert!(b.shared_access_signature().is_none());
    }

    #[test]
    fn test_clear_operation_timeout_individually() {
        let mut b = RelayConnectionStringBuilder::new();
        b.set_operation_timeout(Duration::from_secs(45));
        assert_eq!(b.operation_timeout(), Some(Duration::from_secs(45)));
        b.clear_operation_timeout();
        assert!(b.operation_timeout().is_none());
    }

    #[test]
    fn test_authentication_type_default_is_none() {
        assert_eq!(AuthenticationType::default(), AuthenticationType::None);
    }
}
