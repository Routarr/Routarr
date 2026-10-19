//! The webhook connections Radarr and Sonarr hold, and the switches of one that Routarr acts on.

use std::collections::BTreeMap;

/// The header an Arr that sends custom headers carries the token in.
pub const TOKEN_HEADER: &str = "X-Routarr-Token";

/// One switch of an Arr's webhook connection whose event can change where a title belongs.
pub struct Trigger {
    /// The instance type whose Arr has it.
    pub kind: &'static str,
    /// The `eventType` it sends, lower-cased since both Arrs warn its case may change.
    pub event: &'static str,
    /// The connection's switches, any one of which sends it.
    pub fields: &'static [&'static str],
    /// The switch as the Arr's interface names it.
    pub label: &'static str,
}

/// Every switch Routarr needs ticked, in the order each Arr's form lists them. A file deletion
/// counts: removing the last file flips `has_files`, which a rule may read. "On File Upgrade"
/// sends a `Download` too, and is left to the operator.
pub const TRIGGERS: [Trigger; 10] = [
    Trigger { kind: "radarr", event: "download", fields: &["onDownload"], label: "On File Import" },
    Trigger { kind: "radarr", event: "rename", fields: &["onRename"], label: "On Rename" },
    Trigger {
        kind: "radarr",
        event: "movieadded",
        fields: &["onMovieAdded"],
        label: "On Movie Added",
    },
    Trigger {
        kind: "radarr",
        event: "moviedelete",
        fields: &["onMovieDelete"],
        label: "On Movie Delete",
    },
    Trigger {
        kind: "radarr",
        event: "moviefiledelete",
        fields: &["onMovieFileDelete"],
        label: "On Movie File Delete",
    },
    // Sonarr sends a `Download` for each file, and one for a whole release on Import Complete.
    Trigger {
        kind: "sonarr",
        event: "download",
        fields: &["onDownload", "onImportComplete"],
        label: "On File Import",
    },
    Trigger { kind: "sonarr", event: "rename", fields: &["onRename"], label: "On Rename" },
    Trigger {
        kind: "sonarr",
        event: "seriesadd",
        fields: &["onSeriesAdd"],
        label: "On Series Add",
    },
    Trigger {
        kind: "sonarr",
        event: "seriesdelete",
        fields: &["onSeriesDelete"],
        label: "On Series Delete",
    },
    Trigger {
        kind: "sonarr",
        event: "episodefiledelete",
        fields: &["onEpisodeFileDelete"],
        label: "On Episode File Delete",
    },
];

/// Whether a delivery of `event`, lower-cased, can change where a title belongs.
pub fn acted_on(event: &str) -> bool {
    TRIGGERS.iter().any(|trigger| trigger.event == event)
}

/// The switches an instance of `kind` needs ticked, as its Arr names them.
pub fn labels(kind: &str) -> Vec<String> {
    TRIGGERS
        .iter()
        .filter(|trigger| trigger.kind == kind)
        .map(|trigger| trigger.label.to_string())
        .collect()
}

/// A webhook connection of the Arr addressed to a Routarr instance, kept with what a check reads
/// alone. No `Debug`: the header and the password are secrets.
pub struct RoutarrConnection {
    /// The instance its address names, `.../api/v1/webhook/{id}`.
    pub instance_id: String,
    /// The token in the address, `.../webhook/{id}/{token}`.
    pub path_token: Option<String>,
    /// The `X-Routarr-Token` header's value.
    pub header_token: Option<String>,
    /// The Basic password, `********` where the Arr masks it.
    pub password: Option<String>,
    /// `POST` or `PUT`.
    pub method: String,
    pub tags: Vec<i64>,
    /// The `on*` switches the Arr lists.
    pub switches: BTreeMap<String, bool>,
}

/// The connections in `listed`, the body of `GET /api/v3/notification`, addressed to a Routarr
/// instance. Every other connection is dropped here, with the secrets its fields carry.
pub fn routarr_connections(listed: Vec<serde_json::Value>) -> Vec<RoutarrConnection> {
    listed.iter().filter_map(routarr_connection).collect()
}

fn routarr_connection(listed: &serde_json::Value) -> Option<RoutarrConnection> {
    if listed.get("implementation")?.as_str()? != "Webhook" {
        return None;
    }
    let field = |name: &str| {
        listed
            .get("fields")?
            .as_array()?
            .iter()
            .find(|field| field.get("name").and_then(|n| n.as_str()) == Some(name))?
            .get("value")
    };
    let (instance_id, path_token) = addressed(field("url")?.as_str()?)?;
    let text =
        |name: &str| field(name).and_then(|value| value.as_str()).filter(|value| !value.is_empty());
    let header_token = field("headers")
        .and_then(|headers| headers.as_array())
        .into_iter()
        .flatten()
        .find(|header| {
            header
                .get("key")
                .and_then(|key| key.as_str())
                .is_some_and(|key| key.trim().eq_ignore_ascii_case(TOKEN_HEADER))
        })
        .and_then(|header| header.get("value")?.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    // Both Arrs number the methods, POST 1 and PUT 2.
    let method = match field("method") {
        Some(serde_json::Value::Number(n)) if n.as_i64() == Some(2) => "PUT".to_string(),
        Some(serde_json::Value::String(s)) => s.to_ascii_uppercase(),
        _ => "POST".to_string(),
    };
    let switches = listed
        .as_object()?
        .iter()
        .filter(|(name, _)| name.starts_with("on"))
        .filter_map(|(name, value)| Some((name.clone(), value.as_bool()?)))
        .collect();
    let tags = listed
        .get("tags")
        .and_then(|tags| tags.as_array())
        .map(|tags| tags.iter().filter_map(|tag| tag.as_i64()).collect())
        .unwrap_or_default();
    Some(RoutarrConnection {
        instance_id,
        path_token,
        header_token,
        password: text("password").map(str::to_string),
        method,
        tags,
        switches,
    })
}

/// The instance id an address names, and the token after it, when its path ends in
/// `/api/v1/webhook/{id}` or `/api/v1/webhook/{id}/{token}`, whatever the host or the prefix.
fn addressed(url: &str) -> Option<(String, Option<String>)> {
    let url = reqwest::Url::parse(url).ok()?;
    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let at = segments.windows(3).rposition(|w| w == ["api", "v1", "webhook"])? + 3;
    match &segments[at..] {
        [id] => Some((id.to_string(), None)),
        [id, token] => Some((id.to_string(), Some(token.to_string()))),
        _ => None,
    }
}
