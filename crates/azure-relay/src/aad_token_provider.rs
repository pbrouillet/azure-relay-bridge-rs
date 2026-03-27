//! Azure Active Directory / Entra ID token provider.
//!
//! Uses credentials from `azure_identity` to obtain tokens for Azure Relay
//! authentication, matching the .NET azbridge behavior.
//!
//! When the `azure-identity` feature is enabled and no SAS credentials are
//! present in the connection string, the client/listener will automatically
//! fall back to AAD authentication using [`AadTokenProvider`].

#[cfg(feature = "azure-identity")]
mod inner {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use azure_core::credentials::TokenCredential;
    use azure_identity::DeveloperToolsCredential;

    use crate::error::{RelayError, Result};
    use crate::token_provider::{SecurityToken, TokenProvider};

    /// Indicates whether an AAD token was used (determines which HTTP header to set).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TokenKind {
        /// SAS token — uses `ServiceBusAuthorization` header.
        Sas,
        /// AAD Bearer token — uses standard `Authorization` header.
        Aad,
    }

    /// Token provider using Azure Entra ID credentials (CLI login, managed identity, etc.)
    ///
    /// By default, creates a [`DeveloperToolsCredential`] which tries Azure CLI
    /// and Azure Developer CLI in sequence. For production workloads running
    /// on Azure, use [`AadTokenProvider::from_credential`] with a
    /// [`ManagedIdentityCredential`](azure_identity::ManagedIdentityCredential).
    pub struct AadTokenProvider {
        credential: Arc<dyn TokenCredential>,
    }

    impl std::fmt::Debug for AadTokenProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AadTokenProvider")
                .field("credential", &"<dyn TokenCredential>")
                .finish()
        }
    }

    impl AadTokenProvider {
        /// Create a new AAD token provider using `DeveloperToolsCredential`.
        ///
        /// This credential chain tries Azure CLI and Azure Developer CLI, matching
        /// the .NET `DefaultAzureCredential` behavior for local development.
        pub fn new() -> Result<Self> {
            let credential: Arc<dyn TokenCredential> =
                DeveloperToolsCredential::new(None).map_err(|e| {
                    RelayError::communication(format!(
                        "Failed to create DeveloperToolsCredential: {e}"
                    ))
                })?;
            Ok(Self { credential })
        }

        /// Create an AAD token provider from any `TokenCredential` implementation.
        ///
        /// Use this for production scenarios with `ManagedIdentityCredential`,
        /// `ClientSecretCredential`, etc.
        pub fn from_credential(credential: Arc<dyn TokenCredential>) -> Self {
            Self { credential }
        }
    }

    impl TokenProvider for AadTokenProvider {
        async fn get_token(
            &self,
            _audience: &str,
            _valid_for: Duration,
        ) -> Result<SecurityToken> {
            let scope = "https://relay.azure.net/.default";
            let token_response = self
                .credential
                .get_token(&[scope], None)
                .await
                .map_err(|e| {
                    RelayError::AuthorizationFailed(format!(
                        "AAD token acquisition failed: {e}"
                    ))
                })?;

            let token_str = token_response.token.secret().to_string();
            let expires_at = SystemTime::from(token_response.expires_on);

            Ok(SecurityToken {
                token: format!("Bearer {token_str}"),
                expires_at,
            })
        }
    }
}

#[cfg(feature = "azure-identity")]
pub use inner::{AadTokenProvider, TokenKind};

#[cfg(all(test, feature = "azure-identity"))]
mod tests {
    use super::inner::*;

    #[test]
    fn aad_token_provider_can_be_constructed() {
        // DeveloperToolsCredential::new() should succeed even without Azure CLI
        // logged in — it defers actual token acquisition to get_token().
        let provider = AadTokenProvider::new();
        assert!(
            provider.is_ok(),
            "AadTokenProvider::new() failed: {:?}",
            provider.err()
        );
    }

    #[test]
    fn aad_token_provider_debug_impl() {
        let provider = AadTokenProvider::new().unwrap();
        let debug = format!("{:?}", provider);
        assert!(debug.contains("AadTokenProvider"));
    }

    #[test]
    fn token_kind_variants() {
        assert_ne!(TokenKind::Sas, TokenKind::Aad);
        let cloned = TokenKind::Aad;
        assert_eq!(cloned, TokenKind::Aad);
    }

    #[test]
    fn from_credential_accepts_developer_tools() {
        use azure_identity::DeveloperToolsCredential;
        let cred = DeveloperToolsCredential::new(None).unwrap();
        let provider = AadTokenProvider::from_credential(cred);
        assert!(format!("{:?}", provider).contains("AadTokenProvider"));
    }
}
