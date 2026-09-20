//! Shared outbound HTTP client.
//!
//! One client is built at startup and cloned everywhere: it carries the
//! connection pool, so a client per request costs a TCP and TLS handshake on
//! every sync, health check and metadata fetch.

use std::time::Duration;

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// Whether a redirect stays on the service the user configured.
///
/// The Arr credential travels in a custom `X-Api-Key` header. reqwest strips
/// `Authorization` and `Cookie` across hosts but cannot know a custom header is
/// sensitive, so a redirect would forward the key verbatim.
///
/// **All three of scheme, host and port.** Host alone is not enough:
/// self-hosted services commonly share one host on different ports, where
/// host-only matching would carry the key between them. And scheme alone was
/// missing here until this function was extracted — `port_or_known_default()`
/// answers `Some(8080)` for both `https://arr:8080` and `http://arr:8080`, so a
/// downgrade to cleartext on an explicit port read as the same origin and the
/// key went out unencrypted. The one exception is an `http` → `https` upgrade
/// that keeps the port, or goes from 80 to 443 — the two shapes an upgrade
/// takes. Any other port change is another service on the same host, however
/// the scheme reads.
///
/// A free function rather than a closure so it can be tested without a TLS
/// server: the property is about three fields of two URLs, and nothing else.
fn stays_on_origin(previous: &reqwest::Url, to: &reqwest::Url) -> bool {
    let same_host = match (previous.host_str(), to.host_str()) {
        (Some(from), Some(to)) => from.eq_ignore_ascii_case(to),
        _ => false,
    };
    if !same_host {
        return false;
    }

    let (from_port, to_port) = (previous.port_or_known_default(), to.port_or_known_default());

    // The exception, and only the two shapes it actually describes: the
    // canonical 80 → 443, and the same explicit port served both ways. Written
    // as "any http → any https" it let `http://nas:7878` reach
    // `https://nas:8384` — a *different service on the same host*, which is
    // exactly the hazard the port comparison exists for, and which the word
    // "upgrade" made look safe.
    let tls_upgrade = previous.scheme() == "http"
        && to.scheme() == "https"
        && (from_port == to_port || (from_port == Some(80) && to_port == Some(443)));

    let same_origin = previous.scheme() == to.scheme() && from_port == to_port;

    same_origin || tls_upgrade
}

/// Follow redirects only while [`stays_on_origin`] holds.
fn same_origin_only() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let Some(previous) = attempt.previous().last() else {
            return attempt.stop();
        };

        if !stays_on_origin(previous, attempt.url()) {
            return attempt.stop();
        }
        if attempt.previous().len() > 5 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// Build the process-wide HTTP client.
///
/// Bounded by `ROUTARR_HTTP_TIMEOUT_SECS`: an unresponsive Arr would otherwise
/// hang a request handler, and with it the health page.
///
/// Fails rather than falling back. `Client::new()` carries reqwest's defaults —
/// follow up to ten redirects anywhere, and no timeout at all — so a fallback
/// would quietly discard both properties this function exists to set, on the
/// one path that sends an Arr credential. A server that cannot build its HTTP
/// client has nothing useful to do afterwards.
pub fn build_client(config: &Config) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(config.http_timeout)
        .redirect(same_origin_only())
        // The connect phase must not outlast the overall budget, or a timeout
        // shorter than the connect timeout has no effect.
        .connect_timeout(config.http_timeout.min(Duration::from_secs(5)))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .user_agent(concat!("Routarr/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AppError::Config(format!("cannot build the HTTP client: {e}")))
}

#[cfg(test)]
mod tests {
    use super::stays_on_origin;

    fn judge(from: &str, to: &str) -> bool {
        stays_on_origin(&from.parse().expect("from"), &to.parse().expect("to"))
    }

    /// The case this function was extracted for. `port_or_known_default()`
    /// answers `Some(8080)` on both sides, so before the scheme was compared
    /// this read as the same origin and `X-Api-Key` went out in cleartext.
    #[test]
    fn a_downgrade_to_cleartext_on_the_same_port_is_refused() {
        assert!(!judge("https://arr:8080/a", "http://arr:8080/b"));
        assert!(!judge("https://arr/a", "http://arr/b"));
    }

    /// The one exception: it strengthens the channel.
    /// Only where the upgrade is one: the default pair, or the same explicit
    /// port served both ways.
    #[test]
    fn an_upgrade_to_tls_is_followed() {
        assert!(judge("http://arr/a", "https://arr/b"));
        assert!(judge("http://arr:80/a", "https://arr:443/b"));
        assert!(judge("http://arr:8080/a", "https://arr:8080/b"));
    }

    /// "Upgrade" is not a licence to change service. Written as any http → any
    /// https, this carried `X-Api-Key` from an Arr on one port to whatever
    /// answers on another — on the host where every homelab service lives.
    #[test]
    fn an_upgrade_to_another_port_is_not_an_upgrade() {
        assert!(!judge("http://nas:7878/a", "https://nas:8384/b"));
        assert!(!judge("http://nas/a", "https://nas:8384/b"));
        assert!(!judge("http://nas:7878/a", "https://nas/b"));
    }

    #[test]
    fn the_same_origin_is_followed_whatever_the_path() {
        assert!(judge("http://arr:7878/api/v3/movie", "http://arr:7878/login"));
        assert!(judge("https://arr:8080/a", "https://ARR:8080/b"));
    }

    /// Every homelab service is on one host behind a different port, so the
    /// port is the boundary that matters most here.
    #[test]
    fn another_host_or_another_port_stops() {
        assert!(!judge("http://arr:7878/a", "http://evil.example/b"));
        assert!(!judge("http://arr:7878/a", "http://arr:8989/b"));
        assert!(!judge("https://arr:8080/a", "https://arr:9090/b"));
    }
}
