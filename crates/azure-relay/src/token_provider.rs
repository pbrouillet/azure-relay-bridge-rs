use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{RelayError, Result};

type HmacSha256 = Hmac<Sha256>;

/// URL-encodes a string for use in SAS tokens (percent-encoding).
///
/// Encodes everything except unreserved chars: A-Z a-z 0-9 - . _ ~
/// This matches the C# `Uri.EscapeDataString` behavior.
pub fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", byte));
            }
        }
    }
    encoded
}

/// A security token with a value and expiry time.
#[derive(Debug, Clone)]
pub struct SecurityToken {
    /// The token string (e.g., SAS token).
    pub token: String,
    /// When the token expires.
    pub expires_at: SystemTime,
}

/// Trait for providing security tokens for Azure Relay authentication.
pub trait TokenProvider: Send + Sync {
    /// Gets a token for the given audience, valid for the specified duration.
    fn get_token(
        &self,
        audience: &str,
        valid_for: Duration,
    ) -> impl std::future::Future<Output = Result<SecurityToken>> + Send;
}

/// Creates SAS tokens from a shared access key name and key.
#[derive(Debug)]
pub struct SharedAccessSignatureTokenProvider {
    key_name: String,
    key: String,
}

impl SharedAccessSignatureTokenProvider {
    const MAX_KEY_NAME_LENGTH: usize = 256;
    const MAX_KEY_LENGTH: usize = 256;

    /// Creates a new `SharedAccessSignatureTokenProvider`.
    ///
    /// # Errors
    /// Returns `RelayError::InvalidArgument` if `key_name` or `key` is empty or exceeds 256 chars.
    pub fn new(key_name: &str, key: &str) -> Result<Self> {
        if key_name.is_empty() {
            return Err(RelayError::invalid_argument(
                "key_name",
                "must not be empty",
            ));
        }
        if key_name.len() > Self::MAX_KEY_NAME_LENGTH {
            return Err(RelayError::invalid_argument(
                "key_name",
                format!(
                    "must not exceed {} characters",
                    Self::MAX_KEY_NAME_LENGTH
                ),
            ));
        }
        if key.is_empty() {
            return Err(RelayError::invalid_argument("key", "must not be empty"));
        }
        if key.len() > Self::MAX_KEY_LENGTH {
            return Err(RelayError::invalid_argument(
                "key",
                format!("must not exceed {} characters", Self::MAX_KEY_LENGTH),
            ));
        }

        Ok(Self {
            key_name: key_name.to_string(),
            key: key.to_string(),
        })
    }

    /// Returns the shared access key name.
    pub fn key_name(&self) -> &str {
        &self.key_name
    }

    /// Returns the shared access key.
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl TokenProvider for SharedAccessSignatureTokenProvider {
    async fn get_token(
        &self,
        audience: &str,
        valid_for: Duration,
    ) -> Result<SecurityToken> {
        if audience.is_empty() {
            return Err(RelayError::invalid_argument(
                "audience",
                "must not be empty",
            ));
        }
        if valid_for.is_zero() {
            return Err(RelayError::invalid_argument(
                "valid_for",
                "must be greater than zero",
            ));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| RelayError::communication(format!("system time error: {e}")))?;
        let expiry = now.as_secs() + valid_for.as_secs();
        let encoded_audience = url_encode(audience);

        // Signed content: "{url_encoded_audience}\n{expiry}"
        let signed_content = format!("{}\n{}", encoded_audience, expiry);

        // HMAC-SHA256 using the raw UTF-8 bytes of the key (matches C# Encoding.UTF8.GetBytes)
        let mut mac = HmacSha256::new_from_slice(self.key.as_bytes())
            .map_err(|e| RelayError::communication(format!("HMAC key error: {e}")))?;
        mac.update(signed_content.as_bytes());
        let signature = mac.finalize().into_bytes();

        let encoded_signature = url_encode(&STANDARD.encode(signature));

        let token = format!(
            "SharedAccessSignature sr={}&sig={}&se={}&skn={}",
            encoded_audience, encoded_signature, expiry, self.key_name
        );

        let expires_at = UNIX_EPOCH + Duration::from_secs(expiry);

        Ok(SecurityToken { token, expires_at })
    }
}

/// Wraps a pre-built SAS token string.
#[derive(Debug)]
pub struct SharedAccessSignatureToken {
    token: String,
    expires_at: SystemTime,
}

impl SharedAccessSignatureToken {
    const SAS_PREFIX: &str = "SharedAccessSignature ";

    /// Creates a new `SharedAccessSignatureToken` by parsing an existing SAS token string.
    ///
    /// The token must start with `"SharedAccessSignature "` and contain an `se=` (expiry) field.
    ///
    /// # Errors
    /// Returns `RelayError::InvalidArgument` if the token format is invalid.
    pub fn new(token: &str) -> Result<Self> {
        if !token.starts_with(Self::SAS_PREFIX) {
            return Err(RelayError::invalid_argument(
                "token",
                "must start with 'SharedAccessSignature '",
            ));
        }

        let params = &token[Self::SAS_PREFIX.len()..];
        let expiry_str = params
            .split('&')
            .find_map(|pair| pair.strip_prefix("se="))
            .ok_or_else(|| {
                RelayError::invalid_argument("token", "missing 'se' (expiry) field")
            })?;

        let expiry_secs: u64 = expiry_str.parse().map_err(|_| {
            RelayError::invalid_argument("token", "invalid 'se' (expiry) value")
        })?;

        let expires_at = UNIX_EPOCH + Duration::from_secs(expiry_secs);

        Ok(Self {
            token: token.to_string(),
            expires_at,
        })
    }
}

impl TokenProvider for SharedAccessSignatureToken {
    async fn get_token(
        &self,
        _audience: &str,
        _valid_for: Duration,
    ) -> Result<SecurityToken> {
        let now = SystemTime::now();
        if now >= self.expires_at {
            return Err(RelayError::AuthorizationFailed(
                "the shared access signature token has expired".to_string(),
            ));
        }

        Ok(SecurityToken {
            token: self.token.clone(),
            expires_at: self.expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_unreserved_chars_unchanged() {
        let input = "abcXYZ019-._~";
        assert_eq!(url_encode(input), input);
    }

    #[test]
    fn url_encode_special_chars() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a=b&c"), "a%3Db%26c");
        assert_eq!(url_encode("https://example.com"), "https%3A%2F%2Fexample.com");
    }

    #[test]
    fn provider_new_rejects_empty_key_name() {
        let err = SharedAccessSignatureTokenProvider::new("", "key").unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn provider_new_rejects_long_key_name() {
        let long = "a".repeat(257);
        let err = SharedAccessSignatureTokenProvider::new(&long, "key").unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn provider_new_rejects_empty_key() {
        let err = SharedAccessSignatureTokenProvider::new("name", "").unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn provider_new_rejects_long_key() {
        let long = "a".repeat(257);
        let err = SharedAccessSignatureTokenProvider::new("name", &long).unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn provider_new_valid() {
        let provider = SharedAccessSignatureTokenProvider::new("myKey", "mySecret").unwrap();
        assert_eq!(provider.key_name(), "myKey");
        assert_eq!(provider.key(), "mySecret");
    }

    #[tokio::test]
    async fn provider_get_token_rejects_empty_audience() {
        let provider = SharedAccessSignatureTokenProvider::new("key", "secret").unwrap();
        let err = provider.get_token("", Duration::from_secs(60)).await.unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn provider_get_token_rejects_zero_duration() {
        let provider = SharedAccessSignatureTokenProvider::new("key", "secret").unwrap();
        let err = provider.get_token("http://example.com", Duration::ZERO).await.unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn provider_get_token_produces_valid_sas() {
        let provider = SharedAccessSignatureTokenProvider::new("keyName", "keyValue").unwrap();
        let token = provider
            .get_token("http://my.namespace.example.com", Duration::from_secs(3600))
            .await
            .unwrap();

        assert!(token.token.starts_with("SharedAccessSignature "));
        assert!(token.token.contains("sr="));
        assert!(token.token.contains("sig="));
        assert!(token.token.contains("se="));
        assert!(token.token.contains("skn=keyName"));
        assert!(token.expires_at > SystemTime::now());
    }

    #[test]
    fn sas_token_new_rejects_invalid_prefix() {
        let err = SharedAccessSignatureToken::new("Bearer abc").unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn sas_token_new_rejects_missing_expiry() {
        let err =
            SharedAccessSignatureToken::new("SharedAccessSignature sr=foo&sig=bar").unwrap_err();
        assert!(matches!(err, RelayError::InvalidArgument { .. }));
    }

    #[test]
    fn sas_token_new_parses_valid_token() {
        let future_expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 7200;
        let raw = format!(
            "SharedAccessSignature sr=foo&sig=bar&se={}&skn=key",
            future_expiry
        );
        let token = SharedAccessSignatureToken::new(&raw).unwrap();
        assert_eq!(token.token, raw);
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(future_expiry)
        );
    }

    #[tokio::test]
    async fn sas_token_get_token_returns_stored_token() {
        let future_expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 7200;
        let raw = format!(
            "SharedAccessSignature sr=foo&sig=bar&se={}&skn=key",
            future_expiry
        );
        let sas = SharedAccessSignatureToken::new(&raw).unwrap();
        let result = sas.get_token("anything", Duration::from_secs(60)).await.unwrap();
        assert_eq!(result.token, raw);
    }

    #[tokio::test]
    async fn sas_token_get_token_rejects_expired() {
        let raw = "SharedAccessSignature sr=foo&sig=bar&se=0&skn=key";
        let sas = SharedAccessSignatureToken::new(raw).unwrap();
        let err = sas.get_token("anything", Duration::from_secs(60)).await.unwrap_err();
        assert!(matches!(err, RelayError::AuthorizationFailed(_)));
    }
}
