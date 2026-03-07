use crate::adi_router::{AdiCallerContext, AdiHandleResult, AdiService, AdiServiceError};
use crate::protocol::types::{AdiMethodInfo, AdiServiceCapabilities};
use async_trait::async_trait;
use credentials_core::{
    Config, CredentialRow, CredentialType, Database, SecretManager,
};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use uuid::Uuid;

/// Credentials service for ADI router
///
/// Provides secure credential storage and retrieval over WebRTC.
pub struct CredentialsService {
    db: Database,
    secrets: SecretManager,
}

impl CredentialsService {
    /// Initialize from environment configuration
    pub async fn from_env() -> Result<Self, String> {
        let config = Config::from_env().map_err(|e| format!("Config error: {e}"))?;

        let pool = PgPool::connect(&config.database_url)
            .await
            .map_err(|e| format!("Database connection failed: {e}"))?;

        let secrets =
            SecretManager::from_hex(&config.encryption_key).map_err(|e| format!("{e}"))?;

        Ok(Self {
            db: Database::new(pool),
            secrets,
        })
    }

    fn parse_uuid(params: &JsonValue, field: &str) -> Result<Uuid, AdiServiceError> {
        params
            .get(field)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Uuid>().ok())
            .ok_or_else(|| AdiServiceError::invalid_params(format!("{field} is required (UUID)")))
    }

    fn parse_credential_type(s: &str) -> Result<CredentialType, AdiServiceError> {
        match s {
            "github_token" => Ok(CredentialType::GithubToken),
            "gitlab_token" => Ok(CredentialType::GitlabToken),
            "api_key" => Ok(CredentialType::ApiKey),
            "oauth2" => Ok(CredentialType::Oauth2),
            "ssh_key" => Ok(CredentialType::SshKey),
            "password" => Ok(CredentialType::Password),
            "certificate" => Ok(CredentialType::Certificate),
            "custom" => Ok(CredentialType::Custom),
            other => Err(AdiServiceError::invalid_params(format!(
                "Unknown credential type: {other}"
            ))),
        }
    }

    async fn handle_list(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let cred_type = params
            .get("credential_type")
            .and_then(|v| v.as_str())
            .map(Self::parse_credential_type)
            .transpose()?;
        let provider = params.get("provider").and_then(|v| v.as_str());

        let rows = match (cred_type, provider) {
            (None, None) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .fetch_all(self.db.pool())
                .await
            }
            (Some(ct), None) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND credential_type = $2 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(ct)
                .fetch_all(self.db.pool())
                .await
            }
            (None, Some(prov)) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND provider = $2 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(prov)
                .fetch_all(self.db.pool())
                .await
            }
            (Some(ct), Some(prov)) => {
                sqlx::query_as::<_, CredentialRow>(
                    "SELECT * FROM credentials WHERE user_id = $1 AND credential_type = $2 AND provider = $3 ORDER BY created_at DESC",
                )
                .bind(user_id)
                .bind(ct)
                .bind(prov)
                .fetch_all(self.db.pool())
                .await
            }
        }
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        let credentials: Vec<JsonValue> = rows.into_iter().map(|r| row_to_json(&r)).collect();
        Ok(AdiHandleResult::Success(json!(credentials)))
    }

    async fn handle_get(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        let row = sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))?;

        Ok(AdiHandleResult::Success(row_to_json(&row)))
    }

    async fn handle_get_with_data(
        &self,
        ctx: &AdiCallerContext,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        let row = sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))?;

        let decrypted = self
            .secrets
            .decrypt(&row.encrypted_data)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;
        let data: JsonValue = serde_json::from_str(&decrypted)
            .map_err(|e| AdiServiceError::internal(format!("Failed to parse credential data: {e}")))?;

        sqlx::query("UPDATE credentials SET last_used_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.log_access(id, user_id, "read").await;

        let mut result = row_to_json(&row);
        result["data"] = data;
        Ok(AdiHandleResult::Success(result))
    }

    async fn handle_create(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;

        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("name is required"))?
            .trim();

        if name.is_empty() {
            return Err(AdiServiceError::invalid_params("Name cannot be empty"));
        }

        let credential_type = params
            .get("credential_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("credential_type is required"))
            .and_then(Self::parse_credential_type)?;

        let data = params
            .get("data")
            .ok_or_else(|| AdiServiceError::invalid_params("data is required"))?;

        let data_json = serde_json::to_string(data)
            .map_err(|e| AdiServiceError::invalid_params(format!("Invalid data: {e}")))?;
        let encrypted_data = self
            .secrets
            .encrypt(&data_json)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        let metadata = params.get("metadata").cloned().unwrap_or(json!({}));
        let description = params.get("description").and_then(|v| v.as_str());
        let provider = params.get("provider").and_then(|v| v.as_str());
        let expires_at = params.get("expires_at").and_then(|v| v.as_str());

        let row = sqlx::query_as::<_, CredentialRow>(
            r#"
            INSERT INTO credentials (user_id, name, description, credential_type, encrypted_data, metadata, provider, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)
            RETURNING *
            "#,
        )
        .bind(user_id)
        .bind(name)
        .bind(description)
        .bind(&credential_type)
        .bind(&encrypted_data)
        .bind(&metadata)
        .bind(provider)
        .bind(expires_at)
        .fetch_one(self.db.pool())
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

        Ok(AdiHandleResult::Success(row_to_json(&row)))
    }

    async fn handle_update(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        let existing = sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))?;

        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&existing.name);
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .or(existing.description.as_deref());
        let metadata = params
            .get("metadata")
            .cloned()
            .unwrap_or(existing.metadata);
        let provider = params
            .get("provider")
            .and_then(|v| v.as_str())
            .or(existing.provider.as_deref());
        let expires_at = params
            .get("expires_at")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| existing.expires_at.map(|e| e.to_rfc3339()));

        let encrypted_data = if let Some(new_data) = params.get("data") {
            let data_json = serde_json::to_string(new_data)
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
        .bind(name)
        .bind(description)
        .bind(&encrypted_data)
        .bind(&metadata)
        .bind(provider)
        .bind(expires_at.as_deref())
        .bind(id)
        .bind(user_id)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.log_access(id, user_id, "update").await;

        Ok(AdiHandleResult::Success(row_to_json(&row)))
    }

    async fn handle_delete(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        self.log_access(id, user_id, "delete").await;

        let result =
            sqlx::query("DELETE FROM credentials WHERE id = $1 AND user_id = $2")
                .bind(id)
                .bind(user_id)
                .execute(self.db.pool())
                .await
                .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(AdiServiceError::not_found("Credential not found"));
        }

        Ok(AdiHandleResult::Success(json!({ "deleted": true })))
    }

    async fn handle_verify(&self, ctx: &AdiCallerContext, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        let row = sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))?;

        let is_expired = row
            .expires_at
            .map(|exp| exp < chrono::Utc::now())
            .unwrap_or(false);

        Ok(AdiHandleResult::Success(json!({
            "valid": !is_expired,
            "is_expired": is_expired,
            "expires_at": row.expires_at,
        })))
    }

    async fn handle_access_logs(
        &self,
        ctx: &AdiCallerContext,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let user_id: Uuid = ctx.require_user_id()?.parse().map_err(|_| AdiServiceError::internal("Invalid user_id format"))?;
        let id = Self::parse_uuid(&params, "id")?;

        // Verify ownership
        sqlx::query_as::<_, CredentialRow>(
            "SELECT * FROM credentials WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?
        .ok_or_else(|| AdiServiceError::not_found("Credential not found"))?;

        let logs = sqlx::query_as::<_, credentials_core::CredentialAccessLog>(
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
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(logs)))
    }

    async fn log_access(&self, credential_id: Uuid, user_id: Uuid, action: &str) {
        let _ = sqlx::query(
            "INSERT INTO credential_access_log (credential_id, user_id, action) VALUES ($1, $2, $3)",
        )
        .bind(credential_id)
        .bind(user_id)
        .bind(action)
        .execute(self.db.pool())
        .await;
    }
}

fn row_to_json(row: &CredentialRow) -> JsonValue {
    json!({
        "id": row.id,
        "name": row.name,
        "description": row.description,
        "credential_type": row.credential_type,
        "metadata": row.metadata,
        "provider": row.provider,
        "expires_at": row.expires_at,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "last_used_at": row.last_used_at,
    })
}

#[async_trait]
impl AdiService for CredentialsService {
    fn service_id(&self) -> &str {
        "credentials"
    }

    fn name(&self) -> &str {
        "Credentials"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> Option<&str> {
        Some("Secure credential storage with encryption, audit logging, and expiry verification")
    }

    fn capabilities(&self) -> AdiServiceCapabilities {
        AdiServiceCapabilities {
            subscriptions: false,
            notifications: false,
            streaming: false,
        }
    }

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![
            AdiMethodInfo {
                name: "list".to_string(),
                description: "List credentials, optionally filtered by type/provider".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "credential_type": {
                            "type": "string",
                            "enum": ["github_token", "gitlab_token", "api_key", "oauth2", "ssh_key", "password", "certificate", "custom"]
                        },
                        "provider": { "type": "string" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Credential" }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "get".to_string(),
                description: "Get credential metadata (without decrypted data)".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Credential" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "get_with_data".to_string(),
                description: "Get credential with decrypted sensitive data".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/CredentialWithData" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "create".to_string(),
                description: "Create a new credential".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["name", "credential_type", "data"],
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "credential_type": {
                            "type": "string",
                            "enum": ["github_token", "gitlab_token", "api_key", "oauth2", "ssh_key", "password", "certificate", "custom"]
                        },
                        "data": { "type": "object", "description": "Sensitive credential data (will be encrypted)" },
                        "metadata": { "type": "object" },
                        "provider": { "type": "string" },
                        "expires_at": { "type": "string", "format": "date-time" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Credential" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "update".to_string(),
                description: "Update a credential (partial update)".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "data": { "type": "object" },
                        "metadata": { "type": "object" },
                        "provider": { "type": "string" },
                        "expires_at": { "type": "string", "format": "date-time" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Credential" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "delete".to_string(),
                description: "Delete a credential".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": { "deleted": { "type": "boolean" } }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "verify".to_string(),
                description: "Verify credential validity (check expiration)".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "valid": { "type": "boolean" },
                        "is_expired": { "type": "boolean" },
                        "expires_at": { "type": ["string", "null"], "format": "date-time" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "access_logs".to_string(),
                description: "Get access audit logs for a credential".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/CredentialAccessLog" }
                })),
                ..Default::default()
            },
        ]
    }

    async fn handle(
        &self,
        ctx: &AdiCallerContext,
        method: &str,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "list" => self.handle_list(ctx, params).await,
            "get" => self.handle_get(ctx, params).await,
            "get_with_data" => self.handle_get_with_data(ctx, params).await,
            "create" => self.handle_create(ctx, params).await,
            "update" => self.handle_update(ctx, params).await,
            "delete" => self.handle_delete(ctx, params).await,
            "verify" => self.handle_verify(ctx, params).await,
            "access_logs" => self.handle_access_logs(ctx, params).await,
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}
