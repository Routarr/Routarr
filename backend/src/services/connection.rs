//! What stops Routarr reaching an Arr, told as what to do about it: each cause names the
//! field to change, in the operator's language, since it is read under the form they typed
//! it in.

use std::net::IpAddr;

use crate::error::{AppError, Transport, Upstream};
use crate::localization::{Localizer, Told};
use crate::models::Instance;

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
    /// The answer is larger than Routarr reads of it.
    TooLarge,
    KeyRefused,
    /// Something in front of the Arr asks for its own sign-in.
    SignInInFront,
    /// A cloud host's metadata service answers at the address, where Routarr
    /// does not connect.
    MetadataAddress,
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
    let AppError::ExternalApi { status, kind, .. } = error else {
        return None;
    };
    match kind {
        Upstream::Transport(Transport::NameUnresolved) => Some(Cause::NameUnresolved),
        Upstream::Transport(Transport::MetadataAddress) => Some(Cause::MetadataAddress),
        Upstream::Transport(Transport::Unreachable) => Some(Cause::Unreachable),
        Upstream::Transport(Transport::TimedOut) => Some(Cause::TimedOut),
        Upstream::Transport(Transport::HandshakeFailed) => Some(Cause::HandshakeFailed),
        Upstream::Transport(Transport::NotHttp) => Some(Cause::NotHttp),
        Upstream::Transport(Transport::RedirectLoop) => Some(Cause::RedirectLoop),
        Upstream::Transport(Transport::Failed) => None,
        Upstream::Unreadable => Some(Cause::NotTheApi),
        Upstream::TooLarge => Some(Cause::TooLarge),
        Upstream::RedirectedTo(host) => Some(Cause::RedirectedElsewhere(host.clone())),
        Upstream::SignInInFront => Some(Cause::SignInInFront),
        Upstream::Http => match *status {
            300..=399 => Some(Cause::Redirected),
            401 => Some(Cause::KeyRefused),
            // Radarr and Sonarr answer a wrong key with 401. A 403 comes from a
            // firewall, an allow list or a sign-in policy in front of them.
            403 => Some(Cause::RefusedInFront),
            404 | 405 => Some(Cause::NotTheApi),
            502..=504 => Some(Cause::UnreachableBehind(*status)),
            500..=599 => Some(Cause::ServerError(*status)),
            _ => None,
        },
        Upstream::WriteRedirected
        | Upstream::RefusedWith200
        | Upstream::QuotaSpent
        | Upstream::NotListed
        | Upstream::SourceUnseen => None,
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
    told(cause, kind, base_url).render(localizer)
}

/// [`explain`]'s sentence as a key and its values, for a row that stores it
/// and says it again in each reader's language.
pub fn told(cause: &Cause, kind: &str, base_url: &str) -> Told {
    let address = crate::http::masked(base_url);
    let service = service_name(kind);
    let at = |key: &str| Told::new(key).with("service", service).with("address", &address);
    match cause {
        Cause::NameUnresolved => at("ArrNameUnresolved"),
        Cause::Unreachable if is_loopback(base_url) => at("ArrUnreachableLoopback"),
        Cause::Unreachable => at("ArrUnreachable"),
        Cause::TimedOut => at("ArrTimedOut"),
        Cause::HandshakeFailed => at("ArrHandshakeFailed"),
        Cause::NotHttp => at("ArrNotHttp"),
        Cause::Redirected => Told::new("ArrRedirected").with("address", &address),
        Cause::RedirectedElsewhere(host) => at("ArrRedirectedElsewhere").with("host", host),
        Cause::RedirectLoop => at("ArrRedirectLoop"),
        // Only a library listing grows without bound in an Arr. A status or a
        // folder list past the cap is something else streaming at the address.
        Cause::NotTheApi | Cause::TooLarge => {
            at("ArrNotTheApi").with("base", format!("/{}", service.to_ascii_lowercase()))
        }
        Cause::KeyRefused => Told::new("ArrKeyRefused").with("service", service),
        Cause::MetadataAddress => at("ArrMetadataAddress"),
        Cause::SignInInFront => Told::new("ArrProxySignIn").with("service", service),
        Cause::RefusedInFront => Told::new("ArrProxyRefused").with("service", service),
        Cause::UnreachableBehind(code) => {
            Told::new("ArrProxyUnreachable").with("service", service).with("status", code)
        }
        Cause::ServerError(code) => {
            Told::new("ArrServerError").with("service", service).with("status", code)
        }
        Cause::WrongApp(found) => at("ArrWrongApp").with("found", found),
    }
}

/// A failure of a call to an Arr twice over: as a row keeps it, said again in
/// each reader's language (`localization::said`), and as this reader reads it.
pub struct Said {
    pub stored: String,
    pub said: String,
}

impl Said {
    /// `error`, told as what to change when `instance`'s address or key
    /// explains it, as the instance's own test tells it.
    pub fn of(error: &AppError, instance: Option<&Instance>, localizer: &Localizer) -> Self {
        let told = instance
            .and_then(|instance| told_of(error, &instance.instance_type, &instance.base_url))
            .or_else(|| error.told());
        // Named as every other failed move names its Arr, by the instance.
        let told = match (told, instance, error) {
            (
                Some(told),
                Some(instance),
                AppError::ExternalApi { kind: Upstream::SourceUnseen, .. },
            ) => Some(told.with("arr", &instance.name)),
            (told, ..) => told,
        };
        match told {
            Some(told) => Self::told(&told, localizer),
            None => {
                let said = error.public_message();
                Self { stored: said.clone(), said }
            }
        }
    }

    pub fn told(told: &Told, localizer: &Localizer) -> Self {
        Self { stored: told.stored(), said: told.render(localizer) }
    }
}

/// `error`, told as what to change, when the address or the key explains it.
pub fn told_of(error: &AppError, kind: &str, base_url: &str) -> Option<Told> {
    cause_of(error).map(|cause| told(&cause, kind, base_url))
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
        let kind =
            if status == 0 { Upstream::Transport(Transport::Failed) } else { Upstream::Http };
        crate::integrations::upstream("Radarr", status, kind, message.into())
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
        assert_eq!(
            cause_of(&AppError::InProgress {
                reason: "sync_running",
                message: "a sync is already running".into()
            }),
            None
        );
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
