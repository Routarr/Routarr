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
    /// Nothing listens at the address, or its host does not resolve.
    Unreachable,
    /// The connection was taken and nothing came back in time.
    TimedOut,
    /// The address sends the request to another origin, where the key never goes.
    Redirected,
    /// Something answers, but not the Arr's API: a web page, a proxy, another port.
    NotTheApi,
    KeyRefused,
    /// The Arr failed on its own side, with this HTTP status.
    ServerError(u16),
    /// Another application answers, named as it names itself.
    WrongApp(String),
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
            Transport::Unreachable => Some(Cause::Unreachable),
            Transport::TimedOut => Some(Cause::TimedOut),
            Transport::Redirected => Some(Cause::Redirected),
            Transport::Unreadable => Some(Cause::NotTheApi),
        },
        300..=399 => Some(Cause::Redirected),
        401 | 403 => Some(Cause::KeyRefused),
        404 | 405 => Some(Cause::NotTheApi),
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

/// The sentence the operator reads, naming what to change.
///
/// `kind` is the declared type, `radarr` or `sonarr`.
pub fn explain(cause: &Cause, kind: &str, base_url: &str, localizer: &Localizer) -> String {
    let service = if kind.eq_ignore_ascii_case("sonarr") { "Sonarr" } else { "Radarr" };
    let url_base = format!("/{}", service.to_ascii_lowercase());
    let status;
    let (key, params): (&str, Vec<(&str, &str)>) = match cause {
        Cause::Unreachable if is_loopback(base_url) => {
            ("ArrUnreachableLoopback", vec![("service", service), ("address", base_url)])
        }
        Cause::Unreachable => ("ArrUnreachable", vec![("service", service), ("address", base_url)]),
        Cause::TimedOut => ("ArrTimedOut", vec![("service", service), ("address", base_url)]),
        Cause::Redirected => ("ArrRedirected", vec![("address", base_url)]),
        Cause::NotTheApi => (
            "ArrNotTheApi",
            vec![("service", service), ("address", base_url), ("base", url_base.as_str())],
        ),
        Cause::KeyRefused => ("ArrKeyRefused", vec![("service", service)]),
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

/// An error the operator can act on: the explanation when the cause is known,
/// the error as it came otherwise.
pub fn explained(error: AppError, kind: &str, base_url: &str, localizer: &Localizer) -> AppError {
    match cause_of(&error) {
        Some(cause) => AppError::BadRequest(explain(&cause, kind, base_url, localizer)),
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
        assert_eq!(cause_of(&upstream(403, "")), Some(Cause::KeyRefused));
        assert_eq!(cause_of(&upstream(404, "")), Some(Cause::NotTheApi));
        assert_eq!(cause_of(&upstream(302, "")), Some(Cause::Redirected));
        assert_eq!(cause_of(&upstream(503, "")), Some(Cause::ServerError(503)));
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
