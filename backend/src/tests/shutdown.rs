//! A stop that fits the grace a runtime gives it, whatever is still open.

use std::time::Duration;

use axum::Router;
use axum::routing::get;
use tokio::io::AsyncWriteExt;

/// A server whose one route answers after a minute, stopped by `stop` and hurried by `hurry`.
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
        crate::listener::Limits::of(&crate::config::Config::for_tests()),
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

/// Under Docker a stop that waits for an open request ends in SIGKILL, before the database
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

#[tokio::test]
async fn a_second_signal_cuts_the_drain_short() {
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let (hurry, hurried) = tokio::sync::oneshot::channel();
    let server = serving_a_slow_request(stopped, hurried, Duration::from_secs(60)).await;

    stop.send(()).unwrap();
    hurry.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("the stop waited").unwrap();
}

/// The scheduler hears the first signal with the server, so no pass starts while requests drain.
#[tokio::test]
async fn the_first_signal_stops_everything_at_once_and_a_second_hurries() {
    let (first, first_heard) = tokio::sync::oneshot::channel::<()>();
    let (second, second_heard) = tokio::sync::oneshot::channel::<()>();
    let (mut stopping, mut hurrying) = crate::listener::stop_on(
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

/// A copy blocked on a share that hangs holds a thread nothing can stop. The process ends within
/// its grace all the same, the runtime abandoning the thread, where dropping the runtime waits
/// for it without end.
#[test]
fn a_thread_blocked_on_a_share_never_holds_the_process_past_its_grace() {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.spawn_blocking(|| std::thread::sleep(Duration::from_secs(60)));
    let (ended, ending) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        crate::listener::end(runtime, Duration::from_secs(1));
        let _ = ended.send(());
    });

    ending.recv_timeout(Duration::from_secs(5)).expect("the stop waited for the blocked thread");
}

/// A stop asked while the start archives or migrates a large database ends the start at once:
/// in a container, a stop that goes unheard ends in SIGKILL once the grace is over.
#[tokio::test]
async fn a_stop_during_the_start_ends_it_at_once() {
    let signals = || {
        let (first, first_heard) = tokio::sync::oneshot::channel::<()>();
        let (second, second_heard) = tokio::sync::oneshot::channel::<()>();
        let (stopping, _) = crate::listener::stop_on(
            async move {
                let _ = first_heard.await;
            },
            async move {
                let _ = second_heard.await;
            },
        );
        (first, second, stopping)
    };

    let (_first, _second, stopping) = signals();
    let ended = crate::listener::until_stopped(async { "started" }, stopping).await;
    assert_eq!(ended, Some("started"), "a start nobody stopped was cut short");

    let (first, _second, stopping) = signals();
    let long =
        crate::listener::until_stopped(tokio::time::sleep(Duration::from_secs(60)), stopping);
    first.send(()).unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(2), long).await;
    assert_eq!(ended.expect("the start went on past the stop"), None);
}
