//! Persistent record shapes shared by administrative and gateway endpoints.

use azbridge_broker_api::{AuditEntry, Gateway, Profile, Workspace};
use serde::{Deserialize, Serialize};

use crate::{AppState, error::BrokerError, storage::Table};

pub const PLATFORM_PARTITION: &str = "platform";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    pub workspace: Workspace,
    pub created_by: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub profile: Profile,
    pub next_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionRecord {
    pub revision: u64,
    pub created_at: u64,
}

/// The credential's digest is internal metadata, not part of the gateway DTO.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayRecord {
    pub gateway: Gateway,
    pub credential_hash: String,
    pub created_by: u64,
}

/// The device code is encrypted before the record is persisted.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollmentRecord {
    pub workspace_id: String,
    pub profile_id: String,
    pub gateway_name: String,
    pub encrypted_device_code: String,
    pub expires_at: u64,
    pub next_poll_at: u64,
    pub interval: u64,
}

pub fn revision_key(profile_id: &str, revision: u64) -> String {
    format!("{profile_id}:{revision:020}")
}

pub async fn record_audit(
    state: &AppState,
    workspace: &str,
    actor: &str,
    action: &str,
    target: &str,
) -> Result<(), BrokerError> {
    let entry = AuditEntry {
        id: uuid::Uuid::new_v4().to_string(),
        at: crate::auth::now()?,
        actor: actor.into(),
        action: action.into(),
        target: target.into(),
    };
    state
        .metadata_store
        .insert(
            Table::Audit,
            workspace,
            &entry.id,
            serde_json::to_value(&entry).map_err(|error| BrokerError::Internal(error.into()))?,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use azbridge_broker_api::ProvisioningState;
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::Value;

    fn roundtrip<T: Serialize + DeserializeOwned>(value: T) {
        let encoded = serde_json::to_value(value).unwrap();
        let decoded: T = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    }

    #[test]
    fn workspace_record_roundtrip() {
        roundtrip(WorkspaceRecord {
            workspace: Workspace {
                id: "workspace-id".into(),
                name: "Office".into(),
                namespace_name: "azb-office-unique".into(),
                location: "westus".into(),
                state: ProvisioningState::Pending,
                error: None,
            },
            created_by: 1,
        });
    }

    #[test]
    fn profile_and_revision_records_roundtrip() {
        roundtrip(ProfileRecord {
            profile: Profile {
                id: "profile-id".into(),
                name: "gateway".into(),
                current_revision: None,
            },
            next_revision: 1,
        });
        roundtrip(RevisionRecord {
            revision: 1,
            created_at: 1_791_984_600,
        });
    }

    #[test]
    fn gateway_record_roundtrip_does_not_flatten_private_fields() {
        let gateway = Gateway {
            id: "gateway-id".into(),
            name: "gateway".into(),
            profile_id: "profile-id".into(),
            revoked: false,
            last_seen_at: None,
            applied_revision: None,
            version: None,
            hostname: None,
        };
        let record = GatewayRecord {
            gateway,
            credential_hash: "test-only-digest".into(),
            created_by: 1,
        };
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["gateway"]["credentialHash"], Value::Null);
        roundtrip(record);
    }

    #[test]
    fn enrollment_record_roundtrip() {
        roundtrip(EnrollmentRecord {
            workspace_id: "workspace-id".into(),
            profile_id: "profile-id".into(),
            gateway_name: "gateway".into(),
            encrypted_device_code: "encrypted-test-only".into(),
            expires_at: 900,
            next_poll_at: 5,
            interval: 5,
        });
    }

    #[test]
    fn revision_keys_sort_in_revision_order() {
        assert_eq!(revision_key("p", 1), "p:00000000000000000001");
        assert!(revision_key("p", 9) < revision_key("p", 10));
    }

    #[tokio::test]
    async fn audit_records_are_partitioned_by_workspace() {
        use clap::Parser;
        let settings = crate::settings::Settings::from_args(
            crate::settings::Args::try_parse_from(["broker", "--dev"]).unwrap(),
        )
        .unwrap();
        let state = AppState::new(settings).unwrap();
        record_audit(&state, "workspace", "user:1", "publish", "profile")
            .await
            .unwrap();
        let entries = state
            .metadata_store
            .list(Table::Audit, "workspace")
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        let entry: AuditEntry = serde_json::from_value(entries[0].data.clone()).unwrap();
        assert_eq!(entry.action, "publish");
        assert!(
            state
                .metadata_store
                .list(Table::Audit, "other")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            record_audit(&state, "invalid/workspace", "user:1", "publish", "profile").await,
            Err(BrokerError::BadRequest)
        ));
    }
}
