//! The Arr's webhook connection to an instance, read from its Settings, Connect and held to what
//! the webhook routes accept and the events Routarr acts on.

use super::TestApp;
use super::fake_arr::FakeArr;
use serde_json::{Value, json};

const ADDRESS: &str = "http://routarr:9876/api/v1/webhook/inst-1";

/// A webhook connection as Radarr 6 lists it, every switch Routarr needs ticked and the token in
/// the header.
fn radarr_connection(url: &str) -> Value {
    json!({
        "id": 7,
        "name": "Routarr",
        "implementation": "Webhook",
        "configContract": "WebhookSettings",
        "onGrab": false,
        "onDownload": true,
        "onUpgrade": true,
        "onRename": true,
        "onMovieAdded": true,
        "onMovieDelete": true,
        "onMovieFileDelete": true,
        "onMovieFileDeleteForUpgrade": true,
        "onHealthIssue": false,
        "onHealthRestored": false,
        "onApplicationUpdate": false,
        "onManualInteractionRequired": false,
        "supportsOnGrab": true,
        "supportsOnRename": true,
        "tags": [],
        "fields": [
            { "name": "url", "value": url },
            { "name": "method", "value": 1 },
            { "name": "username" },
            { "name": "password" },
            { "name": "headers", "value": [{ "key": "X-Routarr-Token", "value": "tok" }] },
        ],
    })
}

/// The same as Sonarr 4 lists it.
fn sonarr_connection(url: &str) -> Value {
    let mut connection = radarr_connection(url);
    let switches = connection.as_object_mut().unwrap();
    for radarr_only in ["onMovieAdded", "onMovieDelete", "onMovieFileDelete"] {
        switches.remove(radarr_only);
    }
    for sonarr_only in ["onSeriesAdd", "onSeriesDelete", "onEpisodeFileDelete", "onImportComplete"]
    {
        switches.insert(sonarr_only.into(), json!(true));
    }
    connection
}

fn with_field(mut connection: Value, name: &str, value: Value) -> Value {
    let fields = connection["fields"].as_array_mut().unwrap();
    fields.retain(|field| field["name"] != name);
    fields.push(json!({ "name": name, "value": value }));
    connection
}

fn with(mut connection: Value, member: &str, value: Value) -> Value {
    connection[member] = value;
    connection
}

async fn instance_on(arr: &FakeArr, kind: &str) -> TestApp {
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", kind, &arr.base_url).await;
    if kind == "sonarr" {
        arr.report_version("4.0.20.2900", "Sonarr");
    }
    app
}

/// The check a connection test of `inst-1` reads, its Arr of `kind` holding `connections`.
async fn checked(kind: &str, connections: Value) -> Value {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, kind).await;
    arr.hold_connections(connections);
    app.post("/api/v1/instances/inst-1/test", json!({})).await.assert_ok()["webhook"].clone()
}

async fn stored(app: &TestApp) -> Value {
    app.get("/api/v1/instances/inst-1").await.assert_ok()["webhook_check"].clone()
}

/// A connection with the current token and every switch ticked is right, and the tags limiting
/// it are noted without counting as a fault.
#[tokio::test]
async fn a_connection_set_up_as_routarr_needs_reads_ok() {
    let check = checked("radarr", json!([radarr_connection(ADDRESS)])).await;
    assert_eq!(check["state"], "ok", "{check}");
    assert_eq!(check["token"], "ok");
    assert_eq!(check["problems"], json!([]));
    assert_eq!(check["notes"], json!([]));

    let tagged = with(radarr_connection(ADDRESS), "tags", json!([1]));
    let check = checked("radarr", json!([tagged])).await;
    assert_eq!(check["state"], "ok", "{check}");
    assert_eq!(check["tags"], json!(["anime"]));
    assert!(check["notes"][0].as_str().unwrap().contains("anime"), "{check}");
}

/// Each fault that keeps a delivery out, or an event Routarr acts on from being sent, is named
/// in the Arr's own words.
#[tokio::test]
async fn each_fault_that_stops_a_delivery_is_named() {
    let old_header = with_field(
        radarr_connection(ADDRESS),
        "headers",
        json!([{ "key": "x-routarr-token", "value": "old" }]),
    );
    let check = checked("radarr", json!([old_header])).await;
    assert_eq!((&check["state"], &check["token"]), (&json!("misconfigured"), &json!("stale")));
    assert_eq!(check["problems"].as_array().unwrap().len(), 1, "{check}");

    let in_address = |token: &str| {
        with_field(radarr_connection(&format!("{ADDRESS}/{token}")), "headers", json!([]))
    };
    assert_eq!(checked("radarr", json!([in_address("old")])).await["token"], "stale");
    assert_eq!(checked("radarr", json!([in_address("tok")])).await["token"], "ok");

    let bare = with_field(radarr_connection(ADDRESS), "headers", json!([]));
    assert_eq!(checked("radarr", json!([bare])).await["token"], "missing");

    let put = with_field(radarr_connection(ADDRESS), "method", json!(2));
    let check = checked("radarr", json!([put])).await;
    assert_eq!((&check["state"], &check["method"]), (&json!("misconfigured"), &json!("PUT")));

    let quiet = with(
        with(radarr_connection(ADDRESS), "onRename", json!(false)),
        "onMovieAdded",
        json!(false),
    );
    let check = checked("radarr", json!([quiet])).await;
    assert_eq!(check["missing_triggers"], json!(["On Rename", "On Movie Added"]));
    let problem = check["problems"][0].as_str().unwrap();
    assert!(problem.contains("On Rename") && problem.contains("On Movie Added"), "{problem}");
}

/// A Basic password the Arr masks leaves the token to the first delivery, and one returned in
/// clear is compared.
#[tokio::test]
async fn a_masked_password_cannot_be_checked_and_a_clear_one_is() {
    let basic = |password: &str| {
        with_field(
            with_field(radarr_connection(ADDRESS), "headers", json!([])),
            "password",
            json!(password),
        )
    };
    let check = checked("radarr", json!([basic("********")])).await;
    assert_eq!((&check["state"], &check["token"]), (&json!("ok"), &json!("unchecked")));
    assert_eq!(check["notes"].as_array().unwrap().len(), 1, "{check}");
    assert_eq!(checked("radarr", json!([basic("tok")])).await["token"], "ok");
    assert_eq!(checked("radarr", json!([basic("old")])).await["token"], "stale");
}

/// Sonarr's Import Complete sends the `Download` On File Import does, and a switch an older Arr
/// does not list is not asked of it.
#[tokio::test]
async fn import_complete_stands_for_file_import_and_an_unlisted_switch_is_not_asked_for() {
    let complete_only = with(sonarr_connection(ADDRESS), "onDownload", json!(false));
    let check = checked("sonarr", json!([complete_only])).await;
    assert_eq!(check["state"], "ok", "{check}");
    let neither = with(
        with(sonarr_connection(ADDRESS), "onDownload", json!(false)),
        "onImportComplete",
        json!(false),
    );
    assert_eq!(
        checked("sonarr", json!([neither])).await["missing_triggers"],
        json!(["On File Import"])
    );

    let mut older = radarr_connection(ADDRESS);
    older.as_object_mut().unwrap().remove("onMovieAdded");
    assert_eq!(checked("radarr", json!([older])).await["state"], "ok");
}

/// Of two connections naming the instance, the one that works is reported.
#[tokio::test]
async fn the_working_one_of_two_connections_is_reported() {
    let stale = with_field(radarr_connection(ADDRESS), "headers", json!([]));
    let check = checked("radarr", json!([stale, radarr_connection(ADDRESS)])).await;
    assert_eq!(check["state"], "ok", "{check}");
}

/// A connection naming an instance on another Arr, as a Radarr address pasted into Sonarr, is
/// named. Another service's webhook is not Routarr's, and nothing of it is kept.
#[tokio::test]
async fn a_connection_naming_another_instance_is_named_and_other_services_are_ignored() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    app.seed_instance_at("inst-2", "sonarr", "http://127.0.0.1:1").await;
    let discord = json!({
        "implementation": "Discord",
        "onDownload": true,
        "fields": [{ "name": "webHookUrl", "value": "https://discord.example/api/webhooks/1/SECRET-1" }],
    });
    let home = with_field(
        radarr_connection("http://home:8123/api/webhook/SECRET-2"),
        "headers",
        json!([{ "key": "Authorization", "value": "Bearer SECRET-3" }]),
    );
    arr.hold_connections(json!([discord, home]));
    app.post("/api/v1/instances/inst-1/test", json!({})).await.assert_ok();
    assert_eq!(stored(&app).await["state"], "absent");

    let elsewhere = radarr_connection("http://routarr:9876/api/v1/webhook/inst-2");
    arr.hold_connections(json!([discord, home, elsewhere]));
    app.post("/api/v1/instances/inst-1/test", json!({})).await.assert_ok();
    let check = stored(&app).await;
    assert_eq!(
        (&check["state"], &check["other_instance"]),
        (&json!("misconfigured"), &json!("Fake sonarr"))
    );
    let kept: String =
        sqlx::query_scalar("SELECT webhook_check FROM instances WHERE id = 'inst-1'")
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
    assert!(!kept.contains("SECRET"), "{kept}");
}

/// Each sync reads the connection. A rotation or a new address clears what was read of the old
/// one, and the next read finds the Arr still sending the old token.
#[tokio::test]
async fn each_sync_reads_the_connection_and_a_rotation_or_a_new_address_clears_it() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    arr.hold_connections(json!([radarr_connection(ADDRESS)]));
    assert!(stored(&app).await.is_null());
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();
    assert_eq!(stored(&app).await["state"], "ok");

    app.post("/api/v1/instances/inst-1/webhook-token", json!({})).await.assert_ok();
    assert!(stored(&app).await.is_null());
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();
    assert_eq!(stored(&app).await["token"], "stale");

    let other = FakeArr::start().await;
    let moved = json!({
        "name": "Fake radarr", "instance_type": "radarr", "base_url": other.base_url,
        "api_key": "another-key",
    });
    app.put("/api/v1/instances/inst-1", moved).await.assert_ok();
    assert!(stored(&app).await.is_null());
}

/// The form's try reads the connection of the instance it edits, on the Arr it was saved with,
/// and never for values not saved yet.
#[tokio::test]
async fn a_try_reads_the_saved_instance_connection_and_not_one_of_new_values() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    arr.hold_connections(json!([radarr_connection(ADDRESS)]));
    let typed = |id: Option<&str>| json!({ "instance_type": "radarr", "base_url": arr.base_url, "api_key": "k", "id": id });

    let fresh = app.post("/api/v1/instances/test", typed(None)).await;
    assert!(fresh.assert_ok()["webhook"].is_null());
    assert!(stored(&app).await.is_null());
    let edited = app.post("/api/v1/instances/test", typed(Some("inst-1"))).await;
    assert_eq!(edited.assert_ok()["webhook"]["state"], "ok");
    assert_eq!(stored(&app).await["state"], "ok");
}

/// An Arr that cannot list its connections keeps the last check, and its sync goes on.
#[tokio::test]
async fn an_arr_that_cannot_list_its_connections_keeps_the_last_check() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    arr.hold_connections(json!([radarr_connection(ADDRESS)]));
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();

    arr.break_connection_listing(500);
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();
    assert_eq!(stored(&app).await["state"], "ok");
    let tested = app.post("/api/v1/instances/inst-1/test", json!({})).await;
    assert!(tested.assert_ok()["webhook"].is_null());
}

/// An armed instance whose connection needs a fix is warned of each fault, and not of its
/// silence, which the fault explains. Once fixed, the silence is warned of again.
#[tokio::test]
async fn an_armed_instance_is_warned_of_a_fault_in_its_connection_rather_than_its_silence() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    app.execute(&["UPDATE instances SET created_at = datetime('now', '-2 hours')"]).await;
    let codes = || async {
        let status = app.get("/api/v1/status").await;
        status.assert_ok()["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["code"].as_str().unwrap().starts_with("webhook_"))
            .map(|w| (w["code"].as_str().unwrap().to_string(), w["message"].to_string()))
            .collect::<Vec<_>>()
    };
    let quiet = with(radarr_connection(ADDRESS), "onRename", json!(false));
    arr.hold_connections(json!([quiet]));
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();
    assert!(codes().await.is_empty(), "warned with automatic application off");

    app.store_setting("auto_apply_enabled", "true").await;
    app.store_setting("global_dry_run", "false").await;
    let warned = codes().await;
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].0, "webhook_misconfigured");
    assert!(warned[0].1.contains("On Rename") && warned[0].1.contains("Fake radarr"), "{warned:?}");

    arr.hold_connections(json!([radarr_connection(ADDRESS)]));
    app.post("/api/v1/instances/inst-1/sync", json!({})).await.assert_ok();
    let warned = codes().await;
    assert_eq!(
        warned.iter().map(|w| w.0.as_str()).collect::<Vec<_>>(),
        ["webhook_never_delivered"]
    );
}

/// The token a check calls right is the one the webhook routes accept, whichever way the Arr
/// sends it, and the one it calls stale or missing is refused.
#[tokio::test]
async fn the_token_verdict_agrees_with_what_the_webhook_routes_accept() {
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;

    let basic = |password: &str| {
        let pair = base64::engine::general_purpose::STANDARD.encode(format!("arr:{password}"));
        ("Authorization", format!("Basic {pair}"))
    };
    let shapes: Vec<(String, Vec<(&str, String)>)> = vec![
        (format!("{ADDRESS}/tok"), vec![]),
        (format!("{ADDRESS}/old"), vec![("X-Routarr-Token", "tok".into())]),
        (ADDRESS.into(), vec![("X-Routarr-Token", "tok".into())]),
        (ADDRESS.into(), vec![("X-Routarr-Token", "old".into())]),
        (ADDRESS.into(), vec![basic("tok")]),
        (ADDRESS.into(), vec![basic("old")]),
        (ADDRESS.into(), vec![("X-Routarr-Token", "tok".into()), basic("old")]),
        (ADDRESS.into(), vec![]),
    ];
    for (url, sent) in shapes {
        let headers: Vec<Value> = sent
            .iter()
            .filter(|(name, _)| *name == "X-Routarr-Token")
            .map(|(name, value)| json!({ "key": name, "value": value }))
            .collect();
        let mut connection = with_field(radarr_connection(&url), "headers", json!(headers));
        if let Some((_, credentials)) = sent.iter().find(|(name, _)| *name == "Authorization") {
            let pair = base64::engine::general_purpose::STANDARD
                .decode(credentials.trim_start_matches("Basic "))
                .unwrap();
            let pair = String::from_utf8(pair).unwrap();
            connection = with_field(connection, "password", json!(pair.split_once(':').unwrap().1));
        }

        let arr = FakeArr::start().await;
        let app = instance_on(&arr, "radarr").await;
        arr.hold_connections(json!([connection]));
        let check =
            app.post("/api/v1/instances/inst-1/test", json!({})).await.assert_ok()["webhook"]
                .clone();

        let path = url.trim_start_matches("http://routarr:9876");
        let mut request = Request::post(path).header("content-type", "application/json");
        for (name, value) in &sent {
            request = request.header(*name, value);
        }
        let delivered = app
            .send_raw(request.body(Body::from(r#"{"eventType":"Test"}"#)).unwrap())
            .await
            .status();
        assert_eq!(check["token"] == "ok", delivered.is_success(), "{url} {sent:?}: {check}");
    }
}

/// A check read with the token a rotation then replaced is not stored: it would call the old
/// token right until the next sync.
#[tokio::test]
async fn a_check_read_before_a_rotation_is_not_stored() {
    let arr = FakeArr::start().await;
    let app = instance_on(&arr, "radarr").await;
    let read_with_old_token = app.state.instance("inst-1").await.unwrap();
    let check = crate::models::WebhookCheck {
        state: crate::models::WebhookState::Ok,
        ..Default::default()
    };
    app.post("/api/v1/instances/inst-1/webhook-token", json!({})).await.assert_ok();

    crate::services::webhook_check::store(&app.state.pool, &read_with_old_token, &check)
        .await
        .unwrap();
    assert!(stored(&app).await.is_null());
    let read_now = app.state.instance("inst-1").await.unwrap();
    crate::services::webhook_check::store(&app.state.pool, &read_now, &check).await.unwrap();
    assert_eq!(stored(&app).await["state"], "ok");
}
