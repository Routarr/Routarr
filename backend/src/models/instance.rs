use serde::{Deserialize, Serialize};

/// Type of Arr instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InstanceType {
    Radarr,
    Sonarr,
}

impl std::fmt::Display for InstanceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceType::Radarr => write!(f, "radarr"),
            InstanceType::Sonarr => write!(f, "sonarr"),
        }
    }
}

impl std::str::FromStr for InstanceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "radarr" => Ok(InstanceType::Radarr),
            "sonarr" => Ok(InstanceType::Sonarr),
            _ => Err(format!("Invalid instance type: {}", s)),
        }
    }
}

/// Column list shared by every `SELECT` on `instances`, kept in one place so a
/// schema change cannot leave one query out of sync with the struct.
pub const INSTANCE_COLUMNS: &str = "id, name, instance_type, base_url, api_key, enabled,
     sync_interval_minutes, last_sync_at, last_sync_attempt_at, last_sync_status, created_at, updated_at, webhook_token";

/// An Arr instance (Radarr or Sonarr) configured in Routarr.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// When a sync last *succeeded*. Null while none ever has.
    pub last_sync_at: Option<String>,
    /// When one was last attempted, successful or not — the figure that says
    /// whether the scheduler is running at all.
    pub last_sync_attempt_at: Option<String>,
    pub last_sync_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Secret path segment for this instance's webhook endpoint.
    #[serde(skip_serializing)]
    pub webhook_token: Option<String>,
}

/// Request body for creating/updating an instance.
#[derive(Debug, Deserialize)]
pub struct CreateInstanceRequest {
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_minutes: i64,
}

/// Response for instance listing (with masked API key).
#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    pub id: String,
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// When a sync last *succeeded*. Null while none ever has.
    pub last_sync_at: Option<String>,
    /// When one was last attempted, successful or not — the figure that says
    /// whether the scheduler is running at all.
    pub last_sync_attempt_at: Option<String>,
    pub last_sync_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Relative URL to register in Radarr/Sonarr's webhook connection.
    pub webhook_url: Option<String>,
    /// Whether the stored API key is encrypted at rest.
    pub api_key_encrypted: bool,
}

impl InstanceResponse {
    /// Build the response, given where Routarr is mounted.
    ///
    /// Not a `From` impl on purpose: the webhook URL is handed to Radarr to call
    /// back, so it has to carry the sub-path Routarr is served under. A
    /// conversion that cannot see the configuration would silently produce a URL
    /// that 404s behind a reverse proxy — and only there.
    pub fn from_instance(i: Instance, base_path: &str) -> Self {
        // The stored value may be ciphertext; never expose a prefix of it.
        let encrypted = crate::crypto::SecretBox::is_sealed(&i.api_key);
        let masked = if encrypted {
            "\u{2022}\u{2022}\u{2022}\u{2022} (encrypted)".to_string()
        } else {
            mask(&i.api_key)
        };
        let webhook_url =
            i.webhook_token.as_ref().map(|t| format!("{base_path}/api/v1/webhook/{}/{}", i.id, t));
        Self {
            id: i.id,
            name: i.name,
            instance_type: i.instance_type,
            base_url: i.base_url,
            api_key_masked: masked,
            enabled: i.enabled,
            sync_interval_minutes: i.sync_interval_minutes,
            last_sync_at: i.last_sync_at,
            last_sync_attempt_at: i.last_sync_attempt_at,
            last_sync_status: i.last_sync_status,
            created_at: i.created_at,
            updated_at: i.updated_at,
            webhook_url,
            api_key_encrypted: encrypted,
        }
    }
}

/// Show only enough of a legacy plaintext key to recognise it.
fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() > 8 {
        format!(
            "{}\u{2026}{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    } else {
        "\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
    }
}

fn default_true() -> bool {
    true
}
fn default_sync_interval() -> i64 {
    15
}
