//! What a caller can make the server spend: connections held open, bodies
//! that never end.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::TestApp;

/// The production listener on a port of the loopback, its header deadline
/// shortened to what a test can wait for.
async fn listening(header_read_timeout: Duration) -> std::net::SocketAddr {
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
    ));
    address
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

/// A peer that opens a connection and never finishes its headers holds a
/// task and a descriptor for as long as it likes, until the process has none
/// left and answers nobody. It is cut at the deadline, and a request sent whole
/// on the same listener is still answered.
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

/// A body sent a byte at a time keeps its connection for as long as the
/// sender likes. Past the deadline the request is refused, whoever sent it.
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
    assert!(answered.status.is_client_error(), "{}: {}", answered.status, answered.json);
}

/// An answer may carry a key, a webhook token or an archive holding the master
/// key. None is kept by a shared browser profile or a proxy cache: an answer,
/// a refusal and an error alike say so.
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
