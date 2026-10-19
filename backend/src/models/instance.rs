use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
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
     sync_interval_minutes, last_sync_at, last_sync_attempt_at, last_sync_status, created_at,
     updated_at, webhook_token, auto_apply";

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
    /// When one was last attempted, successful or not: the figure that says
    /// whether the scheduler is running at all.
    pub last_sync_attempt_at: Option<String>,
    pub last_sync_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Secret path segment for this instance's webhook endpoint.
    #[serde(skip_serializing)]
    pub webhook_token: Option<String>,
    /// Whether automatic application may write here, under the global switch.
    pub auto_apply: bool,
}

/// Request body for creating/updating an instance.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInstanceRequest {
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_minutes: i64,
    #[serde(default = "default_true")]
    pub auto_apply: bool,
}

/// An Arr Routarr reads.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InstanceResponse {
    pub id: String,
    pub name: String,
    pub instance_type: String,
    pub base_url: String,
    /// A fixed mask when a key is stored, never any part of it. Empty when
    /// none is, and for an application key.
    pub api_key_masked: String,
    pub enabled: bool,
    pub sync_interval_minutes: i64,
    /// When a sync last *succeeded*. Null while none ever has.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_sync_at: Option<String>,
    /// When one was last attempted, successful or not: the figure that says
    /// whether the scheduler is running at all.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_sync_attempt_at: Option<String>,
    pub last_sync_status: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub updated_at: String,
    /// Relative URL to register in Radarr/Sonarr's webhook connection, the
    /// token in it, for an Arr that sends no custom header. `null` for an
    /// application key: its token lets anyone post events as the Arr.
    pub webhook_url: Option<String>,
    /// The same without the token, which goes in `X-Routarr-Token`: Radarr
    /// 5.16 and Sonarr 4.0.11 send custom headers. `null` for an application
    /// key.
    pub webhook_path: Option<String>,
    /// The token for `X-Routarr-Token`. `null` for an application key.
    pub webhook_token: Option<String>,
    /// Always `true`: every stored key is encrypted at rest.
    pub api_key_encrypted: bool,
    /// Whether the Arr's key is stored. An instance a configuration import
    /// created waits for its key, which a bundle never carries.
    pub api_key_set: bool,
    /// Whether automatic application may write to this instance. It writes only while the
    /// `auto_apply_enabled` setting is on and the dry run off as well: off, the instance's
    /// moves stay proposals.
    pub auto_apply: bool,
}

impl InstanceResponse {
    /// Not a `From` impl: the webhook URL handed to the Arr must carry the sub-path Routarr is
    /// served under, `base_path`, or it 404s behind a reverse proxy, and only there.
    pub fn from_instance(
        i: Instance,
        base_path: &str,
        localizer: &crate::localization::Localizer,
        key_set: bool,
    ) -> Self {
        let masked = if key_set {
            "\u{2022}\u{2022}\u{2022}\u{2022} (encrypted)".to_string()
        } else {
            String::new()
        };
        let webhook_path =
            i.webhook_token.as_ref().map(|_| format!("{base_path}/api/v1/webhook/{}", i.id));
        let webhook_url = webhook_path
            .as_ref()
            .zip(i.webhook_token.as_ref())
            .map(|(path, t)| format!("{path}/{t}"));
        Self {
            id: i.id,
            name: i.name,
            instance_type: i.instance_type,
            base_url: crate::http::masked(&i.base_url).into_owned(),
            api_key_masked: masked,
            enabled: i.enabled,
            sync_interval_minutes: i.sync_interval_minutes,
            last_sync_at: i.last_sync_at,
            last_sync_attempt_at: i.last_sync_attempt_at,
            last_sync_status: sync_status_said(i.last_sync_status, localizer),
            created_at: i.created_at,
            updated_at: i.updated_at,
            webhook_url,
            webhook_path,
            webhook_token: i.webhook_token,
            api_key_encrypted: true,
            api_key_set: key_set,
            auto_apply: i.auto_apply,
        }
    }
}

/// The last sync's outcome, `success`, or `error: ` and why, the why said in
/// `localizer`'s language: the sync stores it as a key and its values.
pub fn sync_status_said(
    stored: Option<String>,
    localizer: &crate::localization::Localizer,
) -> Option<String> {
    stored.map(|status| match status.strip_prefix(SYNC_FAILED) {
        Some(why) => format!("{SYNC_FAILED}{}", crate::localization::said(why, localizer)),
        None => status,
    })
}

/// What the last sync's outcome starts with when it failed, which every
/// reader of it tells a failure by.
pub const SYNC_FAILED: &str = "error: ";

fn default_true() -> bool {
    true
}
fn default_sync_interval() -> i64 {
    15
}
