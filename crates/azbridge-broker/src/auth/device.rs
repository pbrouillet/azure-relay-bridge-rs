//! Browser-bound GitHub device login, separate from gateway enrollment.

use super::*;
use azbridge_broker_api::{DeviceLoginPollResponse, DeviceLoginStartResponse};

const DEVICE_COOKIE: &str = "azbridge_device";
const DEVICE_PATH: &str = "/auth/github/device";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceChallenge {
    id: String,
    csrf_token: String,
    expires_at: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceLoginRecord {
    encrypted_device_code: String,
    expires_at: u64,
    next_poll_at: u64,
    interval: u64,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/github/device/start", post(start))
        .route("/auth/github/device/poll", post(poll))
}

async fn start(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, BrokerError> {
    check_origin(&state, &headers)?;
    // A custom header prevents cross-site form submissions before a challenge exists.
    if headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        != Some("device-login")
    {
        return Err(BrokerError::Forbidden);
    }
    let github = state.auth.github.as_ref().ok_or(BrokerError::NotFound)?;
    state
        .auth
        .rate_limit("device_login.start", 30, Duration::from_secs(60))
        .await?;
    let device = github.start_device_flow().await?;
    let timestamp = now()?;
    let challenge = DeviceChallenge {
        id: random_secret(),
        csrf_token: random_secret(),
        expires_at: timestamp + device.expires_in,
    };
    let record = DeviceLoginRecord {
        encrypted_device_code: state.auth.codec.seal(&device.device_code)?,
        expires_at: challenge.expires_at,
        next_poll_at: timestamp + device.interval,
        interval: device.interval,
    };
    state
        .metadata_store
        .insert(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &format!("device-login:{}", challenge.id),
            serde_json::to_value(record).map_err(|error| BrokerError::Internal(error.into()))?,
        )
        .await?;
    let encrypted_challenge = state.auth.codec.seal(&challenge)?;
    let mut response = Json(DeviceLoginStartResponse {
        user_code: device.user_code,
        verification_uri: device.verification_uri,
        interval: device.interval,
        expires_in: device.expires_in,
        csrf_token: challenge.csrf_token,
    })
    .into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header(
            &state,
            DEVICE_COOKIE,
            &encrypted_challenge,
            device.expires_in,
            DEVICE_PATH,
        )?,
    );
    Ok(response)
}

fn reply(state: &AppState, result: DeviceLoginPollResponse) -> Result<Response, BrokerError> {
    let terminal = !matches!(
        result,
        DeviceLoginPollResponse::Pending { .. } | DeviceLoginPollResponse::SlowDown { .. }
    );
    let mut response = Json(result).into_response();
    if terminal {
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie_header(state, DEVICE_COOKIE, "", 0, DEVICE_PATH)?,
        );
    }
    Ok(response)
}

async fn poll(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, BrokerError> {
    check_origin(&state, &headers)?;
    let challenge: DeviceChallenge = state.auth.codec.open(cookie(&headers, DEVICE_COOKIE)?)?;
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or(BrokerError::Forbidden)?;
    if !constant_time_equal(csrf, &challenge.csrf_token) {
        return Err(BrokerError::Forbidden);
    }
    let github = state.auth.github.as_ref().ok_or(BrokerError::NotFound)?;
    state
        .auth
        .rate_limit("device_login.poll", 240, Duration::from_secs(60))
        .await?;
    let key = format!("device-login:{}", challenge.id);
    let Some(entity) = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &key)
        .await?
    else {
        return reply(&state, DeviceLoginPollResponse::Expired);
    };
    let mut record: DeviceLoginRecord =
        serde_json::from_value(entity.data).map_err(|error| BrokerError::Internal(error.into()))?;
    let timestamp = now()?;
    if record.expires_at <= timestamp || challenge.expires_at <= timestamp {
        state
            .metadata_store
            .delete(Table::Enrollments, PLATFORM_PARTITION, &key, &entity.etag)
            .await?;
        return reply(&state, DeviceLoginPollResponse::Expired);
    }
    let early = timestamp < record.next_poll_at;
    if early {
        record.interval = record.interval.saturating_add(5);
    }
    record.next_poll_at = timestamp.saturating_add(record.interval);
    // Reserve with CAS before contacting GitHub, so concurrent polls cannot mint sessions.
    let reserved = state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &key,
            serde_json::to_value(&record).map_err(|error| BrokerError::Internal(error.into()))?,
            &entity.etag,
        )
        .await?;
    if early {
        return reply(
            &state,
            DeviceLoginPollResponse::SlowDown {
                interval: record.interval,
            },
        );
    }
    let device_code: String = state.auth.codec.open(&record.encrypted_device_code)?;
    let result = github.poll_device_flow(&device_code).await?;
    match result {
        DevicePoll::Pending => reply(
            &state,
            DeviceLoginPollResponse::Pending {
                interval: record.interval,
            },
        ),
        DevicePoll::SlowDown => {
            record.interval = record.interval.saturating_add(5);
            record.next_poll_at = now()?.saturating_add(record.interval);
            state
                .metadata_store
                .replace(
                    Table::Enrollments,
                    PLATFORM_PARTITION,
                    &key,
                    serde_json::to_value(&record)
                        .map_err(|error| BrokerError::Internal(error.into()))?,
                    &reserved.etag,
                )
                .await?;
            reply(
                &state,
                DeviceLoginPollResponse::SlowDown {
                    interval: record.interval,
                },
            )
        }
        DevicePoll::Expired | DevicePoll::Denied => {
            state
                .metadata_store
                .delete(Table::Enrollments, PLATFORM_PARTITION, &key, &reserved.etag)
                .await?;
            reply(
                &state,
                if matches!(result, DevicePoll::Denied) {
                    DeviceLoginPollResponse::Denied
                } else {
                    DeviceLoginPollResponse::Expired
                },
            )
        }
        DevicePoll::Authorized(token) => {
            let user = github.consume_token(&token).await?;
            state
                .metadata_store
                .delete(Table::Enrollments, PLATFORM_PARTITION, &key, &reserved.etag)
                .await?;
            if record.expires_at <= now()? {
                return reply(&state, DeviceLoginPollResponse::Expired);
            }
            if !is_platform_admin(&state, &user).await?
                && accessible_workspaces(&state, &user).await?.is_empty()
            {
                return reply(&state, DeviceLoginPollResponse::Denied);
            }
            let mut response = reply(&state, DeviceLoginPollResponse::Authorized)?;
            set_session(&state, &user, &mut response)?;
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests;
