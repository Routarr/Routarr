//! A stored timestamp as the API writes it: RFC 3339, in UTC.
//!
//! Stored, a timestamp has no zone (`routing::format_timestamp`), which is
//! what lets two of them compare as text in SQL. Sent that way, a client
//! reads it in its own time zone, or not at all, so every field that carries
//! one goes through here, with `#[schema(format = DateTime)]` beside it.

use chrono::SecondsFormat;
use serde::Serializer;

use crate::services::routing::parse_timestamp;

pub fn rfc3339<S: Serializer>(stored: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&written(stored))
}

pub fn rfc3339_or_null<S: Serializer>(
    stored: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match stored {
        Some(stored) => serializer.serialize_some(&written(stored)),
        None => serializer.serialize_none(),
    }
}

/// A value no shape reads is written as it is: a field that looks odd beats
/// one that silently says another moment.
fn written(stored: &str) -> String {
    parse_timestamp(stored)
        .map_or_else(|| stored.to_string(), |at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_timestamp_is_written_with_its_zone() {
        assert_eq!(written("2026-10-05 21:26:14"), "2026-10-05T21:26:14Z");
        assert_eq!(written("2026-10-05T23:26:14+02:00"), "2026-10-05T21:26:14Z");
        assert_eq!(written("not a date"), "not a date");
    }
}
