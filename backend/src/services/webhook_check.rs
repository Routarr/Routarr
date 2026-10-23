//! Whether the Arr's webhook connection reaches an instance with every event Routarr acts on,
//! read at each sync and by a connection test.

use std::collections::HashMap;

use sqlx::SqlitePool;
use tracing::warn;

use crate::error::AppResult;
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::arr_webhook::{RoutarrConnection, TRIGGERS};
use crate::localization::Localizer;
use crate::models::{Instance, TokenCheck, WebhookCheck, WebhookCheckResponse, WebhookState};
use crate::state::AppState;

/// The check of `instance` among its Arr's `connections`. `others` names the instances on
/// another Arr by id, to spot a connection pasted into the wrong Arr, and `tags` the Arr's tags.
pub fn evaluate(
    instance: &Instance,
    connections: &[RoutarrConnection],
    others: &HashMap<String, String>,
    tags: &HashMap<i64, String>,
) -> WebhookCheck {
    let own = connections
        .iter()
        .filter(|connection| connection.instance_id == instance.id)
        .map(|connection| assess(instance, connection, tags))
        .min_by_key(WebhookCheck::blocking);
    if let Some(check) = own {
        return check;
    }
    let other_instance =
        connections.iter().find_map(|connection| others.get(&connection.instance_id)).cloned();
    let state =
        if other_instance.is_some() { WebhookState::Misconfigured } else { WebhookState::Absent };
    WebhookCheck { state, other_instance, ..Default::default() }
}

fn assess(
    instance: &Instance,
    connection: &RoutarrConnection,
    tags: &HashMap<i64, String>,
) -> WebhookCheck {
    let current = instance.webhook_token.as_deref().unwrap_or_default();
    let compared = |token: &str| {
        if !current.is_empty() && token == current { TokenCheck::Ok } else { TokenCheck::Stale }
    };
    // In the order the webhook routes read a token.
    let token = match (&connection.path_token, &connection.header_token, &connection.password) {
        (Some(token), _, _) | (None, Some(token), _) => compared(token),
        (None, None, Some(password)) if password.chars().all(|c| c == '*') => TokenCheck::Unchecked,
        (None, None, Some(password)) => compared(password),
        (None, None, None) => TokenCheck::Missing,
    };
    let ticked = |field: &&str| connection.switches.get(*field) == Some(&true);
    let missing_triggers = TRIGGERS
        .iter()
        .filter(|trigger| trigger.kind == instance.instance_type)
        // A switch the Arr does not list belongs to another release of it.
        .filter(|trigger| trigger.fields.iter().any(|f| connection.switches.contains_key(*f)))
        .filter(|trigger| !trigger.fields.iter().any(ticked))
        .map(|trigger| trigger.label.to_string())
        .collect();
    let mut check = WebhookCheck {
        state: WebhookState::Ok,
        token: Some(token),
        missing_triggers,
        method: (connection.method != "POST").then(|| connection.method.clone()),
        tags: connection
            .tags
            .iter()
            .map(|id| tags.get(id).cloned().unwrap_or_else(|| id.to_string()))
            .collect(),
        other_instance: None,
    };
    if check.blocking() > 0 {
        check.state = WebhookState::Misconfigured;
    }
    check
}

/// Read `instance`'s webhook connection from its Arr. `tags` names the Arr's tags, read from it
/// when not given.
pub async fn read(
    state: &AppState,
    instance: &Instance,
    adapter: &ArrAdapter,
    tags: Option<&HashMap<i64, String>>,
) -> AppResult<WebhookCheck> {
    let connections = adapter.webhook_connections().await?;
    let others = instances_on_another_arr(&state.pool, instance).await?;
    let read;
    let tags = match tags {
        Some(tags) => tags,
        None => {
            read = match adapter.get_tags().await {
                Ok(tags) => tags.into_iter().map(|tag| (tag.arr_id, tag.label)).collect(),
                Err(_) => {
                    crate::services::sync::stored_tag_labels(&state.pool, &instance.id).await?
                }
            };
            &read
        }
    };
    Ok(evaluate(instance, &connections, &others, tags))
}

async fn instances_on_another_arr(
    pool: &SqlitePool,
    instance: &Instance,
) -> AppResult<HashMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, name FROM instances WHERE id != ? AND base_url != ?")
            .bind(&instance.id)
            .bind(&instance.base_url)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Store `check` on `instance`, unless its token or its address changed since it was read.
pub async fn store<'e>(
    executor: impl sqlx::SqliteExecutor<'e>,
    instance: &Instance,
    check: &WebhookCheck,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE instances SET webhook_check = ?, webhook_checked_at = datetime('now')
          WHERE id = ? AND webhook_token IS ? AND base_url = ?",
    )
    .bind(serde_json::to_string(check)?)
    .bind(&instance.id)
    .bind(&instance.webhook_token)
    .bind(&instance.base_url)
    .execute(executor)
    .await?;
    Ok(())
}

/// Read, store and say `instance`'s webhook connection, as a connection test reports it. An Arr
/// that cannot list its connections leaves the last check in place.
pub async fn checked(
    state: &AppState,
    instance: &Instance,
    adapter: &ArrAdapter,
    localizer: &Localizer,
) -> Option<WebhookCheckResponse> {
    let read = async {
        let check = read(state, instance, adapter, None).await?;
        store(&state.pool, instance, &check).await?;
        AppResult::Ok(check)
    };
    match read.await {
        Ok(check) => Some(WebhookCheckResponse::said(
            check,
            &instance.name,
            crate::services::routing::format_timestamp(chrono::Utc::now()),
            localizer,
        )),
        Err(e) => {
            warn!(instance = %instance.name, "Could not read the Arr's webhook connections: {e}");
            None
        }
    }
}
