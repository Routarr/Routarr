//! Accepting connections, and serving each one with a deadline on its headers.
//!
//! The port is often published straight to the network: a peer that never finishes its
//! headers, or keeps its connections busy, would hold every place. A slow body is bounded by
//! [`crate::BODY_DEADLINE`].

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
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

use crate::services::accounts::{ClientShare, bucket};

/// How long a client has to send a request's headers, the first one and each
/// one after on a kept-alive connection.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the open requests are given once a stop is asked. The work in
/// flight and the database are given the rest of the Compose file's 30-second
/// grace, which a runtime ends with SIGKILL.
pub const DRAIN: Duration = Duration::from_secs(5);

/// How many connections are served at once.
const MAX_CONNECTIONS: usize = 512;

/// How many of them one client holds once half are taken, far above the six a browser opens.
const PER_CLIENT_CONNECTIONS: usize = 32;

/// The places kept for `routarr healthcheck` once every other is taken.
const HEALTH_CHECK_PLACES: usize = 4;

/// What the listener serves at once.
#[derive(Debug, Clone)]
pub struct Limits {
    pub connections: usize,
    /// What one address, an IPv6 one by its /64, holds once half the connections are taken.
    pub per_client: usize,
    /// The reverse proxies of `ROUTARR_TRUSTED_PROXIES`, which carry every client and hold
    /// no share.
    pub proxies: Vec<crate::config::Network>,
}

impl Limits {
    pub fn of(config: &crate::config::Config) -> Self {
        Self {
            connections: MAX_CONNECTIONS,
            per_client: PER_CLIENT_CONNECTIONS,
            proxies: config.trusted_proxies.clone(),
        }
    }
}

/// The places a connection takes, and the share of its client.
struct Places {
    open: Arc<Semaphore>,
    health_check: Arc<Semaphore>,
    by_client: Arc<Mutex<HashMap<IpAddr, usize>>>,
    limits: Limits,
}

/// What a served connection holds until it ends.
struct Place {
    _permit: OwnedSemaphorePermit,
    _share: Option<ClientShare>,
}

impl Places {
    fn new(limits: Limits) -> Self {
        Self {
            open: Arc::new(Semaphore::new(limits.connections)),
            health_check: Arc::new(Semaphore::new(HEALTH_CHECK_PLACES)),
            by_client: Arc::default(),
            limits,
        }
    }

    /// The place `peer` is served in, or `None` to close it unanswered. Below half the places
    /// taken nobody is held to a share, so where every client shows one address, as behind
    /// Docker Desktop or rootless Docker, nobody is refused short of that.
    fn admit(&self, permit: Permit, peer: IpAddr) -> Option<Place> {
        let permit = match permit {
            Permit::Open(permit) => permit,
            Permit::HealthCheck(permit) => {
                return health_check(peer).then_some(Place { _permit: permit, _share: None });
            }
        };
        if self.limits.proxies.iter().any(|proxy| proxy.contains(peer)) {
            return Some(Place { _permit: permit, _share: None });
        }
        let taken = self.limits.connections - self.open.available_permits() - 1;
        let crowded = taken >= self.limits.connections / 2;
        let at_most = if crowded { self.limits.per_client } else { usize::MAX };
        let share = ClientShare::take(&self.by_client, bucket(peer), at_most)?;
        Some(Place { _permit: permit, _share: Some(share) })
    }
}

/// Where the image's health check comes from. It reaches the loopback only while the server
/// binds every address (`Config::ping_url`), as the image does: bound to one address, the
/// probe goes there.
fn health_check(peer: IpAddr) -> bool {
    matches!(peer.to_canonical(), IpAddr::V4(Ipv4Addr::LOCALHOST) | IpAddr::V6(Ipv6Addr::LOCALHOST))
}

enum Permit {
    Open(OwnedSemaphorePermit),
    HealthCheck(OwnedSemaphorePermit),
}

/// Serve `app` until `shutdown` resolves, then let the open connections
/// finish, for `drain` at most or until `hurry` resolves. A probe of a host
/// that never answers, or a client that never reads, would otherwise hold the
/// stop open past any grace.
///
/// Every request carries the peer's address as `ConnectInfo`, which the
/// sign-in throttle and the security log read.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
    hurry: impl Future<Output = ()>,
    header_read_timeout: Duration,
    drain: Duration,
    limits: Limits,
) {
    let places = Places::new(limits);
    let graceful = GracefulShutdown::new();
    let mut builder = http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(header_read_timeout);

    tokio::pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            accepted = accept(&listener, &places) => accepted,
            () = &mut shutdown => break,
        };
        let Some((permit, stream, peer)) = accepted else { continue };
        let Some(place) = places.admit(permit, peer.ip()) else {
            tracing::debug!("A connection from {peer} was closed: its client holds its share");
            continue;
        };

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
            drop(place);
        });
    }
    let cut = async {
        tokio::select! {
            () = tokio::time::sleep(drain) => {}
            () = hurry => {}
        }
    };
    tokio::select! {
        () = graceful.shutdown() => {}
        () = cut => tracing::warn!("Requests still open were cut for Routarr to stop"),
    }
}

/// The next connection, once a place is free, else one kept for the health check, which waits
/// in the kernel's backlog meanwhile. `None` when the accept failed, after a pause when the
/// failure is the process's: out of descriptors, accepting again at once only spins.
async fn accept(
    listener: &TcpListener,
    places: &Places,
) -> Option<(Permit, TcpStream, SocketAddr)> {
    let permit = tokio::select! {
        biased;
        permit = Arc::clone(&places.open).acquire_owned() => Permit::Open(permit.ok()?),
        permit = Arc::clone(&places.health_check).acquire_owned() => {
            Permit::HealthCheck(permit.ok()?)
        }
    };
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

/// The stop, said at once to the server and the scheduler: `first` stops
/// both, and `second`, awaited only once `first` has resolved, skips what the
/// stop waits for.
pub fn stop_on(
    first: impl Future<Output = ()> + Send + 'static,
    second: impl Future<Output = ()> + Send + 'static,
) -> (tokio::sync::watch::Receiver<bool>, tokio::sync::watch::Receiver<bool>) {
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let (hurry, hurrying) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        first.await;
        let _ = stop.send(true);
        second.await;
        tracing::warn!("A second signal: stopping without waiting");
        let _ = hurry.send(true);
    });
    (stopping, hurrying)
}

/// Resolves once `flag` is raised.
pub async fn fired(mut flag: tokio::sync::watch::Receiver<bool>) {
    if flag.wait_for(|raised| *raised).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// How long the process waits, once its work is done, for the blocking threads still running:
/// a copy blocked on a share that hangs never ends, and the process ends without it.
pub const ABANDON: Duration = Duration::from_secs(2);

/// End the process's runtime, giving its blocking threads `grace` and no more. A runtime merely
/// dropped waits for every one of them without end.
pub fn end(runtime: tokio::runtime::Runtime, grace: Duration) {
    runtime.shutdown_timeout(grace);
}

/// `work`, or `None` once a stop is asked first, `work` then dropped where it waits. Only for
/// work a kill would leave safe: what it leaves half done, the next start sweeps or resumes.
pub async fn until_stopped<T>(
    work: impl Future<Output = T>,
    stopping: tokio::sync::watch::Receiver<bool>,
) -> Option<T> {
    tokio::select! {
        done = work => Some(done),
        () = fired(stopping) => None,
    }
}
