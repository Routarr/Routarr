//! A stop that fits the grace a runtime gives it, whatever is still open.

use std::time::Duration;

use axum::Router;
use axum::routing::get;
use tokio::io::AsyncWriteExt;

/// A server whose one route answers after a minute, as a probe of a host that
/// never answers does, stopped by `stop` and hurried by `hurry`.
async fn serving_a_slow_request(
    stop: tokio::sync::oneshot::Receiver<()>,
    hurry: tokio::sync::oneshot::Receiver<()>,
    drain: Duration,
) -> tokio::task::JoinHandle<()> {
    let app = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            "late"
        }),
    );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let server = tokio::spawn(crate::listener::serve(
        socket,
        app,
        async move {
            let _ = stop.await;
        },
        async move {
            let _ = hurry.await;
        },
        crate::listener::HEADER_READ_TIMEOUT,
        drain,
    ));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(client);
    });
    // The request reaches the route before the stop is asked.
    tokio::time::sleep(Duration::from_millis(100)).await;
    server
}

/// A request still open when the stop comes is given the drain and no more:
/// under Docker a stop that waits for it ends in SIGKILL, before the database
/// is checkpointed.
#[tokio::test]
async fn a_stop_answers_within_its_drain_even_with_a_request_open() {
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let (_hurry, hurried) = tokio::sync::oneshot::channel();
    let server = serving_a_slow_request(stopped, hurried, Duration::from_millis(300)).await;

    let asked = std::time::Instant::now();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("the stop waited").unwrap();
    assert!(asked.elapsed() < Duration::from_millis(1300), "{:?}", asked.elapsed());
}

/// A second signal cuts the drain short.
#[tokio::test]
async fn a_second_signal_cuts_the_drain_short() {
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let (hurry, hurried) = tokio::sync::oneshot::channel();
    let server = serving_a_slow_request(stopped, hurried, Duration::from_secs(60)).await;

    stop.send(()).unwrap();
    hurry.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("the stop waited").unwrap();
}

/// The first signal reaches the server and the scheduler together, so no pass
/// starts while the requests drain, and only a second hurries the stop.
#[tokio::test]
async fn the_first_signal_stops_everything_at_once_and_a_second_hurries() {
    let (first, first_heard) = tokio::sync::oneshot::channel::<()>();
    let (second, second_heard) = tokio::sync::oneshot::channel::<()>();
    let (mut stopping, mut hurrying) = crate::stop_on(
        async move {
            let _ = first_heard.await;
        },
        async move {
            let _ = second_heard.await;
        },
    );
    assert!(!*stopping.borrow());

    first.send(()).unwrap();
    let heard = tokio::time::timeout(Duration::from_secs(1), stopping.wait_for(|raised| *raised));
    heard.await.expect("the first signal stopped nothing").unwrap();
    assert!(!*hurrying.borrow(), "one signal hurried the stop");

    second.send(()).unwrap();
    let heard = tokio::time::timeout(Duration::from_secs(1), hurrying.wait_for(|raised| *raised));
    heard.await.expect("the second signal hurried nothing").unwrap();
}
