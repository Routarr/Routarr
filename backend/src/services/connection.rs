//! What stops Routarr reaching an Arr, told as what to do about it.
//!
//! A raw failure names the transport, "connection refused" or "HTTP 401", and
//! leaves the operator to work out which of the address, the port, the key or
//! the type they typed is wrong. Each cause names the field to change, in the
//! operator's language, since it is read under the form they typed it in.

use std::net::IpAddr;

use crate::error::AppError;
use crate::integrations::{self, Transport};
use crate::localization::Localizer;

/// Why an address did not answer as the Arr it was declared to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The host name does not resolve.
    NameUnresolved,
    /// Nothing listens at the address, or the network does not reach it.
    Unreachable,
    /// The connection was taken and nothing came back in time.
    TimedOut,
    /// The TLS handshake failed: a certificate not trusted, or `https` on a
    /// port that serves plain HTTP.
    HandshakeFailed,
    /// What answered is not HTTP, as TLS on a port given with `http`.
    NotHttp,
    /// The address redirects to another port or scheme of the same host,
    /// where the key never goes.
    Redirected,
    /// The address redirects to another host, a sign-in portal as often as
    /// the Arr.
    RedirectedElsewhere(String),
    /// Redirects that never leave the address.
    RedirectLoop,
    /// Something answers, but not the Arr's API: a web page, another port.
    NotTheApi,
    KeyRefused,
    /// Something in front of the Arr asks for its own sign-in.
    SignInInFront,
    /// The request was refused before it reached the Arr (HTTP 403).
    RefusedInFront,
    /// A proxy in front answers, and cannot reach the Arr, with this status.
    UnreachableBehind(u16),
    /// The Arr failed on its own side, with this HTTP status.
    ServerError(u16),
    /// Another application answers, named as it names itself.
    WrongApp(String),
}

impl Cause {
    /// Whether the Arr is down or failing, which a retry may get past, rather
    /// than something the operator typed, which a retry never changes.
    pub fn is_outage(&self) -> bool {
        matches!(
            self,
            Cause::NameUnresolved
                | Cause::Unreachable
                | Cause::TimedOut
                | Cause::UnreachableBehind(_)
                | Cause::ServerError(_)
        )
    }
}

/// The cause of a failed call to an Arr, when the address or the key explains it.
///
/// Anything else, a database error or a sync already running, is not about
/// what the operator typed and passes through as it is.
pub fn cause_of(error: &AppError) -> Option<Cause> {
    let AppError::ExternalApi { status, message, .. } = error else {
        return None;
    };
    match *status {
        0 => match integrations::transport_failure(message)? {
            Transport::NameUnresolved => Some(Cause::NameUnresolved),
            Transport::Unreachable => Some(Cause::Unreachable),
            Transport::TimedOut => Some(Cause::TimedOut),
            Transport::HandshakeFailed => Some(Cause::HandshakeFailed),
            Transport::NotHttp => Some(Cause::NotHttp),
            Transport::RedirectLoop => Some(Cause::RedirectLoop),
            Transport::Unreadable => Some(Cause::NotTheApi),
        },
        300..=399 => Some(match integrations::redirected_to(message) {
            Some(host) => Cause::RedirectedElsewhere(host.to_string()),
            None => Cause::Redirected,
        }),
        401 if integrations::signs_in_in_front(message) => Some(Cause::SignInInFront),
        401 => Some(Cause::KeyRefused),
        // Radarr and Sonarr answer a wrong key with 401. A 403 comes from a
        // firewall, an allow list or a sign-in policy in front of them.
        403 => Some(Cause::RefusedInFront),
        404 | 405 => Some(Cause::NotTheApi),
        502..=504 => Some(Cause::UnreachableBehind(*status)),
        500..=599 => Some(Cause::ServerError(*status)),
        _ => None,
    }
}

/// The application that answered, when it is not the one declared.
///
/// Radarr and Sonarr name themselves in `appName`. A build that names nothing
/// is taken at its word.
pub fn wrong_app(declared: &str, answered: Option<&str>) -> Option<Cause> {
    let answered = answered?.trim();
    (!answered.is_empty() && !answered.eq_ignore_ascii_case(declared))
        .then(|| Cause::WrongApp(answered.to_string()))
}

/// Whether the address points back at the machine Routarr runs on, which in
/// Docker is Routarr's own container, where no Arr listens.
pub fn is_loopback(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// The name an Arr goes by, from its declared type, `radarr` or `sonarr`.
pub fn service_name(kind: &str) -> &'static str {
    if kind.eq_ignore_ascii_case("sonarr") { "Sonarr" } else { "Radarr" }
}

/// The sentence the operator reads, naming what to change.
///
/// `kind` is the declared type, `radarr` or `sonarr`.
pub fn explain(cause: &Cause, kind: &str, base_url: &str, localizer: &Localizer) -> String {
    let service = service_name(kind);
    let url_base = format!("/{}", service.to_ascii_lowercase());
    let status;
    let (key, params): (&str, Vec<(&str, &str)>) = match cause {
        Cause::NameUnresolved => {
            ("ArrNameUnresolved", vec![("service", service), ("address", base_url)])
        }
        Cause::Unreachable if is_loopback(base_url) => {
            ("ArrUnreachableLoopback", vec![("service", service), ("address", base_url)])
        }
        Cause::Unreachable => ("ArrUnreachable", vec![("service", service), ("address", base_url)]),
        Cause::TimedOut => ("ArrTimedOut", vec![("service", service), ("address", base_url)]),
        Cause::HandshakeFailed => {
            ("ArrHandshakeFailed", vec![("service", service), ("address", base_url)])
        }
        Cause::NotHttp => ("ArrNotHttp", vec![("service", service), ("address", base_url)]),
        Cause::Redirected => ("ArrRedirected", vec![("address", base_url)]),
        Cause::RedirectedElsewhere(host) => (
            "ArrRedirectedElsewhere",
            vec![("service", service), ("address", base_url), ("host", host.as_str())],
        ),
        Cause::RedirectLoop => {
            ("ArrRedirectLoop", vec![("service", service), ("address", base_url)])
        }
        Cause::NotTheApi => (
            "ArrNotTheApi",
            vec![("service", service), ("address", base_url), ("base", url_base.as_str())],
        ),
        Cause::KeyRefused => ("ArrKeyRefused", vec![("service", service)]),
        Cause::SignInInFront => ("ArrProxySignIn", vec![("service", service)]),
        Cause::RefusedInFront => ("ArrProxyRefused", vec![("service", service)]),
        Cause::UnreachableBehind(code) => {
            status = code.to_string();
            ("ArrProxyUnreachable", vec![("service", service), ("status", status.as_str())])
        }
        Cause::ServerError(code) => {
            status = code.to_string();
            ("ArrServerError", vec![("service", service), ("status", status.as_str())])
        }
        Cause::WrongApp(found) => (
            "ArrWrongApp",
            vec![("service", service), ("address", base_url), ("found", found.as_str())],
        ),
    };
    localizer.translate(key, &params)
}

/// The error the caller receives for `cause`, told in `sentence`: a 502 for
/// an Arr down or failing, which a script may retry, a 400 for what the
/// operator typed.
pub fn refusal(cause: &Cause, sentence: String) -> AppError {
    if cause.is_outage() {
        AppError::UpstreamDown(sentence)
    } else {
        AppError::BadRequest(sentence)
    }
}

/// An error the operator can act on: the explanation when the cause is known,
/// the error as it came otherwise.
pub fn explained(error: AppError, kind: &str, base_url: &str, localizer: &Localizer) -> AppError {
    match cause_of(&error) {
        Some(cause) => {
            let sentence = explain(&cause, kind, base_url, localizer);
            refusal(&cause, sentence)
        }
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(status: u16, message: &str) -> AppError {
        AppError::ExternalApi {
            service: "Radarr".into(),
            status,
            message: message.into(),
            retry_after: None,
        }
    }

    #[test]
    fn localhost_in_every_spelling_is_loopback() {
        for address in [
            "http://localhost:7878",
            "http://LOCALHOST:7878/radarr",
            "http://127.0.0.1:7878",
            "http://127.1.2.3",
            "http://[::1]:7878",
            "http://0.0.0.0:7878",
        ] {
            assert!(is_loopback(address), "{address}");
        }
    }

    #[test]
    fn a_container_name_or_a_lan_address_is_not_loopback() {
        for address in [
            "http://radarr:7878",
            "http://192.168.1.10:7878",
            "https://arr.example.org",
            "nonsense",
        ] {
            assert!(!is_loopback(address), "{address}");
        }
    }

    #[test]
    fn a_status_names_its_cause() {
        assert_eq!(cause_of(&upstream(401, "")), Some(Cause::KeyRefused));
        assert_eq!(cause_of(&upstream(403, "")), Some(Cause::RefusedInFront));
        assert_eq!(cause_of(&upstream(404, "")), Some(Cause::NotTheApi));
        assert_eq!(cause_of(&upstream(405, "")), Some(Cause::NotTheApi));
        assert_eq!(cause_of(&upstream(302, "")), Some(Cause::Redirected));
        assert_eq!(cause_of(&upstream(500, "")), Some(Cause::ServerError(500)));
        assert_eq!(cause_of(&upstream(501, "")), Some(Cause::ServerError(501)));
        for status in [502, 503, 504] {
            assert_eq!(cause_of(&upstream(status, "")), Some(Cause::UnreachableBehind(status)));
        }
        assert_eq!(cause_of(&upstream(429, "")), None);
    }

    #[test]
    fn what_is_not_about_the_address_passes_through() {
        assert_eq!(cause_of(&upstream(0, "connection reset by peer")), None);
        assert_eq!(cause_of(&AppError::Conflict("a sync is already running".into())), None);
    }

    #[test]
    fn an_app_that_names_itself_otherwise_is_the_wrong_one() {
        assert_eq!(wrong_app("radarr", Some("Sonarr")), Some(Cause::WrongApp("Sonarr".into())));
        assert_eq!(wrong_app("radarr", Some("Radarr")), None);
        assert_eq!(wrong_app("sonarr", Some(" sonarr ")), None);
        assert_eq!(wrong_app("radarr", Some("")), None);
        assert_eq!(wrong_app("radarr", None), None);
    }

    #[test]
    fn the_container_trap_is_named_only_for_localhost() {
        let localizer = Localizer::new("en");
        let lan = explain(&Cause::Unreachable, "radarr", "http://192.168.1.10:7878", &localizer);
        let local = explain(&Cause::Unreachable, "radarr", "http://localhost:7878", &localizer);

        assert_eq!(
            lan,
            localizer.translate(
                "ArrUnreachable",
                &[("service", "Radarr"), ("address", "http://192.168.1.10:7878")]
            )
        );
        assert_eq!(
            local,
            localizer.translate(
                "ArrUnreachableLoopback",
                &[("service", "Radarr"), ("address", "http://localhost:7878")]
            )
        );
        assert_ne!(lan, local);
    }
}
