//! Broker HTTP composition root for workspace administration and managed gateways.
//!
//! Production requires HTTPS `PUBLIC_URL`, `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`,
//! base64 `COOKIE_KEY` (32 decoded bytes), comma-separated `BOOTSTRAP_ADMIN_IDS`,
//! `STORAGE_ACCOUNT`, `AZURE_SUBSCRIPTION_ID`, `RELAY_RESOURCE_GROUP`, and
//! comma-separated `ALLOWED_LOCATIONS`. `AZURE_CLIENT_ID` optionally selects a UAMI.
//! `LISTEN_ADDR` defaults to `0.0.0.0:8080`; `STATIC_DIR` defaults to `web/dist`
//! and must contain a built `index.html` in production.
//!
//! Only the explicit `--dev` flag enables development; it requires loopback bind
//! and public URL and permits missing SPA assets (503 JSON until built). Dependency
//! failures never switch production into development mode. Development sign-in is
//! disabled in production; Azure-backed storage and Relay management are required.
use {
    azure_core as _, azure_identity as _, azure_relay as _, azure_storage_blob as _, reqwest as _,
    serde as _,
};

pub mod auth;
pub mod error;
pub mod gateways;
pub mod models;
pub mod profiles;
pub mod reconciler;
pub mod relay;
mod relay_map;
pub mod settings;
pub mod storage;
pub mod workspaces;

use std::{sync::Arc, time::Instant};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use azbridge_broker_api::{BrokerMode, HealthResponse};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use error::BrokerError;
use settings::Settings;

#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub auth: Arc<auth::AuthService>,
    pub relay_manager: Arc<dyn relay::RelayManager>,
    pub mutation_lock: Arc<tokio::sync::Mutex<()>>,
    pub metadata_store: Arc<dyn storage::MetadataStore>,
    pub config_store: Arc<dyn storage::ConfigStore>,
}

impl AppState {
    /// Synchronous explicit-development constructor. Production must initialize Azure.
    pub fn new(settings: Settings) -> Result<Self, BrokerError> {
        if !settings.dev {
            return Err(BrokerError::Unavailable);
        }
        Self::with_stores(
            settings,
            Arc::new(storage::InMemoryMetadataStore::new()),
            Arc::new(storage::InMemoryConfigStore::new()),
        )
    }

    pub async fn initialize(settings: Settings) -> Result<Self, BrokerError> {
        let (metadata, configs) = storage::storage_factory(&settings).await?;
        Self::with_stores(settings, metadata, configs)
    }

    pub fn with_stores(
        settings: Settings,
        metadata_store: Arc<dyn storage::MetadataStore>,
        config_store: Arc<dyn storage::ConfigStore>,
    ) -> Result<Self, BrokerError> {
        let auth = Arc::new(auth::AuthService::from_settings(&settings)?);
        let relay_manager: Arc<dyn relay::RelayManager> = if settings.dev {
            Arc::new(relay::FakeRelayManager::new())
        } else {
            Arc::new(relay::ArmRelayManager::from_cloud(
                settings.cloud.as_ref().ok_or(BrokerError::Unavailable)?,
            )?)
        };
        Ok(Self {
            settings: Arc::new(settings),
            auth,
            relay_manager,
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            metadata_store,
            config_store,
        })
    }
}

/// Compose feature routes here before binding the shared state.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(health))
        .merge(auth::routes())
        .merge(workspaces::routes())
        .merge(profiles::routes())
        .merge(gateways::routes())
        .fallback(spa)
        .method_not_allowed_fallback(|| async { BrokerError::MethodNotAllowed })
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(request_middleware))
}

pub fn app(state: AppState) -> Router {
    routes().with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        mode: if state.settings.dev {
            BrokerMode::Development
        } else {
            BrokerMode::Production
        },
    })
}

async fn request_middleware(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    // Never trace query strings, headers, or bodies (OAuth codes and bearer tokens).
    let path = log_path(request.uri().path()).to_owned();
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    tracing::info!(%method, %path, status = response.status().as_u16(), elapsed_ms = started.elapsed().as_millis(), "http request");
    response
}

fn log_path(path: &str) -> &str {
    if path.starts_with("/api/v1/enrollments/") {
        "/api/v1/enrollments/:id"
    } else {
        path
    }
}

async fn spa(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path();
    if ["/api", "/auth"]
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    {
        return BrokerError::NotFound.into_response();
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return BrokerError::MethodNotAllowed.into_response();
    }
    let index = state.settings.static_dir.join("index.html");
    if !index.is_file() {
        // Explicit dev mode supports backend-only development; never fake a UI/API success.
        return BrokerError::Unavailable.into_response();
    }
    let service = ServeDir::new(&state.settings.static_dir).fallback(ServeFile::new(index));
    match service.oneshot(request).await {
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            BrokerError::NotFound.into_response()
        }
        Ok(response) if response.status().is_server_error() => {
            BrokerError::Internal(anyhow::anyhow!("static service failed")).into_response()
        }
        Ok(response) => response.map(Body::new),
        Err(error) => match error {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn enrollment_capabilities_are_not_written_to_request_logs() {
        assert_eq!(
            log_path("/api/v1/enrollments/one-time-secret-handle/poll"),
            "/api/v1/enrollments/:id"
        );
        assert_eq!(log_path("/api/v1/me"), "/api/v1/me");
        assert_eq!(log_path("/api/v1/enrollments"), "/api/v1/enrollments");
    }

    fn test_app(static_dir: PathBuf) -> Router {
        let mut args = settings::Args::try_parse_from(["broker", "--dev"]).unwrap();
        args.static_dir = static_dir;
        let settings = Settings::from_args(args).unwrap();
        app(AppState::new(settings).unwrap())
    }

    async fn request(app: Router, path: &str) -> Response {
        app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn health_is_available_without_built_spa() {
        let response = request(
            test_app(PathBuf::from("nonexistent-broker-assets")),
            "/healthz",
        )
        .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: HealthResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.status, "ok");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["mode"], "development");
    }

    #[tokio::test]
    async fn missing_build_is_unavailable_but_api_auth_are_json_not_found() {
        for path in ["/", "/workspaces/test", "/assets/app.js"] {
            assert_eq!(
                request(test_app("nonexistent-broker-assets".into()), path)
                    .await
                    .status(),
                503
            );
        }
        for path in ["/api", "/api/v1/unknown", "/auth", "/auth/github/login"] {
            let response = request(test_app("nonexistent-broker-assets".into()), path).await;
            assert_eq!(response.status(), 404);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        }
    }

    #[tokio::test]
    async fn spa_deep_links_assets_and_reserved_prefixes() {
        // Scratch assets stay inside this crate, not an OS temporary directory.
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("spa-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html>broker SPA</html>").unwrap();
        std::fs::write(dir.join("app.js"), "export {};").unwrap();
        std::fs::write(
            dir.join("favicon.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#,
        )
        .unwrap();
        std::fs::write(dir.join("site.webmanifest"), r#"{"name":"azbridge"}"#).unwrap();
        for path in ["/", "/workspaces/one"] {
            let response = request(test_app(dir.clone()), path).await;
            assert_eq!(response.status(), 200);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(body, "<html>broker SPA</html>");
        }
        assert_eq!(
            request(test_app(dir.clone()), "/app.js").await.status(),
            200
        );
        for (path, content_type) in [
            ("/favicon.svg", "image/svg+xml"),
            ("/site.webmanifest", "application/manifest+json"),
        ] {
            let response = request(test_app(dir.clone()), path).await;
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert!(!body.starts_with(b"<html>"));
        }
        for path in ["/api", "/api/missing", "/auth", "/auth/missing"] {
            assert_eq!(request(test_app(dir.clone()), path).await.status(), 404);
        }
        let response = test_app(dir.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 405);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let response = test_app(dir.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/workspaces/one")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 405);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
