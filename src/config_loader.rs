use std::path::{Path, PathBuf};
use crate::config::{Config, ConfigError};
use crate::cli::Cli;

/// Platform-specific default config paths.
pub fn machine_config_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/azbridge/azbridge_config.machine.yml"))
    }
    #[cfg(windows)]
    {
        std::env::var("PROGRAMDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("Microsoft").join("AzureBridge").join("azbridge_config.machine.yml"))
    }
}

pub fn user_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        #[cfg(unix)]
        {
            h.join(".azbridge").join("azbridge_config.yml")
        }
        #[cfg(windows)]
        {
            std::env::var("APPDATA")
                .map(|p| PathBuf::from(p).join("azbridge").join("azbridge_config.yml"))
                .unwrap_or_else(|_| h.join(".azbridge").join("azbridge_config.yml"))
        }
    })
}

/// Load a config file, returning None if the file doesn't exist.
fn load_config_file(path: &Path) -> Result<Option<Config>, ConfigError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_yaml::from_str(&content)?;
    Ok(Some(config))
}

/// Load the full configuration by merging layers:
/// 1. Machine config
/// 2. User config (or -f override file)
/// 3. -o inline YAML overrides
/// 4. CLI flag overrides
pub fn load_config(cli: &Cli) -> Result<Config, ConfigError> {
    let mut config = Config::default();

    // 1. Machine config
    if let Some(path) = machine_config_path()
        && let Some(machine_config) = load_config_file(&path)? {
            config.merge(&machine_config);
        }

    // 2. User config or -f override
    if let Some(ref config_file) = cli.config_file {
        if let Some(file_config) = load_config_file(config_file)? {
            config.merge(&file_config);
        }
    } else if let Some(path) = user_config_path()
        && let Some(user_config) = load_config_file(&path)? {
            config.merge(&user_config);
        }

    // 3. -o inline YAML overrides
    for opt in &cli.option {
        // Format: "Key:Value" -> YAML "Key: Value"
        let yaml = opt.replacen(':', ": ", 1);
        let override_config: Config = serde_yaml::from_str(&yaml)
            .map_err(|e| ConfigError::Validation {
                file: "<-o option>".into(),
                message: format!("invalid option '{}': {}", opt, e),
            })?;
        config.merge(&override_config);
    }

    // 4. CLI flag overrides
    apply_cli_overrides(&mut config, cli);

    // 5. Environment variable fallback for connection string
    if config.azure_relay_connection_string.is_none() {
        if let Ok(cs) = std::env::var("AZURE_BRIDGE_CONNECTIONSTRING") {
            if !cs.is_empty() {
                config.azure_relay_connection_string = Some(cs);
            }
        }
    }

    // 6. Normalize convenience fields into bindings
    for lf in &mut config.local_forward {
        lf.normalize();
    }
    for rf in &mut config.remote_forward {
        rf.normalize();
    }

    Ok(config)
}

/// Apply CLI flags onto the config.
fn apply_cli_overrides(config: &mut Config, cli: &Cli) {
    if let Some(ref endpoint) = cli.endpoint_uri {
        config.azure_relay_endpoint = Some(endpoint.clone());
    }
    if let Some(ref cs) = cli.connection_string {
        config.azure_relay_connection_string = Some(cs.clone());
    }
    if let Some(ref key_name) = cli.shared_access_key_name {
        config.azure_relay_shared_access_key_name = Some(key_name.clone());
    }
    if let Some(ref key) = cli.shared_access_key {
        config.azure_relay_shared_access_key = Some(key.clone());
    }
    if let Some(ref sig) = cli.signature {
        config.azure_relay_shared_access_signature = Some(sig.clone());
    }
    if let Some(ref bind) = cli.bind_address {
        config.bind_address = Some(bind.clone());
    }
    if cli.gateway_ports {
        config.gateway_ports = Some(true);
    }
    if cli.quiet {
        config.log_level = Some("QUIET".to_string());
    } else if cli.verbose >= 3 {
        config.log_level = Some("DEBUG3".to_string());
    } else if cli.verbose >= 2 {
        config.log_level = Some("DEBUG".to_string());
    } else if cli.verbose >= 1 {
        config.log_level = Some("VERBOSE".to_string());
    }
    if let Some(ref log_file) = cli.log_file {
        config.log_file_name = Some(log_file.to_string_lossy().into_owned());
    }
    if let Some(ka) = cli.keep_alive_interval {
        config.keep_alive_interval = Some(ka);
    }

    // Parse -L expressions
    use crate::config::parse_local_forward;
    for expr in &cli.local_forward {
        if let Ok(fwd) = parse_local_forward(expr) {
            config.local_forward.push(fwd);
        }
    }

    // Parse -T expressions
    use crate::config::parse_remote_forward;
    for expr in &cli.remote_forward {
        if let Ok(fwd) = parse_remote_forward(expr) {
            config.remote_forward.push(fwd);
        }
    }

    // Parse -R (legacy) expressions
    use crate::config::parse_remote_forward_legacy;
    for expr in &cli.remote_forward_legacy {
        if let Ok(fwd) = parse_remote_forward_legacy(expr) {
            config.remote_forward.push(fwd);
        }
    }

    // Parse -H expressions
    use crate::config::parse_remote_http_forward;
    for expr in &cli.remote_http_forward {
        if let Ok(fwd) = parse_remote_http_forward(expr) {
            config.remote_forward.push(fwd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serial_test::serial;

    #[test]
    fn machine_config_path_returns_something() {
        let path = machine_config_path();
        assert!(path.is_some());
    }

    #[test]
    fn user_config_path_returns_something() {
        let path = user_config_path();
        assert!(path.is_some());
    }

    #[test]
    fn load_nonexistent_config_returns_none() {
        let result = load_config_file(Path::new("nonexistent_path_config.yml")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn apply_cli_overrides_sets_endpoint() {
        let cli = Cli::try_parse_from([
            "azbridge", "-e", "sb://test.servicebus.windows.net",
        ]).unwrap();
        let mut config = Config::default();
        apply_cli_overrides(&mut config, &cli);
        assert_eq!(config.azure_relay_endpoint.as_deref(), Some("sb://test.servicebus.windows.net"));
    }

    #[test]
    fn apply_cli_overrides_quiet_sets_log_level() {
        let cli = Cli::try_parse_from(["azbridge", "-q", "-L", "8080:r"]).unwrap();
        let mut config = Config::default();
        apply_cli_overrides(&mut config, &cli);
        assert_eq!(config.log_level.as_deref(), Some("QUIET"));
    }

    #[test]
    fn apply_cli_overrides_verbose_sets_log_level() {
        let cli = Cli::try_parse_from(["azbridge", "-v", "-L", "8080:r"]).unwrap();
        let mut config = Config::default();
        apply_cli_overrides(&mut config, &cli);
        assert_eq!(config.log_level.as_deref(), Some("VERBOSE"));
    }

    #[test]
    #[serial]
    fn env_var_connection_string_fallback() {
        let val = "Endpoint=sb://test-env.servicebus.windows.net/;SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey=TESTKEY123=";
        unsafe { std::env::set_var("AZURE_BRIDGE_CONNECTIONSTRING", val) };
        let cli = Cli::try_parse_from(["azbridge", "-L", "8080:relay"]).unwrap();
        let config = load_config(&cli).unwrap();
        assert_eq!(config.azure_relay_connection_string.as_deref(), Some(val));
        unsafe { std::env::remove_var("AZURE_BRIDGE_CONNECTIONSTRING") };
    }

    #[test]
    #[serial]
    fn cli_overrides_env_var_connection_string() {
        let env_val = "Endpoint=sb://env.servicebus.windows.net/;SharedAccessKeyName=key;SharedAccessKey=ENV=";
        let cli_val = "Endpoint=sb://cli.servicebus.windows.net/;SharedAccessKeyName=key;SharedAccessKey=CLI=";
        unsafe { std::env::set_var("AZURE_BRIDGE_CONNECTIONSTRING", env_val) };
        let cli = Cli::try_parse_from(["azbridge", "-x", cli_val, "-L", "8080:relay"]).unwrap();
        let config = load_config(&cli).unwrap();
        assert_eq!(config.azure_relay_connection_string.as_deref(), Some(cli_val));
        unsafe { std::env::remove_var("AZURE_BRIDGE_CONNECTIONSTRING") };
    }

    #[test]
    fn load_config_normalizes_flat_local_forward() {
        use std::io::Write;
        let yaml = "\
LocalForward:
  - RelayName: test
    BindPort: 8080
    BindAddress: \"127.0.0.1\"
";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(yaml.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let cli = Cli::try_parse_from([
            "azbridge",
            "-f",
            tmp.path().to_str().unwrap(),
        ])
        .unwrap();

        let config = load_config(&cli).unwrap();
        assert_eq!(config.local_forward.len(), 1);
        assert_eq!(config.local_forward[0].bindings.len(), 1);
        assert_eq!(config.local_forward[0].bindings[0].bind_port, 8080);
        assert_eq!(
            config.local_forward[0].bindings[0].bind_address.as_deref(),
            Some("127.0.0.1")
        );
    }

    #[test]
    fn config_merge_precedence() {
        use std::io::Write;
        // Create a temp config file with some values
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test_config.yml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "AzureRelayEndpoint: sb://from-file.servicebus.windows.net").unwrap();
        writeln!(f, "LogLevel: ERROR").unwrap();
        writeln!(f, "ConnectTimeout: 30").unwrap();
        writeln!(f, "GatewayPorts: true").unwrap();
        drop(f);

        // CLI: -f config_file -o "ConnectTimeout:45" -e sb://from-cli.servicebus.windows.net -v
        let cli = Cli::try_parse_from([
            "azbridge",
            "-f", config_path.to_str().unwrap(),
            "-o", "ConnectTimeout:45",
            "-e", "sb://from-cli.servicebus.windows.net",
            "-v",
            "-L", "8080:test",
        ]).unwrap();

        let config = load_config(&cli).unwrap();

        // CLI -e overrides file's AzureRelayEndpoint
        assert_eq!(config.azure_relay_endpoint.as_deref(), Some("sb://from-cli.servicebus.windows.net"));
        // -o override beats file's ConnectTimeout (45 > 30)
        assert_eq!(config.connect_timeout, Some(45));
        // File's GatewayPorts survives (not overridden by CLI or -o)
        assert_eq!(config.gateway_ports, Some(true));
        // CLI -v overrides file's LogLevel
        assert_eq!(config.log_level.as_deref(), Some("VERBOSE"));
    }

    #[test]
    fn config_file_overrides_defaults() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("test_config2.yml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "ConnectionAttempts: 5").unwrap();
        writeln!(f, "ExitOnForwardFailure: true").unwrap();
        drop(f);

        let cli = Cli::try_parse_from([
            "azbridge",
            "-f", config_path.to_str().unwrap(),
            "-L", "8080:test",
        ]).unwrap();

        let config = load_config(&cli).unwrap();
        assert_eq!(config.connection_attempts, Some(5));
        assert_eq!(config.exit_on_forward_failure, Some(true));
    }
}
