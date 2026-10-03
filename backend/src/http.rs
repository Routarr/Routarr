//! Shared outbound HTTP client.
//!
//! One client is built at startup and cloned everywhere: it carries the
//! connection pool, so a client per request costs a TCP and TLS handshake on
//! every sync, health check and metadata fetch.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// Whether an address is link-local: 169.254.0.0/16 and fe80::/10, the IPv4
/// form inside an IPv6 one included. A cloud host's metadata service answers
/// there with the machine's own credentials, and no Arr, metadata source,
/// identity provider or notification receiver does, so nothing Routarr sends
/// goes there, whoever typed the address.
pub(crate) fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_unicast_link_local() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_link_local())
        }
    }
}

/// A connection refused for a link-local address, which the transport
/// description finds in the error chain.
#[derive(Debug)]
pub(crate) struct LinkLocal;

impl std::fmt::Display for LinkLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the address is link-local")
    }
}

impl std::error::Error for LinkLocal {}

/// The addresses a name resolved to that Routarr connects to, or `LinkLocal`
/// when every one of them is link-local.
fn reachable(resolved: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, LinkLocal> {
    let kept: Vec<SocketAddr> =
        resolved.iter().copied().filter(|a| !is_link_local(a.ip())).collect();
    if kept.is_empty() && !resolved.is_empty() { Err(LinkLocal) } else { Ok(kept) }
}

/// The system resolver, without the link-local addresses. Checked when the
/// connection is made, since a name typed today can resolve anywhere
/// tomorrow. A literal address never reaches a resolver, and is checked where
/// every request is sent (`integrations::check_status`).
struct NoLinkLocal;

impl reqwest::dns::Resolve for NoLinkLocal {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let resolved: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let kept = reachable(resolved)?;
            Ok(Box::new(kept.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Whether a redirect stays on the service the user configured.
///
/// The Arr credential travels in a custom `X-Api-Key` header. reqwest strips
/// `Authorization` and `Cookie` across hosts but cannot know a custom header is
/// sensitive, so a redirect would forward the key verbatim.
///
/// **All three of scheme, host and port.** Host alone is not enough:
/// self-hosted services commonly share one host on different ports, where
/// host-only matching would carry the key between them. The scheme counts too:
/// `port_or_known_default()` answers `Some(8080)` for both `https://arr:8080`
/// and `http://arr:8080`, so without it a downgrade to cleartext on an explicit
/// port reads as the same origin and the key goes out unencrypted. The one
/// exception is an `http` → `https` upgrade that keeps the port, or goes from
/// 80 to 443, the two shapes an upgrade takes. Any other port change is another
/// service on the same host, however the scheme reads.
///
/// A free function rather than a closure so it can be tested without a TLS
/// server: the property is about three fields of two URLs, and nothing else.
pub(crate) fn stays_on_origin(previous: &reqwest::Url, to: &reqwest::Url) -> bool {
    let same_host = match (previous.host_str(), to.host_str()) {
        (Some(from), Some(to)) => from.eq_ignore_ascii_case(to),
        _ => false,
    };
    if !same_host {
        return false;
    }

    let (from_port, to_port) = (previous.port_or_known_default(), to.port_or_known_default());

    // The exception, and only the two shapes it actually describes: the
    // canonical 80 → 443, and the same explicit port served both ways. Read as
    // "any http → any https", it would let `http://nas:7878` reach
    // `https://nas:8384`, a *different service on the same host*, which is
    // exactly the hazard the port comparison exists for, and which the word
    // "upgrade" makes look safe.
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
/// Fails rather than falling back. `Client::new()` carries reqwest's defaults
/// (follow up to ten redirects anywhere, and no timeout at all), so a fallback
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
        .dns_resolver(Arc::new(NoLinkLocal))
        .build()
        .map_err(|e| AppError::Config(format!("cannot build the HTTP client: {e}")))
}

/// What an address shows in place of the `user:pass` it carries.
///
/// reqwest sends those as Basic auth, which is how Routarr reaches an Arr
/// behind a proxy that asks for one, so a stored address may carry a password.
/// It is answered, quoted and logged masked, and the instance form sends the
/// mask back to mean "keep the stored ones".
pub const MASKED_CREDENTIALS: &str = "***";

/// An address cut around its `user:pass`: what comes before, the credentials,
/// and what follows their `@`. Read as text between `://` and the last `@` of
/// the authority, so an address the URL parser refuses is masked all the same.
fn credentials(address: &str) -> Option<(&str, &str, &str)> {
    let start = address.find("://")? + 3;
    let authority = &address[start..];
    let authority = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    let at = start + authority.rfind('@')?;
    Some((&address[..start], &address[start..at], &address[at + 1..]))
}

/// The address with its credentials masked, for whatever a person reads.
pub fn masked(address: &str) -> std::borrow::Cow<'_, str> {
    match credentials(address) {
        Some((head, _, tail)) => format!("{head}{MASKED_CREDENTIALS}@{tail}").into(),
        None => address.into(),
    }
}

/// The address without its credentials, for what leaves the installation.
pub fn without_credentials(address: &str) -> std::borrow::Cow<'_, str> {
    match credentials(address) {
        Some((head, _, tail)) => format!("{head}{tail}").into(),
        None => address.into(),
    }
}

/// `typed` with the credentials of `saved` in place of the mask, or `None`
/// when `typed` does not carry the mask.
pub fn with_saved_credentials(typed: &str, saved: &str) -> Option<String> {
    let (head, shown, tail) = credentials(typed)?;
    if shown != MASKED_CREDENTIALS {
        return None;
    }
    Some(match credentials(saved) {
        Some((_, kept, _)) => format!("{head}{kept}@{tail}"),
        None => format!("{head}{tail}"),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_link_local, masked, reachable, stays_on_origin, with_saved_credentials,
        without_credentials,
    };

    /// The link-local ranges, the IPv4 one written inside an IPv6 address
    /// too, and none of the loopback and private ranges where Arrs live.
    #[test]
    fn link_local_addresses_and_only_those_are_refused() {
        for refused in ["169.254.169.254", "169.254.0.1", "fe80::1", "::ffff:169.254.169.254"] {
            assert!(is_link_local(refused.parse().unwrap()), "{refused}");
        }
        for kept in ["127.0.0.1", "10.0.0.5", "172.17.0.2", "192.168.1.20", "::1", "fd00::5"] {
            assert!(!is_link_local(kept.parse().unwrap()), "{kept}");
        }
        let at = |ip: &str| std::net::SocketAddr::new(ip.parse().unwrap(), 0);
        assert!(reachable(vec![at("169.254.169.254")]).is_err());
        assert_eq!(
            reachable(vec![at("169.254.169.254"), at("10.0.0.5")]).unwrap(),
            [at("10.0.0.5")]
        );
    }

    /// Credentials sit in the authority only: an `@` further on is part of a
    /// path or a query, and an address the URL parser refuses, with a port out
    /// of range here, is masked all the same.
    #[test]
    fn only_the_authority_carries_credentials() {
        assert_eq!(masked("http://user:pw@nas:7878/radarr"), "http://***@nas:7878/radarr");
        assert_eq!(masked("http://user:pw@nas:99999"), "http://***@nas:99999");
        assert_eq!(masked("http://nas:7878/a@b?c=@"), "http://nas:7878/a@b?c=@");
        assert_eq!(without_credentials("https://user@nas/radarr"), "https://nas/radarr");
        assert_eq!(masked("nas:7878"), "nas:7878");
    }

    #[test]
    fn only_the_mask_brings_the_saved_credentials_back() {
        let saved = "http://user:pw@nas:7878";
        assert_eq!(with_saved_credentials("http://***@nas:7878", saved).as_deref(), Some(saved));
        assert_eq!(with_saved_credentials("http://nas:7878", saved), None);
        assert_eq!(with_saved_credentials("http://other:pw2@nas:7878", saved), None);
        assert_eq!(
            with_saved_credentials("http://***@nas:7878", "http://nas:7878").as_deref(),
            Some("http://nas:7878")
        );
    }

    fn judge(from: &str, to: &str) -> bool {
        stays_on_origin(&from.parse().expect("from"), &to.parse().expect("to"))
    }

    /// `port_or_known_default()` answers `Some(8080)` on both sides, so only the
    /// scheme tells them apart. Read as the same origin, the redirect sends
    /// `X-Api-Key` in cleartext.
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

    /// "Upgrade" is not a licence to change service. Any http → any https would
    /// carry `X-Api-Key` from an Arr on one port to whatever answers on another,
    /// on the host where every homelab service lives.
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
