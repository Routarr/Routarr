//! The text log, as the process writes it and as a test reads it.

use std::borrow::Cow;
use std::fmt;

use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Full, Writer};
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::fmt::{FormatFields, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

/// The text layer, written to `writer`: the process's standard output, or a test's capture.
pub fn text_layer<S, W>(
    writer: W,
    colours: bool,
) -> tracing_subscriber::fmt::Layer<S, OneLine, Format<Full, ChronoLocal>, W>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + 'static,
{
    // Local time in the operator's `TZ`, as Radarr and Sonarr log it, with its offset.
    tracing_subscriber::fmt::layer()
        .fmt_fields(OneLine)
        .with_ansi(colours)
        .with_timer(ChronoLocal::rfc_3339())
        .with_writer(writer)
}

/// The fields of an event or a span as `DefaultFields` writes them, a line break written `\n`
/// and a carriage return `\r`: a value a caller sent would otherwise start a line of its own,
/// which a fail2ban filter reads as Routarr's.
pub struct OneLine;

impl<'writer> FormatFields<'writer> for OneLine {
    fn format_fields<R: RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        writer.write_str(&escaped(&formatted(fields)?))
    }

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        let added = formatted(fields)?;
        if !current.fields.is_empty() && !added.is_empty() {
            current.fields.push(' ');
        }
        current.fields.push_str(&escaped(&added));
        Ok(())
    }
}

fn formatted(fields: impl RecordFields) -> Result<String, fmt::Error> {
    let mut text = String::new();
    DefaultFields::new().format_fields(Writer::new(&mut text), fields)?;
    Ok(text)
}

fn escaped(text: &str) -> Cow<'_, str> {
    if !text.contains(['\n', '\r']) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace('\n', "\\n").replace('\r', "\\r"))
}
