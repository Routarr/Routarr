//! How Routarr was installed, which the image and the packages say through `ROUTARR_INSTALL`.
//! The commands a message names, to read the log or to run a command as the right user,
//! follow it. Unset, a message names no command it cannot know.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    Docker,
    Deb,
    Rpm,
}

impl Install {
    /// The installation `ROUTARR_INSTALL` names, in any case, or `None` for a value none sets.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "docker" => Some(Self::Docker),
            "deb" => Some(Self::Deb),
            "rpm" => Some(Self::Rpm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
        }
    }

    /// The command printing the server log. `routarr` is the container name of
    /// `docker-compose.yml` and the unit name of the packages.
    pub fn log_command(self) -> &'static str {
        match self {
            Self::Docker => "docker logs routarr",
            Self::Deb | Self::Rpm => "journalctl -u routarr",
        }
    }
}

static CURRENT: OnceLock<Option<Install>> = OnceLock::new();

/// Settle the installation for the whole process, once its configuration is read. A text
/// translated anywhere names the log through it, with no configuration at hand.
pub fn settle(install: Option<Install>) {
    CURRENT.set(install).ok();
}

/// The installation settled at start. `None` in the tests, which settle nothing.
pub fn current() -> Option<Install> {
    CURRENT.get().copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_installation_reads_back_from_its_own_name() {
        for install in [Install::Docker, Install::Deb, Install::Rpm] {
            assert_eq!(Install::parse(install.as_str()), Some(install));
        }
        assert_eq!(Install::parse(" DEB "), Some(Install::Deb));
        for unknown in ["", "tarball", "dockerfile", "apt"] {
            assert_eq!(Install::parse(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn the_log_is_read_where_each_installation_sends_it() {
        assert_eq!(Install::Docker.log_command(), "docker logs routarr");
        assert_eq!(Install::Deb.log_command(), "journalctl -u routarr");
        assert_eq!(Install::Rpm.log_command(), "journalctl -u routarr");
    }
}
