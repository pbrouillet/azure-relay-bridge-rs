//! Validated startup configuration. Secret-bearing types intentionally omit Debug.
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Parser;
use url::Url;

#[derive(Parser)]
#[command(version, about = "Azure Relay bridge control plane")]
pub struct Args {
    /// Explicit local development mode. Never enabled by environment or fallback.
    #[arg(long)]
    pub dev: bool,
    #[arg(long, env = "LISTEN_ADDR", hide_env_values = true)]
    pub listen_addr: Option<String>,
    #[arg(long, env = "PUBLIC_URL", hide_env_values = true)]
    pub public_url: Option<String>,
    #[arg(long, env = "GITHUB_CLIENT_ID", hide_env_values = true)]
    pub github_client_id: Option<String>,
    #[arg(
        long,
        env = "GITHUB_CLIENT_SECRET",
        hide_env_values = true,
        hide = true
    )]
    pub github_client_secret: Option<String>,
    #[arg(long, env = "COOKIE_KEY", hide_env_values = true, hide = true)]
    pub cookie_key: Option<String>,
    #[arg(long, env = "BOOTSTRAP_ADMIN_IDS", hide_env_values = true)]
    pub bootstrap_admin_ids: Option<String>,
    #[arg(long, env = "STORAGE_ACCOUNT", hide_env_values = true)]
    pub storage_account: Option<String>,
    #[arg(long, env = "AZURE_SUBSCRIPTION_ID", hide_env_values = true)]
    pub subscription_id: Option<String>,
    #[arg(long, env = "RELAY_RESOURCE_GROUP", hide_env_values = true)]
    pub relay_resource_group: Option<String>,
    #[arg(long, env = "ALLOWED_LOCATIONS", hide_env_values = true)]
    pub allowed_locations: Option<String>,
    #[arg(
        long,
        env = "STATIC_DIR",
        default_value = "web/dist",
        hide_env_values = true
    )]
    pub static_dir: PathBuf,
    #[arg(long, env = "AZURE_CLIENT_ID", hide_env_values = true)]
    pub azure_client_id: Option<String>,
}

/// Cloud configuration exists only in production; dev services must be selected explicitly.
pub struct CloudSettings {
    pub github_client_id: String,
    pub github_client_secret: String,
    pub cookie_key: [u8; 32],
    pub bootstrap_admin_ids: BTreeSet<u64>,
    pub storage_account: String,
    pub subscription_id: String,
    pub relay_resource_group: String,
    pub allowed_locations: BTreeSet<String>,
    pub azure_client_id: Option<String>,
}

pub struct Settings {
    pub dev: bool,
    pub listen_addr: SocketAddr,
    pub public_url: Url,
    pub static_dir: PathBuf,
    pub cloud: Option<CloudSettings>,
}

/// Includes setting names only, never rejected values (which may contain secrets).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid or missing setting: {0}")]
pub struct SettingsError(pub &'static str);

fn required(value: Option<String>, name: &'static str) -> Result<String, SettingsError> {
    value
        .filter(|v| !v.trim().is_empty())
        .ok_or(SettingsError(name))
}

impl Settings {
    pub fn from_args(args: Args) -> Result<Self, SettingsError> {
        let listen_addr: SocketAddr = args
            .listen_addr
            .as_deref()
            .unwrap_or(if args.dev {
                "127.0.0.1:8080"
            } else {
                "0.0.0.0:8080"
            })
            .parse()
            .map_err(|_| SettingsError("LISTEN_ADDR"))?;
        if args.dev && !listen_addr.ip().is_loopback() {
            return Err(SettingsError("--dev requires a loopback LISTEN_ADDR"));
        }
        let public_url = match args.public_url {
            Some(value) => Url::parse(&value).map_err(|_| SettingsError("PUBLIC_URL"))?,
            None if args.dev => Url::parse(&format!("http://{listen_addr}"))
                .map_err(|_| SettingsError("PUBLIC_URL"))?,
            None => return Err(SettingsError("PUBLIC_URL")),
        };
        if !public_url.username().is_empty()
            || public_url.password().is_some()
            || public_url.query().is_some()
            || public_url.fragment().is_some()
            || public_url.path() != "/"
            || public_url.host_str().is_none()
            || (!args.dev && public_url.scheme() != "https")
        {
            return Err(SettingsError("PUBLIC_URL"));
        }
        if args.dev {
            let loopback = match public_url.host() {
                Some(url::Host::Domain(host)) => host == "localhost",
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            if !loopback || !matches!(public_url.scheme(), "http" | "https") {
                return Err(SettingsError("--dev requires a loopback PUBLIC_URL"));
            }
        }
        let cloud = if args.dev {
            None
        } else {
            let github_client_id = required(args.github_client_id, "GITHUB_CLIENT_ID")?;
            let github_client_secret = required(args.github_client_secret, "GITHUB_CLIENT_SECRET")?;
            let cookie_key = STANDARD
                .decode(required(args.cookie_key, "COOKIE_KEY")?)
                .map_err(|_| SettingsError("COOKIE_KEY must be base64 encoding 32 bytes"))?
                .try_into()
                .map_err(|_| SettingsError("COOKIE_KEY must be base64 encoding 32 bytes"))?;
            let bootstrap_admin_ids: BTreeSet<u64> =
                required(args.bootstrap_admin_ids, "BOOTSTRAP_ADMIN_IDS")?
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<u64>()
                            .ok()
                            .filter(|id| *id != 0)
                            .ok_or(SettingsError("BOOTSTRAP_ADMIN_IDS"))
                    })
                    .collect::<Result<_, _>>()?;
            let storage_account = required(args.storage_account, "STORAGE_ACCOUNT")?;
            if !(3..=24).contains(&storage_account.len())
                || !storage_account
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            {
                return Err(SettingsError("STORAGE_ACCOUNT"));
            }
            let subscription_id = required(args.subscription_id, "AZURE_SUBSCRIPTION_ID")?;
            uuid::Uuid::parse_str(&subscription_id)
                .map_err(|_| SettingsError("AZURE_SUBSCRIPTION_ID"))?;
            let relay_resource_group = required(args.relay_resource_group, "RELAY_RESOURCE_GROUP")?;
            if relay_resource_group.len() > 90
                || relay_resource_group.ends_with('.')
                || !relay_resource_group
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '(' | ')'))
            {
                return Err(SettingsError("RELAY_RESOURCE_GROUP"));
            }
            let allowed_locations = required(args.allowed_locations, "ALLOWED_LOCATIONS")?
                .split(',')
                .map(|s| {
                    let s = s.trim();
                    if s.is_empty()
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                    {
                        Err(SettingsError("ALLOWED_LOCATIONS"))
                    } else {
                        Ok(s.to_owned())
                    }
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            if let Some(id) = &args.azure_client_id {
                uuid::Uuid::parse_str(id).map_err(|_| SettingsError("AZURE_CLIENT_ID"))?;
            }
            Some(CloudSettings {
                github_client_id,
                github_client_secret,
                cookie_key,
                bootstrap_admin_ids,
                storage_account,
                subscription_id,
                relay_resource_group,
                allowed_locations,
                azure_client_id: args.azure_client_id,
            })
        };
        Ok(Self {
            dev: args.dev,
            listen_addr,
            public_url,
            static_dir: args.static_dir,
            cloud,
        })
    }

    /// Production must ship a built SPA. Dev remains usable for API work without a build.
    pub fn validate_static_dir(&self) -> Result<(), SettingsError> {
        if !self.dev && !self.static_dir.join("index.html").is_file() {
            return Err(SettingsError("STATIC_DIR must contain index.html"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn args(dev: bool) -> Args {
        Args {
            dev,
            listen_addr: None,
            public_url: Some(
                if dev {
                    "http://localhost:8080"
                } else {
                    "https://broker.example.com"
                }
                .into(),
            ),
            github_client_id: Some("Iv1.test".into()),
            github_client_secret: Some("secret".into()),
            cookie_key: Some(STANDARD.encode([7; 32])),
            bootstrap_admin_ids: Some("1, 42".into()),
            storage_account: Some("broker123".into()),
            subscription_id: Some("12345678-1234-1234-1234-123456789012".into()),
            relay_resource_group: Some("relay-rg".into()),
            allowed_locations: Some("eastus,westus2".into()),
            static_dir: PathBuf::from("nonexistent-broker-test-assets"),
            azure_client_id: None,
        }
    }

    #[test]
    fn production_validates_all_settings() {
        let settings = Settings::from_args(args(false)).unwrap();
        assert!(!settings.dev);
        let cloud = settings.cloud.as_ref().unwrap();
        assert_eq!(cloud.cookie_key, [7; 32]);
        assert!(cloud.bootstrap_admin_ids.contains(&42));
        assert_eq!(cloud.allowed_locations.len(), 2);
        assert!(settings.validate_static_dir().is_err());
    }

    #[test]
    fn dev_is_explicit_loopback_and_needs_no_cloud_secrets() {
        let mut input = args(true);
        input.public_url = None;
        input.cookie_key = None;
        let settings = Settings::from_args(input).unwrap();
        assert!(settings.listen_addr.ip().is_loopback());
        assert_eq!(settings.public_url.as_str(), "http://127.0.0.1:8080/");
        assert!(settings.cloud.is_none());
        assert!(settings.validate_static_dir().is_ok());
    }

    #[test]
    fn dev_rejects_public_bind_and_public_url() {
        for bind in ["0.0.0.0:8080", "[::]:8080", "192.168.1.1:8080"] {
            let mut input = args(true);
            input.listen_addr = Some(bind.into());
            assert!(Settings::from_args(input).is_err());
        }
        for url in [
            "http://example.com",
            "https://localhost.example.com",
            "ftp://localhost",
        ] {
            let mut input = args(true);
            input.public_url = Some(url.into());
            assert!(Settings::from_args(input).is_err());
        }
        let mut input = args(true);
        input.listen_addr = Some("[::1]:8080".into());
        input.public_url = Some("http://[::1]:8080".into());
        assert!(Settings::from_args(input).is_ok());
    }

    #[test]
    fn production_rejects_invalid_or_missing_fields_without_disclosing_values() {
        type Change = fn(&mut Args);
        let changes: &[Change] = &[
            |a| a.listen_addr = Some("secret-invalid".into()),
            |a| a.public_url = None,
            |a| a.public_url = Some("http://example.com".into()),
            |a| a.public_url = Some("https://secret:secret@example.com".into()),
            |a| a.public_url = Some("https://example.com?secret".into()),
            |a| a.public_url = Some("https://example.com/path".into()),
            |a| a.github_client_id = None,
            |a| a.github_client_secret = Some(" ".into()),
            |a| a.cookie_key = Some("secret".into()),
            |a| a.cookie_key = Some(STANDARD.encode([1; 31])),
            |a| a.bootstrap_admin_ids = Some("0".into()),
            |a| a.bootstrap_admin_ids = Some("secret".into()),
            |a| a.storage_account = Some("BadAccount".into()),
            |a| a.subscription_id = Some("secret".into()),
            |a| a.relay_resource_group = Some("bad/group".into()),
            |a| a.allowed_locations = Some("eastus,".into()),
            |a| a.azure_client_id = Some("secret".into()),
        ];
        for change in changes {
            let mut input = args(false);
            change(&mut input);
            let error = Settings::from_args(input).err().expect("must reject");
            assert!(!error.to_string().contains("secret"));
        }
    }
}
