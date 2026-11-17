//! A stored timestamp as the API writes it: RFC 3339, in UTC.
//!
//! Stored, a timestamp has no zone (`routing::format_timestamp`), which is
//! what lets two of them compare as text in SQL. Sent that way, a client
//! reads it in its own time zone, or not at all, so every field that carries
//! one goes through here, with `#[schema(format = DateTime)]` beside it.

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
pub fn written(stored: &str) -> String {
    parse_timestamp(stored).map_or_else(|| stored.to_string(), utc)
}

/// `at` as the API writes an instant: RFC 3339 to the second, in UTC.
pub fn utc(at: jiff::Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
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

    #[test]
    fn rfc_3339_reads_as_the_api_documents_it_and_a_near_miss_is_left_as_written() {
        for (sent, read) in [
            ("2026-10-05t21:26:14z", "2026-10-05T21:26:14Z"),
            ("2026-10-05 21:26:14.123456789123Z", "2026-10-05T21:26:14Z"),
            ("2026-10-05T16:26:14\u{2212}05:00", "2026-10-05T21:26:14Z"),
            ("2026-10-05T23:59:60Z", "2026-10-05T23:59:59Z"),
            ("2026-10-05", "2026-10-05T00:00:00Z"),
        ] {
            assert_eq!(written(sent), read, "{sent}");
        }
        for refused in [
            "2026-10-05T21:26Z",
            "2026-10-05X21:26:14Z",
            "2026-02-30T21:26:14Z",
            "2026-10-05T21:26:14.Z",
            "2026-10-05T21:26:14+0500",
            "2026-10-05T21:26:14+05-00",
            "2026-10-05T21:26:14+24:00",
            "2026-10-05T21:26:14+05:60",
            "2026-10-05T21:26:14~05:00",
        ] {
            assert_eq!(written(refused), refused, "{refused}");
        }
    }
}
