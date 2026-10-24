//! What a caller can make the server spend: connections held open, bodies
//! that never end.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::TestApp;

/// The production listener on a port of the loopback, its header deadline
/// shortened to what a test can wait for.
async fn listening(header_read_timeout: Duration) -> std::net::SocketAddr {
    let limits = crate::listener::Limits::of(&crate::config::Config::for_tests());
    listening_within(header_read_timeout, limits).await
}

/// The same, serving no more than `limits`.
async fn listening_within(
    header_read_timeout: Duration,
    limits: crate::listener::Limits,
) -> std::net::SocketAddr {
    let app = TestApp::new().await;
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    tokio::spawn(crate::listener::serve(
        socket,
        app.router.clone(),
        std::future::pending(),
        std::future::pending(),
        header_read_timeout,
        crate::listener::DRAIN,
        limits,
    ));
    address
}

/// A connection to `address` from `from`, an address of the loopback.
async fn connected_from(from: [u8; 4], address: std::net::SocketAddr) -> tokio::net::TcpStream {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(std::net::SocketAddr::from((from, 0))).unwrap();
    socket.connect(address).await.unwrap()
}

/// A connection from `from` that asked once and stays open, as a browser's does.
async fn kept_open_from(from: [u8; 4], address: std::net::SocketAddr) -> tokio::net::TcpStream {
    let mut stream = connected_from(from, address).await;
    stream.write_all(b"GET /api/v1/ping HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let mut chunk = [0u8; 1024];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk)).await;
    let answer = String::from_utf8_lossy(&chunk[..read.expect("no answer").unwrap()]).into_owned();
    assert!(answer.starts_with("HTTP/1.1 200"), "{from:?}: {answer}");
    stream
}

/// What a ping from `from` is answered, `""` when the connection is closed unanswered and
/// `None` when it is still waiting after two seconds.
async fn ping_from(from: [u8; 4], address: std::net::SocketAddr) -> Option<String> {
    let mut stream = connected_from(from, address).await;
    let asked = stream
        .write_all(b"GET /api/v1/ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await;
    if asked.is_err() {
        return Some(String::new());
    }
    until_closed(&mut stream, Duration::from_secs(2)).await
}

/// One client keeping its connections open would take every place. Past half of them taken, an
/// address holds its share and no more, and below, as many as it likes: behind Docker Desktop or
/// rootless Docker every client reaches Routarr from one address.
#[tokio::test]
async fn one_client_cannot_hold_every_connection_place() {
    let limits = crate::listener::Limits { connections: 8, per_client: 2, proxies: Vec::new() };
    let address = listening_within(crate::listener::HEADER_READ_TIMEOUT, limits).await;

    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(kept_open_from([127, 0, 0, 2], address).await);
    }

    assert_eq!(ping_from([127, 0, 0, 2], address).await.as_deref(), Some(""));
    let other = ping_from([127, 0, 0, 3], address).await.unwrap_or_default();
    assert!(other.starts_with("HTTP/1.1 200"), "another client was refused: {other}");
}

/// A reverse proxy carries every client from one address, so it holds no share of its own.
#[tokio::test]
async fn a_trusted_proxy_holds_more_than_a_share() {
    let proxy = crate::config::Network::parse("127.0.0.2").unwrap();
    let limits = crate::listener::Limits { connections: 8, per_client: 2, proxies: vec![proxy] };
    let address = listening_within(crate::listener::HEADER_READ_TIMEOUT, limits).await;

    let mut held = Vec::new();
    for _ in 0..6 {
        held.push(kept_open_from([127, 0, 0, 2], address).await);
    }
    let seventh = ping_from([127, 0, 0, 2], address).await.unwrap_or_default();
    assert!(seventh.starts_with("HTTP/1.1 200"), "the proxy was held to a share: {seventh}");
}

/// The image's health check asks `127.0.0.1`, and an unhealthy container is restarted by an
/// orchestrator while the peer holding every place keeps them: a few places stay its own.
#[tokio::test]
async fn the_health_check_is_answered_when_every_place_is_taken() {
    let limits = crate::listener::Limits { connections: 8, per_client: 4, proxies: Vec::new() };
    let address = listening_within(crate::listener::HEADER_READ_TIMEOUT, limits).await;

    let mut held = Vec::new();
    for from in [2, 3, 4, 5] {
        for _ in 0..2 {
            held.push(kept_open_from([127, 0, 0, from], address).await);
        }
    }

    let stranger = ping_from([127, 0, 0, 6], address).await;
    assert!(stranger.as_deref().is_none_or(str::is_empty), "a ninth was served: {stranger:?}");
    let probe = ping_from([127, 0, 0, 1], address).await.unwrap_or_default();
    assert!(probe.starts_with("HTTP/1.1 200"), "the health check waited: {probe}");
}

/// Everything the server sends on `stream` until it closes it, or `None` when
/// it is still open after `wait`.
async fn until_closed(stream: &mut tokio::net::TcpStream, wait: Duration) -> Option<String> {
    let mut received = Vec::new();
    let read = tokio::time::timeout(wait, async {
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => received.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await;
    read.ok().map(|()| String::from_utf8_lossy(&received).into_owned())
}

/// Kept, such a peer holds a task and a descriptor until the process has none left.
#[tokio::test]
async fn a_client_that_never_finishes_its_headers_is_disconnected() {
    let deadline = Duration::from_millis(200);
    let address = listening(deadline).await;

    let mut whole = tokio::net::TcpStream::connect(address).await.unwrap();
    whole
        .write_all(b"GET /api/v1/ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let answer = until_closed(&mut whole, Duration::from_secs(5)).await.expect("no answer");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    stalled.write_all(b"GET /api/v1/ping HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    assert!(
        until_closed(&mut stalled, deadline + Duration::from_secs(2)).await.is_some(),
        "a connection that never finished its headers was kept open"
    );
}

/// Without a deadline, a body sent a byte at a time holds its connection for good.
#[tokio::test]
async fn a_body_that_stops_arriving_is_refused() {
    let app = TestApp::new().await;
    // Paused once the database is open, whose pool would time out under it.
    tokio::time::pause();
    let trickle =
        futures::stream::once(async { Ok::<_, std::io::Error>(axum::body::Bytes::from("{")) })
            .chain(futures::stream::pending());
    let request = Request::post("/api/v1/simulate")
        .header("content-type", "application/json")
        .body(Body::from_stream(trickle))
        .unwrap();

    let answered = tokio::time::timeout(crate::BODY_DEADLINE * 2, app.send(request))
        .await
        .expect("a request whose body never ended was still being read at twice the deadline");
    assert_eq!(answered.status, StatusCode::REQUEST_TIMEOUT, "{}", answered.json);
    assert_eq!(answered.json["error"], "request_timeout", "{}", answered.json);
}

/// The anime table may weigh 64 MiB: it is given ten minutes to arrive, past which it answers
/// as any body that stops arriving.
#[tokio::test]
async fn the_anime_table_has_ten_minutes_to_arrive() {
    let app = TestApp::new().await;
    tokio::time::pause();
    let trickle =
        futures::stream::once(async { Ok::<_, std::io::Error>(axum::body::Bytes::from("[")) })
            .chain(futures::stream::pending());
    let request = Request::post("/api/v1/sources/anime-ids/import")
        .header("content-type", "application/json")
        .body(Body::from_stream(trickle))
        .unwrap();

    let started = tokio::time::Instant::now();
    let answered = tokio::time::timeout(crate::IMPORT_BODY_DEADLINE * 2, app.send(request))
        .await
        .expect("the upload was still being read at twice its deadline");
    assert!(started.elapsed() >= crate::IMPORT_BODY_DEADLINE, "refused at {:?}", started.elapsed());
    assert_eq!(answered.status, StatusCode::REQUEST_TIMEOUT, "{}", answered.json);
    assert_eq!(answered.json["error"], "request_timeout", "{}", answered.json);
}

/// An answer may carry a key, a webhook token or an archive holding the master key.
#[tokio::test]
async fn every_api_answer_says_not_to_store_it() {
    let app = TestApp::with_api_key("s3cret").await;
    for request in [
        Request::get("/api/v1/ping").body(Body::empty()).unwrap(),
        Request::get("/api/v1/settings").header("x-api-key", "s3cret").body(Body::empty()).unwrap(),
        Request::get("/api/v1/settings").body(Body::empty()).unwrap(),
        Request::get("/api/v1/no-such-route").body(Body::empty()).unwrap(),
    ] {
        let path = request.uri().to_string();
        let response = app.send_raw(request).await;
        assert_eq!(
            response.headers().get(axum::http::header::CACHE_CONTROL).map(|v| v.as_bytes()),
            Some(&b"no-store"[..]),
            "{path} answered {}",
            response.status()
        );
    }
}
