use azure_relay::http::{
    RelayedHttpListenerContext, RelayedHttpListenerResponse, RequestHandler,
};

use crate::http_forward::{filter_forward_headers, strip_relay_prefix, HttpForwardConfig};

/// Forwards HTTP requests received via Azure Relay to a local HTTP target.
pub struct HttpRemoteForwarder {
    config: HttpForwardConfig,
    relay_name: String,
    port_name: String,
    client: reqwest::Client,
}

impl HttpRemoteForwarder {
    pub fn new(
        config: HttpForwardConfig,
        relay_name: String,
        port_name: String,
    ) -> anyhow::Result<Self> {
        let client = reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(config.insecure)
            .no_proxy()
            .build()?;
        Ok(Self {
            config,
            relay_name,
            port_name,
            client,
        })
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }
}

impl RequestHandler for HttpRemoteForwarder {
    async fn handle_request(
        &self,
        context: RelayedHttpListenerContext,
    ) -> RelayedHttpListenerResponse {
        let request = context.request();

        // Strip relay prefix from path and build target URL
        let relative_path = strip_relay_prefix(request.url(), &self.relay_name);
        let base_url = self.config.base_url();
        let target_url = if relative_path.starts_with('/') {
            format!("{}{}", base_url.trim_end_matches('/'), relative_path)
        } else {
            format!("{}/{}", base_url.trim_end_matches('/'), relative_path)
        };

        // Build the reqwest request
        let method: reqwest::Method = request.method().parse().unwrap_or(reqwest::Method::GET);
        let mut req_builder = self.client.request(method, &target_url);

        // Forward headers (filtering out internal ones)
        let filtered = filter_forward_headers(request.headers());
        for (key, value) in &filtered {
            req_builder = req_builder.header(key, value);
        }

        // Forward body if present
        if let Some(body) = request.body() {
            req_builder = req_builder.body(body.clone());
        }

        // Send and build response
        match req_builder.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let mut response = RelayedHttpListenerResponse::new();
                response.set_status_code(status);

                // Copy response headers
                for (name, value) in resp.headers() {
                    if let Ok(v) = value.to_str() {
                        // Skip hop-by-hop headers
                        let name_lower = name.as_str().to_lowercase();
                        if name_lower != "transfer-encoding" {
                            response.set_header(name.as_str(), v);
                        }
                    }
                }

                // Read body
                match resp.bytes().await {
                    Ok(body) if !body.is_empty() => {
                        response.set_body(body);
                    }
                    _ => {}
                }

                response
            }
            Err(e) => {
                tracing::warn!(error = %e, target = %target_url, "HTTP forward request failed");
                let mut response = RelayedHttpListenerResponse::new();
                response.set_status_code(502);
                response.set_status_description(format!("Bad Gateway: {}", e));
                response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_remote_forwarder_new() {
        let config = HttpForwardConfig {
            host: "localhost".into(),
            port: 8080,
            https: false,
            insecure: false,
            path_prefix: None,
        };
        let fwd = HttpRemoteForwarder::new(config, "relay1".into(), "http".into()).unwrap();
        assert_eq!(fwd.port_name(), "http");
    }

    #[test]
    fn http_remote_forwarder_insecure() {
        let config = HttpForwardConfig {
            host: "localhost".into(),
            port: 443,
            https: true,
            insecure: true,
            path_prefix: Some("/api".into()),
        };
        let fwd = HttpRemoteForwarder::new(config, "relay1".into(), "https".into()).unwrap();
        assert_eq!(fwd.port_name(), "https");
    }
}
