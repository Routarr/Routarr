//! Bounded generative fuzzing of the webhook endpoint.
//!
//! The webhook is the one route an unauthenticated party reaches: the Arrs
//! cannot send a custom header, so it is guarded by a token in the path and its
//! body is whatever the caller posted. The property proved here is that **no
//! input produces a panic or a 5xx** — a panic aborts the worker and a 500
//! leaks an error string, both worse than the "ignored" it should return.
//!
//! Not `cargo-fuzz`, which needs nightly: a seeded xorshift generator, so a
//! failure reproduces from its seed, feeding hand-picked hostile shapes and
//! random JSON trees at the real router.

use axum::body::Body;
use axum::http::{Request, StatusCode};

use super::TestApp;
use super::fake_arr::FakeArr;

/// The statuses the endpoint is allowed to answer with.
///
/// A `502` is in the set on purpose: the test instance points at an unreachable
/// Arr, so a payload that deserialises *and* carries a media id proceeds to a
/// real outbound sync, which then fails with a mapped `Bad Gateway`. That is an
/// honest upstream error, not the failure this module hunts. What must never
/// appear is a `500` — `Database`, `Serialization` or `Internal`, all of which
/// mean an input reached code that could not cope with it and leaked a Rust
/// error string in the process.
fn is_controlled(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        // ok / ignored, bad json, wrong content-type, unprocessable, bad token,
        // and the unreachable-Arr 502 (see above).
        200 | 400 | 404 | 405 | 411 | 413 | 415 | 422 | 502
    )
}

async fn post_raw(app: &TestApp, path: &str, body: Vec<u8>, content_type: &str) -> StatusCode {
    let request =
        Request::post(path).header("content-type", content_type).body(Body::from(body)).unwrap();
    app.send(request).await.status
}

/// A hostile body must never 5xx, whatever the token — a valid token exercises
/// the deserialiser and the whole handler, a bogus one the auth path.
async fn assert_controlled(app: &TestApp, body: Vec<u8>, content_type: &str, note: &str) {
    for token in ["tok", "wrong-token", ""] {
        let path = format!("/api/v1/webhook/inst-1/{token}");
        let status = post_raw(app, &path, body.clone(), content_type).await;
        assert!(
            is_controlled(status),
            "{note} (token {token:?}) produced an uncontrolled {status}"
        );
    }
}

// --------------------------------------------------------------- corpus

/// Shapes chosen because each once broke, or plausibly could break, a naive
/// handler: a non-object root, right key with the wrong type, absurd nesting,
/// numbers that overflow the target integer, and so on.
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

#[tokio::test]
async fn the_wrong_content_type_is_rejected_not_crashed() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // A well-formed JSON body sent as text/plain, form data, or octet-stream:
    // the extractor must refuse it cleanly rather than mis-parse it.
    let body = br#"{"eventType":"Download","movie":{"id":1}}"#.to_vec();
    for content_type in
        ["text/plain", "application/x-www-form-urlencoded", "application/octet-stream", ""]
    {
        assert_controlled(&app, body.clone(), content_type, content_type).await;
    }
}

// -------------------------------------------------------- generative

/// A tiny reproducible RNG. `rand` is a dependency, but a self-contained
/// generator keeps the seed visible in the failure message with no ceremony.
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

/// Build a random JSON value up to `depth`, biased towards the field names the
/// handler actually reads so the deserialiser is genuinely exercised, not just
/// handed noise it discards at the first key.
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

    // A fixed seed: a failure names the iteration, and re-running reproduces the
    // exact tree that caused it.
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

#[tokio::test]
async fn a_valid_event_on_an_unknown_instance_is_a_404_not_a_500() {
    let app = TestApp::new().await;
    app.seed_library().await;

    // An instance id that does not exist must fail closed, and must not reveal
    // whether the token would have been valid.
    let body = br#"{"eventType":"Download","movie":{"id":1}}"#.to_vec();
    let status =
        post_raw(&app, "/api/v1/webhook/does-not-exist/tok", body, "application/json").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_real_event_against_a_reachable_arr_completes() {
    // The counterpart to accepting 502 above: pointed at an Arr that answers,
    // a legitimate event runs the whole handler to a 200. If this ever fails,
    // the 502s the fuzzer tolerates would be masking a genuine handler fault.
    let arr = FakeArr::start().await;
    let app = TestApp::new().await;
    app.seed_instance_at("inst-1", "radarr", &arr.base_url).await;

    let body =
        br#"{"eventType":"Download","movie":{"id":10,"tmdbId":8392,"title":"Totoro"}}"#.to_vec();
    let status = post_raw(&app, "/api/v1/webhook/inst-1/tok", body, "application/json").await;
    assert_eq!(status, StatusCode::OK);
}
