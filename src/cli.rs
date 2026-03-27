use std::path::PathBuf;
use clap::Parser;

/// Azure Relay Bridge - Create TCP/UDP/HTTP tunnels via Azure Relay.
#[derive(Parser, Debug)]
#[command(name = "azbridge", version, about)]
pub struct Cli {
    /// Configuration file to use.
    #[arg(short = 'f', long = "config-file")]
    pub config_file: Option<PathBuf>,

    /// Relay endpoint base URI (sb://{namespace}.servicebus.windows.net/).
    #[arg(short = 'e', long = "endpoint-uri")]
    pub endpoint_uri: Option<String>,

    /// Azure Relay connection string.
    #[arg(short = 'x', long = "connection-string")]
    pub connection_string: Option<String>,

    /// Shared access policy name.
    #[arg(short = 'K', long = "shared-access-key-name")]
    pub shared_access_key_name: Option<String>,

    /// Shared access policy key.
    #[arg(short = 'k', long = "shared-access-key")]
    pub shared_access_key: Option<String>,

    /// Shared access signature token.
    #[arg(short = 's', long = "signature")]
    pub signature: Option<String>,

    /// Local forward expressions: [bind_address:]port[/port_name]:relay_name
    #[arg(short = 'L', long = "local-forward")]
    pub local_forward: Vec<String>,

    /// Remote forward expressions: relay_name:[port_name/][host:]port
    #[arg(short = 'T', long = "remote-forward")]
    pub remote_forward: Vec<String>,

    /// Legacy remote forward expressions (hidden, same as -T with different host:port order).
    #[arg(short = 'R', hide = true)]
    pub remote_forward_legacy: Vec<String>,

    /// Remote HTTP forward: relay_name:{http|https}/[host][/path]:port
    #[arg(short = 'H', long = "remote-http-forward")]
    pub remote_http_forward: Vec<String>,

    /// Log file path (default: console).
    #[arg(short = 'l', long = "log-file")]
    pub log_file: Option<PathBuf>,

    /// No log output to stdout/stderr.
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Verbose log output. Use multiple times for more detail (-vvv).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Allow remote hosts to connect to local listeners.
    #[arg(short = 'g', long = "gateway-ports")]
    pub gateway_ports: bool,

    /// Source address for outbound connections.
    #[arg(short = 'b', long = "bind-address")]
    pub bind_address: Option<String>,

    /// Configuration option override (key:value YAML).
    #[arg(short = 'o', long = "option")]
    pub option: Vec<String>,

    /// Keep-alive interval in seconds.
    #[arg(short = 'a')]
    pub keep_alive_interval: Option<u32>,

    /// Windows only: Install as Windows Service. Must run as admin.
    #[arg(short = 'I', long = "svcinstall")]
    pub svc_install: bool,

    /// Windows only: Uninstall Windows Service. Must run as admin.
    #[arg(short = 'U', long = "svcuninstall")]
    pub svc_uninstall: bool,

    /// Reserved for background service invocation.
    #[arg(long = "svc", hide = true)]
    pub svc: bool,

    /// Launch interactive TUI for config scaffolding, browsing, and running.
    #[arg(long = "tui")]
    pub tui: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_minimal_args() {
        let cli = Cli::try_parse_from(["azbridge", "-L", "8080:myrelay", "-e", "sb://test.servicebus.windows.net"]).unwrap();
        assert_eq!(cli.local_forward, vec!["8080:myrelay"]);
        assert_eq!(cli.endpoint_uri.as_deref(), Some("sb://test.servicebus.windows.net"));
    }

    #[test]
    fn parse_multiple_forwards() {
        let cli = Cli::try_parse_from([
            "azbridge",
            "-L", "8080:relay1",
            "-L", "9090:relay2",
            "-T", "relay3:3000",
            "-e", "sb://test.servicebus.windows.net",
        ]).unwrap();
        assert_eq!(cli.local_forward.len(), 2);
        assert_eq!(cli.remote_forward.len(), 1);
    }

    #[test]
    fn parse_all_flags() {
        let cli = Cli::try_parse_from([
            "azbridge",
            "-e", "sb://ns.servicebus.windows.net",
            "-x", "Endpoint=sb://ns.servicebus.windows.net;SharedAccessKeyName=k;SharedAccessKey=v",
            "-K", "keyname",
            "-k", "keyvalue",
            "-s", "SharedAccessSignature sr=foo",
            "-L", "8080:relay1",
            "-T", "relay2:9090",
            "-H", "relay3:http/localhost:80",
            "-f", "config.yml",
            "-l", "output.log",
            "-q",
            "-v",
            "-g",
            "-b", "0.0.0.0",
            "-o", "KeepAliveInterval:60",
            "-a", "30",
        ]).unwrap();
        assert!(cli.quiet);
        assert_eq!(cli.verbose, 1);
        assert!(cli.gateway_ports);
        assert_eq!(cli.keep_alive_interval, Some(30));
    }

    #[test]
    fn parse_verbose_count() {
        let cli = Cli::try_parse_from(["azbridge", "-vvv", "-L", "8080:r"]).unwrap();
        assert_eq!(cli.verbose, 3);
    }

    #[test]
    fn parse_single_verbose() {
        let cli = Cli::try_parse_from(["azbridge", "-v", "-L", "8080:r"]).unwrap();
        assert_eq!(cli.verbose, 1);
    }

    #[test]
    fn parse_legacy_remote_forward() {
        let cli = Cli::try_parse_from(["azbridge", "-R", "relay:host:8080", "-e", "sb://test.servicebus.windows.net"]).unwrap();
        assert_eq!(cli.remote_forward_legacy, vec!["relay:host:8080"]);
    }
}
