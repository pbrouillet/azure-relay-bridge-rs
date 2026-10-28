//! Stable, deliberately non-sensitive HTTP errors.
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use azbridge_broker_api::ErrorResponse;

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("invalid request")]
    BadRequest,
    #[error("authentication required")]
    Unauthorized,
    #[error("access denied")]
    Forbidden,
    #[error("resource not found")]
    NotFound,
    #[error("resource conflict")]
    Conflict,
    #[error(
        "Workspace still has connections. Open the workspace and delete its connections first. If a published profile uses a connection, revoke that profile's gateways and delete or update the profile first. Wait for deleting connections to finish before retrying."
    )]
    WorkspaceNotEmpty,
    #[error(
        "Connection is referenced by a published profile. Delete or update that profile before deleting the connection."
    )]
    ConnectionInUse,
    #[error(
        "Profile is assigned to a non-revoked gateway. Revoke its gateways in the Gateways tab before deleting the profile. Existing tunnels may remain open."
    )]
    ProfileInUse,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("too many requests")]
    TooManyRequests,
    #[error("service unavailable")]
    Unavailable,
    #[error("internal server error")]
    Internal(#[source] anyhow::Error),
}

impl IntoResponse for BrokerError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict => (StatusCode::CONFLICT, "conflict"),
            Self::WorkspaceNotEmpty => (StatusCode::CONFLICT, "workspace_not_empty"),
            Self::ConnectionInUse => (StatusCode::CONFLICT, "connection_in_use"),
            Self::ProfileInUse => (StatusCode::CONFLICT, "profile_in_use"),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
            Self::TooManyRequests => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            Self::Internal(error) => {
                // Do not log arbitrary upstream error text: URLs and bodies can contain tokens.
                tracing::error!(error_type = %std::any::type_name_of_val(error.root_cause()), "unexpected broker error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (
            status,
            Json(ErrorResponse {
                code: code.into(),
                message: self.to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn errors_have_safe_json_and_correct_status() {
        let cases = [
            (BrokerError::BadRequest, 400),
            (BrokerError::Unauthorized, 401),
            (BrokerError::Forbidden, 403),
            (BrokerError::NotFound, 404),
            (BrokerError::Conflict, 409),
            (BrokerError::WorkspaceNotEmpty, 409),
            (BrokerError::ConnectionInUse, 409),
            (BrokerError::ProfileInUse, 409),
            (BrokerError::MethodNotAllowed, 405),
            (BrokerError::TooManyRequests, 429),
            (BrokerError::Unavailable, 503),
            (
                BrokerError::Internal(anyhow::anyhow!("credential=never-disclose")),
                500,
            ),
        ];
        for (error, status) in cases {
            let response = error.into_response();
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: ErrorResponse = serde_json::from_slice(&body).unwrap();
            assert!(!body.code.is_empty());
            assert!(!body.message.contains("never-disclose"));
        }
    }
}
