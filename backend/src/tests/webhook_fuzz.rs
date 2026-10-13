//! Bounded fuzzing of the webhook, the one route reachable without the API key, guarded by a
//! token in its path. No body may panic or answer a 500.
//!
//! A seeded xorshift generator stands in for `cargo-fuzz`, which needs nightly.

use axum::body::Body;
use axum::http::{Request, StatusCode};

use super::TestApp;

/// The statuses the webhook may answer. `tests::api` runs a valid event to a 200 against an
/// Arr that answers.
fn is_controlled(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        // A refusal, a 200 for an event that needs no sync, a 404 for a bad token, or a 502
        // from the sync a valid event starts against the test's unreachable Arr.
        200 | 400 | 404 | 405 | 411 | 413 | 415 | 422 | 502
    )
}

async fn post_raw(app: &TestApp, path: &str, body: Vec<u8>, content_type: &str) -> StatusCode {
    let request =
        Request::post(path).header("content-type", content_type).body(Body::from(body)).unwrap();
    app.send(request).await.status
}

/// Posts `body` under the right token, a wrong one and an empty one. A 404 under the right
/// token means the body never reached the handler, and the case tested nothing.
async fn assert_controlled(app: &TestApp, body: Vec<u8>, content_type: &str, note: &str) {
    for token in ["tok", "wrong-token", ""] {
        let path = format!("/api/v1/webhook/inst-1/{token}");
        let status = post_raw(app, &path, body.clone(), content_type).await;
        assert!(
            is_controlled(status),
            "{note} (token {token:?}) produced an uncontrolled {status}"
        );
        assert!(
            token != "tok" || status != StatusCode::NOT_FOUND,
            "{note} never reached the handler with the right token"
        );
    }
}

// --------------------------------------------------------------- corpus

fn adversarial_corpus() -> Vec<(&'static str, Vec<u8>)> {
    let deep_array = format!("{}{}", "[".repeat(2000), "]".repeat(2000));
    let deep_object = {
        let mut s = String::new();
        for _ in 0..2000 {
            s.push_str("{\"a\":");
        }
        s.push('1');
        s.push_str(&"}".repeat(2000));
        s
    };

    vec![
        ("empty body", b"".to_vec()),
        ("whitespace only", b"   \n\t  ".to_vec()),
        ("not json", b"this is not json at all".to_vec()),
        ("truncated object", b"{\"eventType\":".to_vec()),
        ("truncated string", b"{\"eventType\":\"Down".to_vec()),
        ("json null", b"null".to_vec()),
        ("json true", b"true".to_vec()),
        ("json number", b"42".to_vec()),
        ("json string", b"\"Download\"".to_vec()),
        ("root array", b"[1,2,3]".to_vec()),
        ("eventType is a number", br#"{"eventType": 12345}"#.to_vec()),
        ("eventType is an object", br#"{"eventType": {"nested": true}}"#.to_vec()),
        ("movie is a string", br#"{"eventType":"Download","movie":"not-an-object"}"#.to_vec()),
        ("movie.id is a string", br#"{"eventType":"Download","movie":{"id":"NaN"}}"#.to_vec()),
        (
            "movie.id overflows i64",
            br#"{"eventType":"Download","movie":{"id":99999999999999999999}}"#.to_vec(),
        ),
        ("movie.id is negative", br#"{"eventType":"Download","movie":{"id":-1}}"#.to_vec()),
        (
            "tmdbId is a float",
            br#"{"eventType":"Download","movie":{"id":1,"tmdbId":3.14}}"#.to_vec(),
        ),
        (
            "title carries control bytes",
            b"{\"eventType\":\"Download\",\"movie\":{\"id\":1,\"title\":\"a\x00b\x01c\"}}".to_vec(),
        ),
        (
            "title is enormous",
            format!(
                r#"{{"eventType":"Download","movie":{{"id":1,"title":"{}"}}}}"#,
                "A".repeat(200_000)
            )
            .into_bytes(),
        ),
        ("unicode event", "{\"eventType\":\"💥Download\"}".as_bytes().to_vec()),
        (
            "both movie and series",
            br#"{"eventType":"Download","movie":{"id":1},"series":{"id":2}}"#.to_vec(),
        ),
        (
            "duplicate keys",
            br#"{"eventType":"A","eventType":"Download","movie":{"id":1}}"#.to_vec(),
        ),
        ("deeply nested array", deep_array.into_bytes()),
        ("deeply nested object", deep_object.into_bytes()),
        ("bare high surrogate", br#"{"eventType":"\ud800"}"#.to_vec()),
    ]
}

#[tokio::test]
async fn no_hostile_payload_makes_the_webhook_panic_or_500() {
    let app = TestApp::new().await;
    app.seed_library().await;

    for (note, body) in adversarial_corpus() {
        assert_controlled(&app, body, "application/json", note).await;
    }
}

/// An Arr sends JSON. A delivery under any other content type is refused as
/// unsupported, never read, even under the right token.
#[tokio::test]
async fn the_wrong_content_type_is_refused_as_unsupported() {
    let app = TestApp::new().await;
    app.seed_library().await;

    let body = br#"{"eventType":"Download","movie":{"id":1}}"#.to_vec();
    let right = "/api/v1/webhook/inst-1/tok";
    let read = post_raw(&app, right, body.clone(), "application/json").await;
    assert_ne!(read, StatusCode::UNSUPPORTED_MEDIA_TYPE, "the JSON delivery itself was refused");
    for content_type in
        ["text/plain", "application/x-www-form-urlencoded", "application/octet-stream", ""]
    {
        assert_controlled(&app, body.clone(), content_type, content_type).await;
        let status = post_raw(&app, right, body.clone(), content_type).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{content_type:?} was read");
    }
}

// -------------------------------------------------------- generative

/// A reproducible RNG that needs no dependency.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A random JSON value up to `depth` deep. Its keys are the fields the handler reads, so the
/// deserialiser gets past the first one.
fn random_json(rng: &mut XorShift, depth: u32) -> serde_json::Value {
    use serde_json::Value;
    let leaf = depth == 0 || rng.below(3) == 0;
    match if leaf { rng.below(6) } else { rng.below(8) } {
        0 => Value::Null,
        1 => Value::Bool(rng.below(2) == 1),
        2 => Value::from(rng.next() as i64),
        3 => Value::from(rng.next() as f64 / 3.0),
        4 => Value::from(random_token(rng)),
        5 => Value::from(
            ["Download", "MovieAdded", "SeriesAdd", "Rename", "Test", "?"][rng.below(6) as usize],
        ),
        6 => {
            let n = rng.below(5);
            Value::Array((0..n).map(|_| random_json(rng, depth - 1)).collect())
        }
        _ => {
            let mut map = serde_json::Map::new();
            let keys = ["eventType", "movie", "series", "id", "tmdbId", "title", "junk"];
            let n = rng.below(5) + 1;
            for _ in 0..n {
                let key = keys[rng.below(keys.len() as u64) as usize];
                map.insert(key.to_string(), random_json(rng, depth - 1));
            }
            Value::Object(map)
        }
    }
}

fn random_token(rng: &mut XorShift) -> String {
    let len = rng.below(8) as usize;
    (0..len).map(|_| (b'a' + (rng.below(26) as u8)) as char).collect()
}

#[tokio::test]
async fn thousands_of_random_payloads_stay_controlled() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // A fixed seed, so a failing iteration reproduces on a rerun.
    let mut rng = XorShift(0x9E3779B97F4A7C15);

    for i in 0..3000 {
        let value = random_json(&mut rng, 6);
        let body = serde_json::to_vec(&value).unwrap();
        let path = "/api/v1/webhook/inst-1/tok";
        let status = post_raw(&app, path, body, "application/json").await;
        assert!(
            is_controlled(status),
            "iteration {i} produced an uncontrolled {status} for: {value}"
        );
    }
}
