//! The status each source gives a title, read into the words Radarr and Sonarr
//! write theirs in: `tba`, `announced`, `inCinemas` and `released` for a film,
//! `upcoming`, `continuing` and `ended` for a series. Read otherwise, one
//! title would be "Released", "FINISHED" or "Finished Airing" depending on
//! which source answered first.

/// The status `raw` states for a title of `media_type`, or `None` where the
/// Arrs have no word for it, as a film cancelled before its release.
pub fn read(raw: &str, media_type: &str) -> Option<&'static str> {
    let film = media_type == "movie";
    let folded = raw.trim().to_ascii_lowercase().replace(['_', '-'], " ");
    Some(match folded.as_str() {
        // TMDb and TheTVDB, AniList, MyAnimeList through Jikan.
        "released" | "ended" | "finished" | "finished airing" => {
            if film {
                "released"
            } else {
                "ended"
            }
        }
        "releasing" | "currently airing" => {
            if film {
                "inCinemas"
            } else {
                "continuing"
            }
        }
        "returning series" | "continuing" | "hiatus" if !film => "continuing",
        "planned" | "in production" | "post production" | "pilot" | "not yet released"
        | "not yet aired" | "upcoming" => {
            if film {
                "announced"
            } else {
                "upcoming"
            }
        }
        "rumored" => {
            if film {
                "tba"
            } else {
                "upcoming"
            }
        }
        // A series stopped for good has ended. A film stopped is no status
        // Radarr writes.
        "canceled" | "cancelled" if !film => "ended",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::read;

    /// Every source's words for one state read as one, for a film or a series.
    #[test]
    fn each_source_s_status_reads_as_the_arrs_word() {
        for raw in ["Released", "FINISHED", "Finished Airing"] {
            assert_eq!(read(raw, "movie"), Some("released"), "{raw}");
        }
        for raw in ["Ended", "FINISHED", "Finished Airing", "Canceled", "CANCELLED"] {
            assert_eq!(read(raw, "series"), Some("ended"), "{raw}");
        }
        for raw in ["Returning Series", "RELEASING", "Currently Airing", "Continuing", "HIATUS"] {
            assert_eq!(read(raw, "series"), Some("continuing"), "{raw}");
        }
        for raw in ["Planned", "In Production", "Post Production", "NOT_YET_RELEASED"] {
            assert_eq!(read(raw, "movie"), Some("announced"), "{raw}");
        }
        for raw in ["Not yet aired", "Upcoming", "Pilot", "Rumored"] {
            assert_eq!(read(raw, "series"), Some("upcoming"), "{raw}");
        }
        assert_eq!(read("Rumored", "movie"), Some("tba"));
        assert_eq!(read("RELEASING", "movie"), Some("inCinemas"));
    }

    /// A state the Arrs have no word for is no status rather than a wrong one.
    #[test]
    fn a_state_the_arrs_cannot_say_is_no_status() {
        assert_eq!(read("Canceled", "movie"), None);
        assert_eq!(read("Returning Series", "movie"), None);
        assert_eq!(read("Sortie prévue", "movie"), None);
        assert_eq!(read("", "series"), None);
    }
}
