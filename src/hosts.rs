//! Hosts-file management for canonical browser origins.
//!
//! Web sign-in (OIDC/OAuth/SAML) redirects to the application's registered origin,
//! normally `https://<host>/` on port 443. A local forward reached as
//! `https://<host>:44305` therefore breaks as soon as the identity provider sends
//! the browser back. Giving every tunnelled hostname its own loopback address lets
//! each listener bind the destination's canonical port instead.
//!
//! Addresses are allocated deterministically in `127.77.0.0/16`; the rule must stay
//! identical to `web/src/relays/loopback.ts` (shared fixture
//! `web/src/relays/fixtures/loopback.json`). This module never rewrites traffic.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::cli::{HostsAction, HostsArgs};
use crate::config::Config;

const PREFIX: &str = "127.77";
const SLOTS: u32 = 65534;

pub fn normalize_hostname(host: &str) -> String {
    let name = host.trim().to_ascii_lowercase();
    name.strip_suffix('.').map(str::to_owned).unwrap_or(name)
}

/// True for DNS names that can be mapped in a hosts file (not IP literals, localhost or wildcards).
pub fn is_aliasable_host(host: &str) -> bool {
    let name = normalize_hostname(host);
    if name.is_empty()
        || name == "localhost"
        || name.len() > 253
        || name.parse::<std::net::IpAddr>().is_ok()
    {
        return false;
    }
    if name.split('.').count() == 4
        && name.split('.').all(|part| {
            !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return false;
    }
    name.split('.').all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && (bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_')
            && bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
    })
}

pub fn fnv1a32(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

fn slot_address(slot: u32) -> String {
    format!("{PREFIX}.{}.{}", slot >> 8, slot & 255)
}

/// Allocate stable loopback aliases; previously applied addresses are kept first.
pub fn allocate<'a>(
    hosts: impl IntoIterator<Item = &'a str>,
    existing: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let names: BTreeSet<String> = hosts
        .into_iter()
        .filter(|host| is_aliasable_host(host))
        .map(normalize_hostname)
        .collect();
    let mut result = BTreeMap::new();
    let mut used = BTreeSet::new();
    for name in &names {
        if let Some(kept) = existing.get(name)
            && kept.starts_with(&format!("{PREFIX}."))
            && !used.contains(kept)
        {
            used.insert(kept.clone());
            result.insert(name.clone(), kept.clone());
        }
    }
    for name in &names {
        if result.contains_key(name) {
            continue;
        }
        let mut slot = 1 + fnv1a32(name) % SLOTS;
        while used.contains(&slot_address(slot)) {
            slot = slot % SLOTS + 1;
        }
        used.insert(slot_address(slot));
        result.insert(name.clone(), slot_address(slot));
    }
    result
}

/// Hostnames of TCP/UDP local listeners that should receive a loopback alias.
pub fn listener_hosts(config: &Config) -> Vec<String> {
    let mut hosts = Vec::new();
    for forward in &config.local_forward {
        let mut forward = forward.clone();
        forward.normalize();
        for binding in &forward.bindings {
            if binding.bind_local_socket.is_none()
                && let Some(address) = &binding.bind_address
                && is_aliasable_host(address)
            {
                hosts.push(address.clone());
            }
        }
    }
    hosts
}

fn begin_marker(tag: &str) -> String {
    format!("# >>> azbridge {tag} >>>")
}
fn end_marker(tag: &str) -> String {
    format!("# <<< azbridge {tag} <<<")
}
fn disabled_marker(tag: &str) -> String {
    format!("#azbridge-disabled[{tag}]: ")
}

pub fn render_block(tag: &str, addresses: &BTreeMap<String, String>, newline: &str) -> String {
    let mut lines = vec![begin_marker(tag)];
    lines.extend(addresses.iter().map(|(name, ip)| format!("{ip} {name}")));
    lines.push(end_marker(tag));
    lines.join(newline) + newline
}

fn validate_tag(tag: &str) -> anyhow::Result<()> {
    if tag.is_empty()
        || tag.len() > 64
        || !tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        bail!("Hosts tag must be 1-64 letters, digits, '-', '_' or '.'");
    }
    Ok(())
}

fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Addresses recorded in this tag's block, so re-applying never moves them.
pub fn existing_entries(text: &str, tag: &str) -> HashMap<String, String> {
    let mut entries = HashMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line == begin_marker(tag) {
            inside = true;
        } else if line == end_marker(tag) {
            inside = false;
        } else if inside {
            let mut parts = line.split_whitespace();
            if let (Some(ip), Some(name)) = (parts.next(), parts.next()) {
                entries.insert(normalize_hostname(name), ip.to_owned());
            }
        }
    }
    entries
}

/// Strip this tag's block and restore lines it disabled.
pub fn remove_block(text: &str, tag: &str) -> String {
    let newline = newline_of(text);
    let disabled = disabled_marker(tag);
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == begin_marker(tag) {
            inside = true;
        } else if trimmed == end_marker(tag) {
            inside = false;
        } else if !inside {
            out.push(line.strip_prefix(&disabled).unwrap_or(line).to_owned());
        }
    }
    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join(newline) + newline
    }
}

/// Active (uncommented) lines outside any azbridge block that map one of `names`.
fn conflicts<'a>(lines: &'a [String], names: &BTreeSet<&str>) -> Vec<(usize, &'a str)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let content = line.split('#').next().unwrap_or("");
            let mut parts = content.split_whitespace();
            parts.next().is_some()
                && parts.any(|name| names.contains(normalize_hostname(name).as_str()))
        })
        .map(|(index, line)| (index, line.as_str()))
        .collect()
}

/// Replace or insert this tag's block. Other entries for the same hostnames are an
/// error unless `replace_existing`, which comments them out for `remove_block`.
pub fn merge_block(
    text: &str,
    tag: &str,
    addresses: &BTreeMap<String, String>,
    replace_existing: bool,
) -> anyhow::Result<String> {
    let newline = newline_of(text);
    let base = remove_block(text, tag);
    let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
    let names: BTreeSet<&str> = addresses.keys().map(String::as_str).collect();
    let found = conflicts(&lines, &names);
    if !found.is_empty() && !replace_existing {
        let mut hosts: BTreeSet<String> = BTreeSet::new();
        for (_, line) in &found {
            for name in line
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .skip(1)
            {
                if names.contains(normalize_hostname(name).as_str()) {
                    hosts.insert(normalize_hostname(name));
                }
            }
        }
        bail!(
            "The hosts file already maps {} of these hostnames ({}). Re-run with --replace-existing to comment those lines out (restored by `azbridge hosts remove`).",
            hosts.len(),
            hosts.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    let indexes: Vec<usize> = found.iter().map(|(index, _)| *index).collect();
    for index in indexes {
        lines[index] = format!("{}{}", disabled_marker(tag), lines[index]);
    }
    if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
    let mut result = lines.join(newline);
    if !result.is_empty() && !result.ends_with(newline) {
        result.push_str(newline);
    }
    result.push_str(&render_block(tag, addresses, newline));
    Ok(result)
}

pub fn default_hosts_file() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

fn elevation_hint() -> &'static str {
    if cfg!(windows) {
        "Run this command from an elevated (Run as administrator) terminal."
    } else {
        "Run this command with sudo."
    }
}

fn line_diff(before: &str, after: &str) -> String {
    let old: BTreeSet<&str> = before.lines().collect();
    let new: BTreeSet<&str> = after.lines().collect();
    let mut out = String::new();
    for line in before.lines().filter(|line| !new.contains(line)) {
        out.push_str(&format!("- {line}\n"));
    }
    for line in after.lines().filter(|line| !old.contains(line)) {
        out.push_str(&format!("+ {line}\n"));
    }
    if out.is_empty() {
        "(no changes)\n".into()
    } else {
        out
    }
}

fn write_hosts(path: &Path, before: &str, after: &str, block: &str) -> anyhow::Result<()> {
    if before == after {
        println!("Hosts file already up to date: {}", path.display());
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}.azbridge.bak", path.display()));
    let result = (|| -> std::io::Result<()> {
        if !backup.exists() {
            std::fs::copy(path, &backup)?;
        }
        // Rewrite in place so existing ACLs and ownership are preserved.
        std::fs::write(path, after)
    })();
    match result {
        Ok(()) => {
            println!(
                "Updated {} (first backup: {})",
                path.display(),
                backup.display()
            );
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("{block}");
            bail!(
                "Permission denied writing {}. {} The block above can also be added manually.",
                path.display(),
                elevation_hint()
            )
        }
        Err(error) => Err(error).with_context(|| format!("Failed to update {}", path.display())),
    }
}

fn run_quiet(program: &str, args: &[&str]) {
    match std::process::Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "warning: `{program} {}` exited with {}",
            args.join(" "),
            output.status
        ),
        Err(error) => eprintln!("warning: could not run `{program}`: {error}"),
    }
}

fn flush_dns() {
    if cfg!(windows) {
        run_quiet("ipconfig", &["/flushdns"]);
    } else if cfg!(target_os = "macos") {
        run_quiet("dscacheutil", &["-flushcache"]);
        run_quiet("killall", &["-HUP", "mDNSResponder"]);
    }
}

/// macOS only routes 127.0.0.1 on lo0 by default; other loopback addresses need aliases.
fn macos_aliases(addresses: impl IntoIterator<Item = String>, add: bool) {
    if !cfg!(target_os = "macos") {
        return;
    }
    for ip in addresses {
        if add {
            run_quiet("ifconfig", &["lo0", "alias", &ip, "up"]);
        } else {
            run_quiet("ifconfig", &["lo0", "-alias", &ip]);
        }
    }
    if add {
        eprintln!(
            "note: macOS loopback aliases do not survive a reboot; re-run `sudo azbridge hosts apply` after restarting."
        );
    }
}

async fn load_config(args: &HostsArgs) -> anyhow::Result<Config> {
    let mut argv = vec!["azbridge".to_owned()];
    if let Some(file) = &args.config_file {
        argv.push("-f".into());
        argv.push(file.display().to_string());
    }
    let cli = <crate::cli::Cli as clap::Parser>::try_parse_from(argv)?;
    Ok(crate::broker::bootstrap(&cli, args.machine).await?.config)
}

fn default_tag(args: &HostsArgs) -> String {
    args.tag.clone().unwrap_or_else(|| {
        args.config_file
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| {
                stem.to_string_lossy()
                    .trim_start_matches('_')
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
                    .take(64)
                    .collect::<String>()
            })
            .filter(|tag| !tag.is_empty())
            .unwrap_or_else(|| "default".into())
    })
}

pub async fn run(action: &HostsAction) -> anyhow::Result<()> {
    let args = action.args();
    let tag = default_tag(args);
    validate_tag(&tag)?;
    let path = args.hosts_file.clone().unwrap_or_else(default_hosts_file);
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !matches!(action, HostsAction::Remove { .. }) =>
        {
            String::new()
        }
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()));
        }
    };
    let newline = newline_of(&current);
    match action {
        HostsAction::Remove { dry_run, .. } => {
            let removed: Vec<String> = existing_entries(&current, &tag).into_values().collect();
            let after = remove_block(&current, &tag);
            if *dry_run {
                print!("{}", line_diff(&current, &after));
                return Ok(());
            }
            write_hosts(&path, &current, &after, "")?;
            macos_aliases(removed, false);
            flush_dns();
            Ok(())
        }
        HostsAction::Print { .. } | HostsAction::Apply { .. } => {
            let config = load_config(args).await?;
            let hosts = listener_hosts(&config);
            if hosts.is_empty() {
                bail!(
                    "No local listener binds a DNS hostname; nothing to map. Use BindAddress: <hostname> for browser-facing listeners."
                );
            }
            let addresses = allocate(
                hosts.iter().map(String::as_str),
                &existing_entries(&current, &tag),
            );
            let block = render_block(&tag, &addresses, newline);
            let HostsAction::Apply {
                dry_run,
                replace_existing,
                ..
            } = action
            else {
                print!("{block}");
                return Ok(());
            };
            let after = merge_block(&current, &tag, &addresses, *replace_existing)?;
            if *dry_run {
                print!("{}", line_diff(&current, &after));
                return Ok(());
            }
            write_hosts(&path, &current, &after, &block)?;
            macos_aliases(addresses.values().cloned(), true);
            flush_dns();
            println!(
                "Mapped {} hostname(s). Bind each listener on its destination's port (usually 443) so browser origins and sign-in redirects match production.",
                addresses.len()
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../web/src/relays/fixtures/loopback.json")).unwrap()
    }
    fn map(value: &Value) -> BTreeMap<String, String> {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect()
    }

    #[test]
    fn allocation_matches_browser_parity_fixture() {
        let fixture = fixture();
        let hosts: Vec<&str> = fixture["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            allocate(hosts.iter().copied(), &HashMap::new()),
            map(&fixture["expected"])
        );
        let existing = map(&fixture["existing"]).into_iter().collect();
        assert_eq!(
            allocate(hosts.iter().copied(), &existing),
            map(&fixture["expectedWithExisting"])
        );
        for (text, hash) in fixture["fnv"].as_object().unwrap() {
            assert_eq!(u64::from(fnv1a32(text)), hash.as_u64().unwrap());
        }
        let collision = fixture["collision"].as_array().unwrap();
        let a = collision[0].as_str().unwrap();
        let b = collision[1].as_str().unwrap();
        assert_eq!(fnv1a32(a) % SLOTS, fnv1a32(b) % SLOTS);
    }

    #[test]
    fn aliasable_hosts_exclude_ips_localhost_and_invalid_names() {
        for host in [
            "svc.example.test",
            "App.Example.Test.",
            "single",
            "_srv.example.test",
        ] {
            assert!(is_aliasable_host(host), "{host}");
        }
        for host in [
            "",
            "localhost",
            "127.0.0.1",
            "::1",
            "0.0.0.0",
            "a b",
            "bad..name",
            "-x.example",
            "*.example.test",
        ] {
            assert!(!is_aliasable_host(host), "{host}");
        }
    }

    #[test]
    fn addresses_stay_in_range_and_never_use_default_loopback() {
        let names: Vec<String> = (0..2000).map(|i| format!("h{i}.example.test")).collect();
        let addresses = allocate(names.iter().map(String::as_str), &HashMap::new());
        let unique: BTreeSet<&String> = addresses.values().collect();
        assert_eq!(unique.len(), 2000);
        for ip in addresses.values() {
            let octets: Vec<u8> = ip.split('.').map(|part| part.parse().unwrap()).collect();
            assert_eq!(&octets[..2], &[127, 77]);
            assert!(!(octets[2] == 0 && octets[3] == 0) && !(octets[2] == 255 && octets[3] == 255));
        }
    }

    #[test]
    fn listener_hosts_only_include_hostname_tcp_udp_bindings() {
        let config: Config = serde_yaml::from_str(
            "LocalForward:\n- RelayName: r\n  BindAddress: a.example.test\n  BindPort: 44301\n- RelayName: r\n  BindAddress: 127.0.0.1\n  BindPort: 1\n- RelayName: r\n  Bindings:\n  - BindAddress: b.example.test\n    BindPort: -53\n  - BindLocalSocket: /tmp/s\n  - BindPort: 2\nRemoteForward:\n- RelayName: r\n  Host: c.example.test\n  HostPort: 443\n",
        )
        .unwrap();
        assert_eq!(
            listener_hosts(&config),
            vec!["a.example.test", "b.example.test"]
        );
    }

    fn addresses() -> BTreeMap<String, String> {
        allocate(
            ["svc-01.example.test", "svc-02.example.test"],
            &HashMap::new(),
        )
    }

    #[test]
    fn merge_appends_block_preserving_crlf_and_is_idempotent() {
        let original = "# Copyright\r\n127.0.0.1 localhost\r\n";
        let merged = merge_block(original, "client", &addresses(), false).unwrap();
        assert!(merged.starts_with(original));
        assert!(merged.contains("\r\n# >>> azbridge client >>>\r\n127.77."));
        assert!(!merged.replace("\r\n", "").contains('\n'));
        assert_eq!(
            merge_block(&merged, "client", &addresses(), false).unwrap(),
            merged
        );
        assert_eq!(remove_block(&merged, "client"), original);
        let other = merge_block(
            &merged,
            "server",
            &allocate(["other.example.test"], &HashMap::new()),
            false,
        )
        .unwrap();
        assert!(
            other.contains("# >>> azbridge client >>>")
                && other.contains("# >>> azbridge server >>>")
        );
        assert_eq!(
            remove_block(&remove_block(&other, "server"), "client"),
            original
        );
    }

    #[test]
    fn conflicting_entries_require_opt_in_and_are_restored_on_remove() {
        let original = "127.0.0.1 localhost\n127.0.0.1 SVC-01.example.test # mapped by hand\n# 127.0.0.1 svc-02.example.test\n";
        let error = merge_block(original, "client", &addresses(), false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("1 of these hostnames (svc-01.example.test)"),
            "{error}"
        );
        assert!(error.contains("--replace-existing"));
        let merged = merge_block(original, "client", &addresses(), true).unwrap();
        assert!(merged.contains(
            "#azbridge-disabled[client]: 127.0.0.1 SVC-01.example.test # mapped by hand"
        ));
        assert!(merged.contains("# 127.0.0.1 svc-02.example.test\n"));
        assert_eq!(remove_block(&merged, "client"), original);
    }

    #[test]
    fn existing_block_addresses_are_reused() {
        let text = "# >>> azbridge client >>>\n127.77.1.2 svc-01.example.test\n# <<< azbridge client <<<\n";
        let existing = existing_entries(text, "client");
        assert_eq!(existing["svc-01.example.test"], "127.77.1.2");
        assert_eq!(
            allocate(["svc-01.example.test"], &existing)["svc-01.example.test"],
            "127.77.1.2"
        );
        assert!(existing_entries(text, "other").is_empty());
    }

    #[test]
    fn tags_are_validated_and_dry_run_diff_is_readable() {
        assert!(validate_tag("client-01.v2").is_ok());
        for tag in ["", "has space", "x".repeat(65).as_str(), "a>b"] {
            assert!(validate_tag(tag).is_err());
        }
        assert_eq!(line_diff("a\nb\n", "a\nc\n"), "- b\n+ c\n");
        assert_eq!(line_diff("a\n", "a\n"), "(no changes)\n");
    }

    #[tokio::test]
    async fn apply_and_remove_round_trip_on_a_temporary_hosts_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("_client.yml");
        std::fs::write(&config, "AzureRelayConnectionString: Endpoint=sb://x.example.test/;SharedAccessKeyName=n;SharedAccessKey=k\nLocalForward:\n- RelayName: r\n  BindAddress: svc-01.example.test\n  BindPort: 443\n").unwrap();
        let hosts = dir.path().join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
        let args = HostsArgs {
            config_file: Some(config),
            tag: None,
            hosts_file: Some(hosts.clone()),
            machine: false,
        };
        // Avoid running the platform DNS flush or alias commands in unit tests.
        let addresses = allocate(
            listener_hosts(&load_config(&args).await.unwrap())
                .iter()
                .map(String::as_str),
            &HashMap::new(),
        );
        assert_eq!(default_tag(&args), "client");
        let before = std::fs::read_to_string(&hosts).unwrap();
        let after = merge_block(&before, "client", &addresses, false).unwrap();
        write_hosts(&hosts, &before, &after, "").unwrap();
        assert!(
            std::fs::read_to_string(&hosts)
                .unwrap()
                .contains("svc-01.example.test")
        );
        assert!(PathBuf::from(format!("{}.azbridge.bak", hosts.display())).exists());
        let current = std::fs::read_to_string(&hosts).unwrap();
        write_hosts(&hosts, &current, &remove_block(&current, "client"), "").unwrap();
        assert_eq!(std::fs::read_to_string(&hosts).unwrap(), before);
    }
}
