use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;

use crate::protocol::RequestCommand;

/// An HTTP request received from an Azure Relay Hybrid Connection.
#[derive(Debug, Clone)]
pub struct RelayedHttpListenerRequest {
    /// The HTTP method (GET, POST, etc.).
    method: String,
    /// The request URL path and query string.
    url: String,
    /// The HTTP headers.
    headers: HashMap<String, String>,
    /// Whether the request has a body.
    has_body: bool,
    /// The request body (if present and delivered inline).
    body: Option<Bytes>,
    /// The sender's remote address.
    remote_address: Option<String>,
    /// The sender's remote port.
    remote_port: Option<u16>,
}

impl RelayedHttpListenerRequest {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    pub fn has_body(&self) -> bool {
        self.has_body
    }

    pub fn body(&self) -> Option<&Bytes> {
        self.body.as_ref()
    }

    pub fn remote_address(&self) -> Option<&str> {
        self.remote_address.as_deref()
    }

    pub fn remote_port(&self) -> Option<u16> {
        self.remote_port
    }

    /// Creates a request from a protocol `RequestCommand`.
    pub(crate) fn from_command(cmd: &RequestCommand, body: Option<Bytes>) -> Self {
        let (remote_address, remote_port) = cmd
            .remote_endpoint
            .as_ref()
            .map(|ep| (Some(ep.address.clone()), Some(ep.port)))
            .unwrap_or((None, None));

        Self {
            method: cmd.method.clone(),
            url: cmd.request_target.clone(),
            headers: cmd.request_headers.clone(),
            has_body: cmd.body,
            body,
            remote_address,
            remote_port,
        }
    }
}

/// An HTTP response to send back through an Azure Relay Hybrid Connection.
#[derive(Debug, Clone)]
pub struct RelayedHttpListenerResponse {
    /// HTTP status code (default 200).
    status_code: u16,
    /// Optional status description.
    status_description: Option<String>,
    /// Response headers.
    headers: HashMap<String, String>,
    /// Response body.
    body: Option<Bytes>,
}

impl RelayedHttpListenerResponse {
    pub fn new() -> Self {
        Self {
            status_code: 200,
            status_description: None,
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    pub fn set_status_code(&mut self, code: u16) -> &mut Self {
        self.status_code = code;
        self
    }

    pub fn status_description(&self) -> Option<&str> {
        self.status_description.as_deref()
    }

    pub fn set_status_description(&mut self, desc: impl Into<String>) -> &mut Self {
        self.status_description = Some(desc.into());
        self
    }

    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    pub fn headers_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.headers
    }

    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn body(&self) -> Option<&Bytes> {
        self.body.as_ref()
    }

    pub fn set_body(&mut self, body: impl Into<Bytes>) -> &mut Self {
        self.body = Some(body.into());
        self
    }

    /// Converts this response into a protocol `ResponseCommand` for the given request ID.
    pub(crate) fn into_command(self, request_id: String) -> crate::protocol::ResponseCommand {
        crate::protocol::ResponseCommand {
            request_id,
            status_code: self.status_code,
            status_description: self.status_description,
            response_headers: self.headers,
            body: self.body.is_some(),
        }
    }
}

impl Default for RelayedHttpListenerResponse {
    fn default() -> Self {
        Self::new()
    }
}

/// An HTTP request/response context for a relayed HTTP interaction.
///
/// This is the Rust equivalent of the C# `RelayedHttpListenerContext`.
#[derive(Debug)]
pub struct RelayedHttpListenerContext {
    /// The incoming HTTP request.
    request: RelayedHttpListenerRequest,
    /// The tracking ID for this interaction.
    tracking_id: String,
    /// The rendezvous address (for large request/response upgrade).
    rendezvous_address: Option<String>,
}

impl RelayedHttpListenerContext {
    pub(crate) fn new(
        request: RelayedHttpListenerRequest,
        tracking_id: String,
        rendezvous_address: Option<String>,
    ) -> Self {
        Self {
            request,
            tracking_id,
            rendezvous_address,
        }
    }

    pub fn request(&self) -> &RelayedHttpListenerRequest {
        &self.request
    }

    pub fn tracking_id(&self) -> &str {
        &self.tracking_id
    }

    pub fn rendezvous_address(&self) -> Option<&str> {
        self.rendezvous_address.as_deref()
    }
}

/// Trait for handling HTTP requests received via Azure Relay.
///
/// Implement this trait and pass it to `HybridConnectionListener::set_request_handler()`
/// to handle HTTP requests.
pub trait RequestHandler: Send + Sync + 'static {
    /// Handle an incoming HTTP request and return a response.
    fn handle_request(
        &self,
        context: RelayedHttpListenerContext,
    ) -> impl std::future::Future<Output = RelayedHttpListenerResponse> + Send;
}

// ---------------------------------------------------------------------------
// Dyn-safe wrapper for RequestHandler (RPITIT traits are not dyn-compatible)
// ---------------------------------------------------------------------------

pub(crate) trait DynRequestHandler: Send + Sync {
    fn handle_request_boxed(
        &self,
        context: RelayedHttpListenerContext,
    ) -> Pin<Box<dyn Future<Output = RelayedHttpListenerResponse> + Send + '_>>;
}

impl<T: RequestHandler> DynRequestHandler for T {
    fn handle_request_boxed(
        &self,
        context: RelayedHttpListenerContext,
    ) -> Pin<Box<dyn Future<Output = RelayedHttpListenerResponse> + Send + '_>> {
        Box::pin(self.handle_request(context))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{RemoteEndpoint, RequestCommand};

    #[test]
    fn request_from_command() {
        let cmd = RequestCommand {
            address: "wss://test".into(),
            id: "req-1".into(),
            request_target: "/api/data?foo=bar".into(),
            method: "POST".into(),
            request_headers: [("Content-Type".into(), "application/json".into())].into(),
            body: true,
            remote_endpoint: Some(RemoteEndpoint {
                address: "10.0.0.1".into(),
                port: 5000,
            }),
        };
        let body = Bytes::from_static(b"hello");
        let req = RelayedHttpListenerRequest::from_command(&cmd, Some(body.clone()));
        assert_eq!(req.method(), "POST");
        assert_eq!(req.url(), "/api/data?foo=bar");
        assert_eq!(
            req.headers().get("Content-Type").unwrap(),
            "application/json"
        );
        assert!(req.has_body());
        assert_eq!(req.body().unwrap(), &body);
        assert_eq!(req.remote_address(), Some("10.0.0.1"));
        assert_eq!(req.remote_port(), Some(5000));
    }

    #[test]
    fn response_default_and_builder() {
        let mut resp = RelayedHttpListenerResponse::new();
        assert_eq!(resp.status_code(), 200);
        assert!(resp.status_description().is_none());

        resp.set_status_code(404)
            .set_status_description("Not Found")
            .set_header("X-Custom", "value")
            .set_body(Bytes::from_static(b"not found"));

        assert_eq!(resp.status_code(), 404);
        assert_eq!(resp.status_description(), Some("Not Found"));
        assert_eq!(resp.headers().get("X-Custom").unwrap(), "value");
        assert_eq!(resp.body().unwrap().as_ref(), b"not found");
    }

    #[test]
    fn response_into_command() {
        let mut resp = RelayedHttpListenerResponse::new();
        resp.set_status_code(201)
            .set_status_description("Created")
            .set_header("Location", "/api/items/1")
            .set_body(Bytes::from_static(b"ok"));

        let cmd = resp.into_command("req-42".into());
        assert_eq!(cmd.request_id, "req-42");
        assert_eq!(cmd.status_code, 201);
        assert_eq!(cmd.status_description.as_deref(), Some("Created"));
        assert_eq!(
            cmd.response_headers.get("Location").unwrap(),
            "/api/items/1"
        );
        assert!(cmd.body);
    }

    #[test]
    fn context_creation() {
        let cmd = RequestCommand {
            address: "wss://rendezvous".into(),
            id: "ctx-1".into(),
            request_target: "/path".into(),
            method: "GET".into(),
            request_headers: HashMap::new(),
            body: false,
            remote_endpoint: None,
        };
        let req = RelayedHttpListenerRequest::from_command(&cmd, None);
        let ctx = RelayedHttpListenerContext::new(
            req,
            "track-123".into(),
            Some("wss://rendezvous".into()),
        );

        assert_eq!(ctx.tracking_id(), "track-123");
        assert_eq!(ctx.request().method(), "GET");
        assert_eq!(ctx.rendezvous_address(), Some("wss://rendezvous"));
    }
}
