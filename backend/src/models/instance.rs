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
     updated_at, webhook_token, auto_apply, last_webhook_at, last_webhook_event, webhook_token_at,
     webhook_check, webhook_checked_at, removal_held";

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
    /// When a webhook with the valid token last arrived, at most once a minute.
    pub last_webhook_at: Option<String>,
    pub last_webhook_event: Option<String>,
    /// When the token was rotated. Null for the one the instance was created with.
    pub webhook_token_at: Option<String>,
    /// The Arr's webhook connection as last read, a [`WebhookCheck`] in JSON.
    #[serde(skip)]
    pub webhook_check: Option<String>,
    pub webhook_checked_at: Option<String>,
    pub removal_held: i64,
}

/// How the Arr's webhook connection to an instance is set up, as the check stores it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebhookCheck {
    pub state: WebhookState,
    pub token: Option<TokenCheck>,
    pub missing_triggers: Vec<String>,
    /// The method, when Routarr refuses it.
    pub method: Option<String>,
    pub tags: Vec<String>,
    /// The name of the instance a connection names instead of this one.
    pub other_instance: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebhookState {
    /// Every event Routarr acts on reaches it with the current token.
    Ok,
    /// No connection names this instance.
    #[default]
    Absent,
    /// A connection names it, or names another instance, and needs a fix.
    Misconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenCheck {
    Ok,
    /// Another token than the current one, which Routarr refuses.
    Stale,
    /// No token at all.
    Missing,
    /// A Basic password the Arr masks, which only a delivery proves.
    Unchecked,
}

impl WebhookCheck {
    /// How many things stop a delivery, or an event Routarr acts on, from arriving.
    pub fn blocking(&self) -> usize {
        usize::from(self.other_instance.is_some())
            + usize::from(matches!(self.token, Some(TokenCheck::Stale | TokenCheck::Missing)))
            + usize::from(self.method.is_some())
            + self.missing_triggers.len()
    }

    /// What stops a delivery, or an event Routarr acts on, from arriving, each a sentence
    /// naming `instance`.
    pub fn problems(
        &self,
        instance: &str,
        localizer: &crate::localization::Localizer,
    ) -> Vec<String> {
        let say = |key: &str, extra: &[(&str, &str)]| {
            let mut values = vec![("instance", instance)];
            values.extend_from_slice(extra);
            localizer.translate(key, &values)
        };
        let mut problems = Vec::new();
        if let Some(other) = &self.other_instance {
            problems.push(say("WebhookCheckOtherInstance", &[("other", other)]));
        }
        match self.token {
            Some(TokenCheck::Stale) => problems.push(say("WebhookCheckTokenStale", &[])),
            Some(TokenCheck::Missing) => problems.push(say("WebhookCheckTokenMissing", &[])),
            _ => {}
        }
        if let Some(method) = &self.method {
            problems.push(say("WebhookCheckMethod", &[("method", method)]));
        }
        if !self.missing_triggers.is_empty() {
            let triggers = self.missing_triggers.join(&localizer.translate("ListSeparator", &[]));
            problems.push(say("WebhookCheckTriggers", &[("triggers", &triggers)]));
        }
        problems
    }

    /// What is worth knowing and stops nothing.
    pub fn notes(&self, instance: &str, localizer: &crate::localization::Localizer) -> Vec<String> {
        let mut notes = Vec::new();
        if self.token == Some(TokenCheck::Unchecked) {
            notes
                .push(localizer.translate("WebhookCheckTokenUnchecked", &[("instance", instance)]));
        }
        if !self.tags.is_empty() {
            let tags = self.tags.join(&localizer.translate("ListSeparator", &[]));
            notes.push(
                localizer.translate("WebhookCheckTags", &[("instance", instance), ("tags", &tags)]),
            );
        }
        notes
    }
}

/// The Arr's webhook connection to an instance, as Routarr last read it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookCheckResponse {
    pub state: WebhookState,
    /// Null when no connection names the instance.
    pub token: Option<TokenCheck>,
    /// The Arr's switches the connection lacks, as its interface names them: `On Rename`.
    pub missing_triggers: Vec<String>,
    /// The method the connection uses when Routarr refuses it, `PUT`.
    pub method: Option<String>,
    /// The tags limiting the connection to the titles carrying one.
    pub tags: Vec<String>,
    /// The instance a connection on this Arr names instead of this one.
    pub other_instance: Option<String>,
    /// What to fix, in the interface language.
    pub problems: Vec<String>,
    /// What is worth knowing and stops nothing, in the interface language.
    pub notes: Vec<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub checked_at: String,
}

impl WebhookCheckResponse {
    pub fn said(
        check: WebhookCheck,
        instance: &str,
        checked_at: String,
        localizer: &crate::localization::Localizer,
    ) -> Self {
        Self {
            problems: check.problems(instance, localizer),
            notes: check.notes(instance, localizer),
            state: check.state,
            token: check.token,
            missing_triggers: check.missing_triggers,
            method: check.method,
            tags: check.tags,
            other_instance: check.other_instance,
            checked_at,
        }
    }

    /// The check stored on `instance`, unless none was read since its address or token changed.
    pub fn stored(instance: &Instance, localizer: &crate::localization::Localizer) -> Option<Self> {
        let check = serde_json::from_str(instance.webhook_check.as_deref()?).ok()?;
        let checked_at = instance.webhook_checked_at.clone()?;
        Some(Self::said(check, &instance.name, checked_at, localizer))
    }
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
    /// How the last sync ended: `success`, or `error: ` and the reason. Deprecated: the reason
    /// alone is `last_sync_error`.
    #[schema(deprecated)]
    pub last_sync_status: Option<String>,
    /// Why the last sync failed, in the interface language. Null when it succeeded or none ran.
    pub last_sync_error: Option<String>,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub updated_at: String,
    /// Relative URL to register in Radarr/Sonarr's webhook connection, the
    /// token in it, for an Arr given neither a header nor a password. `null`
    /// for an application key: its token lets anyone post events as the Arr.
    pub webhook_url: Option<String>,
    /// The same without the token, which goes in `X-Routarr-Token`: Radarr
    /// 5.16 and Sonarr 4.0.11 send custom headers, and every release can send
    /// a Basic password. `null` for an application key.
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
    /// When a webhook with this instance's token last arrived, recorded at most once a minute.
    /// Null while none has since the instance was added or its token rotated.
    #[serde(serialize_with = "crate::timestamp::rfc3339_or_null")]
    #[schema(format = DateTime)]
    pub last_webhook_at: Option<String>,
    /// The `eventType` that delivery carried, as `Test` or `MovieAdded`.
    pub last_webhook_event: Option<String>,
    /// The Arr's webhook connection to this instance, read at each sync and by a connection
    /// test. Null until one is read, and again once the address or the token changes.
    pub webhook_check: Option<WebhookCheckResponse>,
    /// The switches the Arr's webhook connection needs ticked, as its interface names them.
    pub webhook_triggers: Vec<String>,
    /// How many titles the last sync kept although the Arr no longer reports them, more than
    /// half of the library. They stay until the owner removes them.
    pub removal_held: i64,
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
        let webhook_check = WebhookCheckResponse::stored(&i, localizer);
        let webhook_triggers = crate::integrations::arr_webhook::labels(&i.instance_type);
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
            last_sync_error: sync_error_said(i.last_sync_status.as_deref(), localizer),
            last_sync_status: sync_status_said(i.last_sync_status, localizer),
            created_at: i.created_at,
            updated_at: i.updated_at,
            webhook_url,
            webhook_path,
            webhook_token: i.webhook_token,
            api_key_encrypted: true,
            api_key_set: key_set,
            auto_apply: i.auto_apply,
            last_webhook_at: i.last_webhook_at,
            last_webhook_event: i.last_webhook_event,
            webhook_check,
            webhook_triggers,
            removal_held: i.removal_held,
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

/// Why the last sync failed, in `localizer`'s language, `None` when it did not.
pub fn sync_error_said(
    stored: Option<&str>,
    localizer: &crate::localization::Localizer,
) -> Option<String> {
    let why = stored?.strip_prefix(SYNC_FAILED)?;
    Some(crate::localization::said(why, localizer))
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
