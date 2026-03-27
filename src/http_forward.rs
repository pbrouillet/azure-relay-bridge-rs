use std::collections::HashMap;

/// HTTP remote forwarder configuration.
#[derive(Debug, Clone)]
pub struct HttpForwardConfig {
    /// Target host (e.g., "localhost").
    pub host: String,
    /// Target port.
    pub port: u16,
    /// Whether target uses HTTPS.
    pub https: bool,
    /// Skip TLS certificate validation.
    pub insecure: bool,
    /// Path prefix to prepend to requests.
    pub path_prefix: Option<String>,
}

impl HttpForwardConfig {
    /// Build the base URL for the target.
    pub fn base_url(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let mut url = format!("{}://{}:{}", scheme, self.host, self.port);
        if let Some(ref prefix) = self.path_prefix {
            if !prefix.starts_with('/') {
                url.push('/');
            }
            url.push_str(prefix);
        }
        url
    }
}

/// Maps relay request headers to forwarded request, stripping relay-internal headers.
///
/// Strips these headers (matching C# behavior):
/// - Host (will be set by the HTTP client)
/// - Content-Length (will be computed from body)
/// - Content-Type (handled separately)
pub fn filter_forward_headers(headers: &HashMap<String, String>) -> HashMap<String, String> {
    let skip = [
        "host",
        "content-length",
        "transfer-encoding",
        "connection",
        "te",
        "trailer",
        "upgrade",
        "close",
    ];
    headers
        .iter()
        .filter(|(k, _)| !skip.contains(&k.to_lowercase().as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Strips the relay name prefix from the request path.
///
/// If the path starts with `/{relay_name}`, strips that prefix.
/// E.g., `/myrelay/api/data` with relay_name "myrelay" becomes `/api/data`.
pub fn strip_relay_prefix(path: &str, relay_name: &str) -> String {
    let prefix = format!("/{}", relay_name);
    if let Some(rest) = path.strip_prefix(&prefix) {
        if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        }
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_http() {
        let config = HttpForwardConfig {
            host: "localhost".into(),
            port: 8080,
            https: false,
            insecure: false,
            path_prefix: None,
        };
        assert_eq!(config.base_url(), "http://localhost:8080");
    }

    #[test]
    fn base_url_https() {
        let config = HttpForwardConfig {
            host: "example.com".into(),
            port: 443,
            https: true,
            insecure: false,
            path_prefix: None,
        };
        assert_eq!(config.base_url(), "https://example.com:443");
    }

    #[test]
    fn base_url_with_path_prefix() {
        let config = HttpForwardConfig {
            host: "localhost".into(),
            port: 8080,
            https: false,
            insecure: false,
            path_prefix: Some("/api/v1".into()),
        };
        assert_eq!(config.base_url(), "http://localhost:8080/api/v1");
    }

    #[test]
    fn base_url_with_path_prefix_no_leading_slash() {
        let config = HttpForwardConfig {
            host: "localhost".into(),
            port: 8080,
            https: false,
            insecure: false,
            path_prefix: Some("api".into()),
        };
        assert_eq!(config.base_url(), "http://localhost:8080/api");
    }

    #[test]
    fn filter_headers_removes_internal() {
        let mut headers = HashMap::new();
        headers.insert("Host".into(), "relay.example.com".into());
        headers.insert("Content-Length".into(), "42".into());
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("X-Custom".into(), "value".into());
        headers.insert("Authorization".into(), "Bearer token".into());

        let filtered = filter_forward_headers(&headers);
        assert!(!filtered.contains_key("Host"));
        assert!(!filtered.contains_key("Content-Length"));
        assert!(filtered.contains_key("Content-Type")); // NOT stripped
        assert!(filtered.contains_key("X-Custom"));
        assert!(filtered.contains_key("Authorization"));
    }

    #[test]
    fn filter_headers_case_insensitive() {
        let mut headers = HashMap::new();
        headers.insert("host".into(), "value".into());
        headers.insert("CONTENT-LENGTH".into(), "0".into());
        headers.insert("Transfer-Encoding".into(), "chunked".into());
        headers.insert("Connection".into(), "keep-alive".into());

        let filtered = filter_forward_headers(&headers);
        assert!(filtered.is_empty());
    }

    #[test]
    fn strip_relay_prefix_removes_prefix() {
        assert_eq!(
            strip_relay_prefix("/myrelay/api/data", "myrelay"),
            "/api/data"
        );
    }

    #[test]
    fn strip_relay_prefix_root_path() {
        assert_eq!(strip_relay_prefix("/myrelay", "myrelay"), "/");
    }

    #[test]
    fn strip_relay_prefix_no_match() {
        assert_eq!(
            strip_relay_prefix("/other/api/data", "myrelay"),
            "/other/api/data"
        );
    }

    #[test]
    fn strip_relay_prefix_empty_path() {
        assert_eq!(strip_relay_prefix("", "myrelay"), "");
    }

    #[test]
    fn strip_relay_prefix_with_query() {
        assert_eq!(
            strip_relay_prefix("/myrelay/api?key=val", "myrelay"),
            "/api?key=val"
        );
    }
}
