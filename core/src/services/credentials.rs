include!(concat!(env!("OUT_DIR"), "/credentials_adi_service.rs"));

use credentials_core::{
    Config, Credential, CredentialAccessLog, CredentialRow, CredentialType, CredentialWithData,
    DeleteResult, SecretManager, VerifyResult,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Credentials service for ADI router.
///
/// Provides secure credential storage and retrieval over WebRTC.
pub struct CredentialsService {
    db: PgPool,
    secrets: SecretManager,
}

impl CredentialsService {
    /// Initialize from environment configuration.
    pub async fn from_env() -> Result<Self, String> {
        let config = Config::from_env().map_err(|e| format!("Config error: {e}"))?;

        let pool = PgPool::connect(&config.database_url)
            .await
            .map_err(|e| format!("Database connection failed: {e}"))?;

        let secrets =
            SecretManager::from_hex(&config.encryption_key).map_err(|e| format!("{e}"))?;

        Ok(Self {
            db: pool,
            secrets,
        })
    }

    fn parse_user_id(ctx: &AdiCallerContext) -> Result<Uuid, AdiServiceError> {
        ctx.require_user_id()?
            .parse()
            .map_err(|_| AdiServiceError::internal("Invalid user_id format"))
    }

    async fn log_access(&self, credential_id: Uuid, user_id: Uuid, action: &str) {
        let _ = sqlx::query(
            "INSERT INTO credential_access_log (credential_id, user_id, action) VALUES ($1, $2, $3)",
        )
        .bind(credential_id)
        .bind(user_id)
        .bind(action)
        .execute(&self.db)
        .await;
    }

    fn row_to_credential(row: CredentialRow) -> Credential {
        row.into()
    }

    async fn fetch_row(&self, id: Uuid, user_id: Uuid) -> Result<CredentialRow, AdiServiceError> {
        sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))
    }
}

#[async_trait]
impl CredentialsServiceHandler for CredentialsService {
    async fn list(
        &self,
        ctx: &AdiCallerContext,
        credential_type: Option<CredentialType>,
        provider: Option<String>,
    ) -> Result<Vec<Credential>, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;

        let rows = match (credential_type, provider.as_deref()) {
            (None, None) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .fetch_all(&self.db)
                .await
            }
            (Some(ct), None) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND credential_type = $2 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(ct)
                .fetch_all(&self.db)
                .await
            }
            (None, Some(prov)) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND provider = $2 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(prov)
                .fetch_all(&self.db)
                .await
            }
            (Some(ct), Some(prov)) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND credential_type = $2 AND provider = $3 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(ct)
                .bind(prov)
                .fetch_all(&self.db)
                .await
            }
        }
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(rows.into_iter().map(Self::row_to_credential).collect())
    }

    async fn get(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
    ) -> Result<Credential, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;
        let row = self.fetch_row(id, user_id).await?;
        Ok(Self::row_to_credential(row))
    }

    async fn get_with_data(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
    ) -> Result<CredentialWithData, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;
        let row = self.fetch_row(id, user_id).await?;

        let decrypted = self
            .secrets
            .decrypt(&row.encrypted_data)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;
        let data: serde_json::Value = serde_json::from_str(&decrypted)
            .map_err(|e| AdiServiceError::internal(format!("Failed to parse credential data: {e}")))?;

        sqlx::query("UPDATE credentials SET last_used_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.log_access(id, user_id, "read").await;

        Ok(CredentialWithData {
            credential: Self::row_to_credential(row),
            data,
        })
    }

    async fn create(
        &self,
        ctx: &AdiCallerContext,
        name: String,
        credential_type: CredentialType,
        data: std::collections::HashMap<String, serde_json::Value>,
        description: Option<String>,
        metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
        provider: Option<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Credential, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;

        let name = name.trim();
        if name.is_empty() {
            return Err(AdiServiceError::invalid_params("Name cannot be empty"));
        }

        let data_json = serde_json::to_string(&data)
            .map_err(|e| AdiServiceError::invalid_params(format!("Invalid data: {e}")))?;
        let encrypted_data = self
            .secrets
            .encrypt(&data_json)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        let metadata_value = metadata
            .map(|m| serde_json::to_value(m).unwrap_or_default())
            .unwrap_or_else(|| serde_json::json!({}));

        let expires_at_str = expires_at.map(|e| e.to_rfc3339());

        let row = sqlx::query_as::<_, CredentialRow>(
            r#"
            INSERT INTO credentials (user_id, name, description, credential_type, encrypted_data, metadata, provider, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)
            RETURNING *
            "#,
        )
        .bind(user_id)
        .bind(name)
        .bind(description.as_deref())
        .bind(&credential_type)
        .bind(&encrypted_data)
        .bind(&metadata_value)
        .bind(provider.as_deref())
        .bind(expires_at_str.as_deref())
        .fetch_one(&self.db)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.constraint() == Some("credentials_user_id_name_key") {
                    return AdiServiceError::invalid_params(format!(
                        "Credential with name '{name}' already exists"
                    ));
                }
            }
            AdiServiceError::internal(e.to_string())
        })?;

        self.log_access(row.id, user_id, "create").await;

        Ok(Self::row_to_credential(row))
    }

    async fn update(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
        data: Option<std::collections::HashMap<String, serde_json::Value>>,
        metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
        provider: Option<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Credential, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;
        let existing = self.fetch_row(id, user_id).await?;

        let final_name = name.as_deref().unwrap_or(&existing.name);
        let final_description = description
            .as_deref()
            .or(existing.description.as_deref());
        let final_metadata = metadata
            .map(|m| serde_json::to_value(m).unwrap_or_default())
            .unwrap_or(existing.metadata);
        let final_provider = provider
            .as_deref()
            .or(existing.provider.as_deref());
        let final_expires_at = expires_at
            .map(|e| e.to_rfc3339())
            .or_else(|| existing.expires_at.map(|e| e.to_rfc3339()));

        let encrypted_data = if let Some(new_data) = data {
            let data_json = serde_json::to_string(&new_data)
                .map_err(|e| AdiServiceError::invalid_params(format!("Invalid data: {e}")))?;
            self.secrets
                .encrypt(&data_json)
                .map_err(|e| AdiServiceError::internal(e.to_string()))?
        } else {
            existing.encrypted_data
        };

        let row = sqlx::query_as::<_, CredentialRow>(
            r#"
            UPDATE credentials
            SET name = $1, description = $2, encrypted_data = $3, metadata = $4,
                provider = $5, expires_at = $6::timestamptz, updated_at = NOW()
            WHERE id = $7 AND user_id = $8
            RETURNING *
            "#,
        )
        .bind(final_name)
        .bind(final_description)
        .bind(&encrypted_data)
        .bind(&final_metadata)
        .bind(final_provider)
        .bind(final_expires_at.as_deref())
        .bind(id)
        .bind(user_id)
        .fetch_one(&self.db)
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.log_access(id, user_id, "update").await;

        Ok(Self::row_to_credential(row))
    }

    async fn delete(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
    ) -> Result<DeleteResult, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;

        self.log_access(id, user_id, "delete").await;

        let result =
            sqlx::query("DELETE FROM credentials WHERE id = $1 AND user_id = $2")
                .bind(id)
                .bind(user_id)
                .execute(&self.db)
                .await
                .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(AdiServiceError::not_found("Credential not found"));
        }

        Ok(DeleteResult { deleted: true })
    }

    async fn verify(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
    ) -> Result<VerifyResult, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;
        let row = self.fetch_row(id, user_id).await?;

        let is_expired = row
            .expires_at
            .map(|exp| exp < chrono::Utc::now())
            .unwrap_or(false);

        Ok(VerifyResult {
            valid: !is_expired,
            is_expired,
            expires_at: row.expires_at,
        })
    }

    async fn access_logs(
        &self,
        ctx: &AdiCallerContext,
        id: Uuid,
    ) -> Result<Vec<CredentialAccessLog>, AdiServiceError> {
        let user_id = Self::parse_user_id(ctx)?;

        // Verify ownership
        self.fetch_row(id, user_id).await?;

        let logs = sqlx::query_as::<_, CredentialAccessLog>(
            r#"
            SELECT id, credential_id, user_id, action,
                   host(ip_address)::text as ip_address, user_agent, details, created_at
            FROM credential_access_log
            WHERE credential_id = $1
            ORDER BY created_at DESC
            LIMIT 100
            "#,
        )
        .bind(id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(logs)
    }
}
