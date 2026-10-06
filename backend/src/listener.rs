//! Accepting connections, and serving each one with a deadline on its headers.
//!
//! The port is often published straight to the network with no proxy in
//! front. A peer that opens connections and never finishes their headers
//! would otherwise hold a task and a descriptor each, until the process runs
//! out of descriptors and stops answering everyone. A body sent a byte at a
//! time is bounded by [`crate::BODY_DEADLINE`], which sits with the other layers.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower::ServiceExt;

/// How long a client has to send a request's headers, the first one and each
/// one after on a kept-alive connection.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How many connections are served at once. The next one waits in the
/// kernel's backlog rather than taking a descriptor of its own.
const MAX_CONNECTIONS: usize = 512;

/// Serve `app` until `shutdown` resolves, then let the open connections finish.
///
/// Every request carries the peer's address as `ConnectInfo`, which the
/// sign-in throttle and the security log read.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
    header_read_timeout: Duration,
) {
    let open = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let graceful = GracefulShutdown::new();
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(header_read_timeout);

    tokio::pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            accepted = accept(&listener, &open) => accepted,
            () = &mut shutdown => break,
        };
        let Some((permit, stream, peer)) = accepted else { continue };

        let service = app.clone().map_request(
            move |mut request: axum::http::Request<hyper::body::Incoming>| {
                request.extensions_mut().insert(ConnectInfo(peer));
                request.map(axum::body::Body::new)
            },
        );
        let connection = graceful.watch(
            builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(service)),
        );
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!("A connection from {peer} ended: {e}");
            }
            drop(permit);
        });
    }
    graceful.shutdown().await;
}

/// The next connection, once one of the places is free. `None` when the
/// accept failed, after a pause when the failure is the process's and not the
/// peer's: out of descriptors, accepting again at once only spins.
async fn accept(
    listener: &TcpListener,
    open: &Arc<Semaphore>,
) -> Option<(OwnedSemaphorePermit, TcpStream, SocketAddr)> {
    let permit = Arc::clone(open).acquire_owned().await.ok()?;
    match listener.accept().await {
        Ok((stream, peer)) => Some((permit, stream, peer)),
        Err(e) if is_the_peers(&e) => None,
        Err(e) => {
            tracing::error!("Could not accept a connection: {e}");
            tokio::time::sleep(Duration::from_secs(1)).await;
            None
        }
    }
}

/// A failure that ends one connection and says nothing about the next.
fn is_the_peers(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
    )
}
