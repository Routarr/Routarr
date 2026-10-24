//! Shared outbound HTTP client, built once and cloned everywhere: it carries the connection
//! pool, so a client per request costs a TCP and TLS handshake each time.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// How long a whole library may take to list by default, see
/// `Config::library_timeout`.
pub const LIBRARY_TIMEOUT: Duration = Duration::from_secs(300);

/// Where Alibaba Cloud's metadata service answers, in the shared address space.
const ALIBABA_METADATA: Ipv4Addr = Ipv4Addr::new(100, 100, 100, 200);
/// Where AWS's answers over IPv6, a unique local address.
const AWS_METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);

/// Whether a cloud host's metadata service can answer at an address: the link-local ranges
/// (169.254.0.0/16, fe80::/10) and the two that answer elsewhere, each also written inside an
/// IPv6 address. It hands out the machine's credentials, and nothing Routarr calls answers there.
pub(crate) fn is_metadata_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_metadata_v4(v4),
        IpAddr::V6(v6) => {
            v6.is_unicast_link_local()
                || v6 == AWS_METADATA_V6
                || v6.to_ipv4_mapped().is_some_and(is_metadata_v4)
                || through_nat64(v6).is_some_and(is_metadata_v4)
        }
    }
}

fn is_metadata_v4(v4: Ipv4Addr) -> bool {
    v4.is_link_local() || v4 == ALIBABA_METADATA
}

/// The IPv4 address a NAT64 gateway reaches for `v6`, under the well-known
/// prefix 64:ff9b::/96.
fn through_nat64(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let [a, b, c, d, e, f, high, low] = v6.segments();
    let [h1, h2] = high.to_be_bytes();
    let [l1, l2] = low.to_be_bytes();
    ([a, b, c, d, e, f] == [0x64, 0xff9b, 0, 0, 0, 0]).then(|| Ipv4Addr::new(h1, h2, l1, l2))
}

/// A connection refused for a metadata address, which the transport
/// description finds in the error chain.
#[derive(Debug)]
pub(crate) struct MetadataAddress;

impl std::fmt::Display for MetadataAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a cloud host's metadata service answers at the address")
    }
}

impl std::error::Error for MetadataAddress {}

/// The addresses a name resolved to that Routarr connects to, or
/// `MetadataAddress` when every one of them is a metadata address.
fn reachable(resolved: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, MetadataAddress> {
    let kept: Vec<SocketAddr> =
        resolved.iter().copied().filter(|a| !is_metadata_address(a.ip())).collect();
    if kept.is_empty() && !resolved.is_empty() { Err(MetadataAddress) } else { Ok(kept) }
}

/// The system resolver, without the metadata addresses. Checked when the
/// connection is made, since a name typed today can resolve anywhere
/// tomorrow. A literal address never reaches a resolver, and is checked where
/// every request is sent (`integrations::check_status`).
struct NoMetadataAddresses;

impl reqwest::dns::Resolve for NoMetadataAddresses {
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

/// Whether a redirect stays on the service the user configured, so `X-Api-Key`, which reqwest
/// cannot know is sensitive, never follows it elsewhere.
///
/// Scheme, host and port all count: homelab services share a host on different ports, and
/// `port_or_known_default()` reads `https://arr:8080` and `http://arr:8080` alike. The one
/// exception is an `http` to `https` upgrade, below.
pub(crate) fn stays_on_origin(previous: &reqwest::Url, to: &reqwest::Url) -> bool {
    let same_host = match (previous.host_str(), to.host_str()) {
        (Some(from), Some(to)) => from.eq_ignore_ascii_case(to),
        _ => false,
    };
    if !same_host {
        return false;
    }

    let (from_port, to_port) = (previous.port_or_known_default(), to.port_or_known_default());

    // Only the canonical 80 to 443 and the same explicit port served both ways: any http to any
    // https would let `http://nas:7878` reach `https://nas:8384`, another service.
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

/// Build the process-wide HTTP client, bounded by `ROUTARR_HTTP_TIMEOUT_SECS`.
///
/// Fails rather than falling back to `Client::new()`, which follows ten redirects anywhere
/// with no timeout, on the path that sends an Arr credential.
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
        .dns_resolver(Arc::new(NoMetadataAddresses))
        .build()
        .map_err(|e| AppError::Config(format!("cannot build the HTTP client: {e}")))
}

/// What an address shows in place of the `user:pass` it carries, which reqwest sends as Basic
/// auth to a proxy in front of an Arr. The instance form sends the mask back to mean "keep the
/// stored ones".
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

/// The address as a person reads it, of a file somebody else publishes: its credentials masked
/// and its query cut, where a token travels.
pub fn for_display(address: &str) -> String {
    let masked = masked(address);
    let end = masked.find(['?', '#']).unwrap_or(masked.len());
    masked[..end].to_string()
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
        is_metadata_address, masked, reachable, stays_on_origin, with_saved_credentials,
        without_credentials,
    };

    #[test]
    fn metadata_addresses_and_only_those_are_refused() {
        for refused in [
            "169.254.169.254",
            "169.254.0.1",
            "fe80::1",
            "::ffff:169.254.169.254",
            "100.100.100.200",
            "fd00:ec2::254",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::6464:64c8",
        ] {
            assert!(is_metadata_address(refused.parse().unwrap()), "{refused}");
        }
        for kept in [
            "127.0.0.1",
            "10.0.0.5",
            "172.17.0.2",
            "192.168.1.20",
            "100.64.0.1",
            "::1",
            "fd00::5",
            "64:ff9b::a00:5",
        ] {
            assert!(!is_metadata_address(kept.parse().unwrap()), "{kept}");
        }
        let at = |ip: &str| std::net::SocketAddr::new(ip.parse().unwrap(), 0);
        assert!(reachable(vec![at("169.254.169.254")]).is_err());
        assert_eq!(
            reachable(vec![at("169.254.169.254"), at("10.0.0.5")]).unwrap(),
            [at("10.0.0.5")]
        );
    }

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

    #[test]
    fn a_downgrade_to_cleartext_on_the_same_port_is_refused() {
        assert!(!judge("https://arr:8080/a", "http://arr:8080/b"));
        assert!(!judge("https://arr/a", "http://arr/b"));
    }

    #[test]
    fn an_upgrade_to_tls_is_followed() {
        assert!(judge("http://arr/a", "https://arr/b"));
        assert!(judge("http://arr:80/a", "https://arr:443/b"));
        assert!(judge("http://arr:8080/a", "https://arr:8080/b"));
    }

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

    #[test]
    fn another_host_or_another_port_stops() {
        assert!(!judge("http://arr:7878/a", "http://evil.example/b"));
        assert!(!judge("http://arr:7878/a", "http://arr:8989/b"));
        assert!(!judge("https://arr:8080/a", "https://arr:9090/b"));
    }
}
