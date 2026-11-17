//! The text log, as the process writes it and as a test reads it.

use std::borrow::Cow;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Full, Writer};
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::{FormatFields, FormattedFields};
use tracing_subscriber::registry::LookupSpan;

/// The text layer, written to `writer`: the process's standard output, or a test's capture.
pub fn text_layer<S, W>(
    writer: W,
    colours: bool,
) -> tracing_subscriber::fmt::Layer<S, OneLine, Format<Full, LocalTime>, W>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + 'static,
{
    tracing_subscriber::fmt::layer()
        .fmt_fields(OneLine)
        .with_ansi(colours)
        .with_timer(LocalTime)
        .with_writer(writer)
}

/// Local time in the operator's `TZ`, as Radarr and Sonarr log it, with its offset.
pub struct LocalTime;

impl FormatTime for LocalTime {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        w.write_str(&rfc3339(&jiff::Zoned::now()))
    }
}

/// `at` as `SECURITY.md` shows a log line's time: its fraction in groups of three digits and
/// none when whole, its offset rounded to the minute and never written `Z`.
fn rfc3339(at: &jiff::Zoned) -> String {
    let fraction = match at.subsec_nanosecond() {
        0 => String::new(),
        nanos if nanos % 1_000_000 == 0 => format!(".{:03}", nanos / 1_000_000),
        nanos if nanos % 1_000 == 0 => format!(".{:06}", nanos / 1_000),
        nanos => format!(".{nanos:09}"),
    };
    let offset = at.offset().seconds();
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = (offset.abs() + 30) / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("{}{fraction}{sign}{hours:02}:{minutes:02}", at.strftime("%Y-%m-%dT%H:%M:%S"))
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

/// How large `ROUTARR_LOG_FILE` grows before it is set aside.
const LOG_FILE_LIMIT: u64 = 10 * 1024 * 1024;

/// How many set-aside files are kept, `routarr.log.1` the newest: with the file in use, the log
/// holds at most six times [`LOG_FILE_LIMIT`].
const LOG_FILES_KEPT: usize = 5;

/// The file `ROUTARR_LOG_FILE` names, for a service manager that keeps no output. Appended to
/// across starts, and moved to `.1` before a line would take it past its limit, the older ones
/// each moving one number up.
pub struct RotatingFile {
    path: PathBuf,
    /// Closed while the files move, since Windows renames no file held open.
    file: Option<File>,
    written: u64,
    limit: u64,
    kept: usize,
}

impl RotatingFile {
    pub fn open(path: &Path) -> io::Result<std::sync::Mutex<Self>> {
        Self::with_limit(path, LOG_FILE_LIMIT, LOG_FILES_KEPT).map(std::sync::Mutex::new)
    }

    fn with_limit(path: &Path, limit: u64, kept: usize) -> io::Result<Self> {
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file = appended(path)?;
        let written = file.metadata()?.len();
        Ok(Self { path: path.to_path_buf(), file: Some(file), written, limit, kept })
    }

    fn numbered(&self, number: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{number}"));
        PathBuf::from(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        for number in (1..self.kept).rev() {
            std::fs::rename(self.numbered(number), self.numbered(number + 1)).ok();
        }
        std::fs::rename(&self.path, self.numbered(1))?;
        self.file = Some(appended(&self.path)?);
        self.written = 0;
        Ok(())
    }
}

fn appended(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new().create(true).append(true).open(path)
}

impl io::Write for RotatingFile {
    fn write(&mut self, line: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + line.len() as u64 > self.limit {
            self.rotate()?;
        }
        let file = match &mut self.file {
            Some(file) => file,
            // A rotation that failed half way: the file it names is opened again.
            None => self.file.insert(appended(&self.path)?),
        };
        let length = file.write(line)?;
        self.written += length as u64;
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_log_time_is_local_with_its_offset_and_the_shortest_group_of_fraction_digits() {
        let at = |stamp: &str, offset: i32| {
            let zone = jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(offset).unwrap());
            rfc3339(&stamp.parse::<jiff::Timestamp>().unwrap().to_zoned(zone))
        };
        let utc = |stamp: &str| at(stamp, 0);
        assert_eq!(utc("2026-11-16T18:45:35.900599649Z"), "2026-11-16T18:45:35.900599649+00:00");
        assert_eq!(utc("2026-11-16T18:45:35.900599Z"), "2026-11-16T18:45:35.900599+00:00");
        assert_eq!(utc("2026-11-16T18:45:35.9Z"), "2026-11-16T18:45:35.900+00:00");
        assert_eq!(utc("2026-11-16T18:45:35Z"), "2026-11-16T18:45:35+00:00");
        assert_eq!(at("2026-11-16T18:45:35Z", 2 * 3600), "2026-11-16T20:45:35+02:00");
        assert_eq!(at("2026-11-16T18:45:35Z", -(3 * 3600 + 1800)), "2026-11-16T15:15:35-03:30");
        // An offset with seconds, as the oldest of a zone's are, rounds to the minute.
        assert_eq!(at("1900-01-01T00:00:00Z", 561), "1900-01-01T00:09:21+00:09");

        let mut written = String::new();
        LocalTime.format_time(&mut Writer::new(&mut written)).unwrap();
        assert!(written.parse::<jiff::Timestamp>().is_ok() && !written.ends_with('Z'), "{written}");
    }

    #[test]
    fn a_log_file_moves_aside_before_it_passes_its_limit_and_keeps_its_last_files() {
        let dir = std::env::temp_dir().join(format!("routarr-log-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("logs").join("routarr.log");
        let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();

        let mut log = RotatingFile::with_limit(&path, 20, 2).unwrap();
        for line in ["first line\n", "second line\n", "third line\n", "fourth line\n"] {
            log.write_all(line.as_bytes()).unwrap();
        }
        assert_eq!(read(&path), "fourth line\n");
        assert_eq!(read(&log.numbered(1)), "third line\n");
        assert_eq!(read(&log.numbered(2)), "second line\n");
        assert!(!log.numbered(3).exists(), "more files kept than asked");

        drop(log);
        let mut again = RotatingFile::with_limit(&path, 20, 2).unwrap();
        again.write_all(b"fifth\n").unwrap();
        assert_eq!(read(&path), "fourth line\nfifth\n", "a restart started the file over");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_written_reaches_the_file_at_once() {
        let dir = std::env::temp_dir().join(format!("routarr-log-open-{}", std::process::id()));
        let path = dir.join("routarr.log");
        let log = RotatingFile::open(&path).unwrap();

        let mut file = log.lock().unwrap();
        file.write_all(b"Starting Routarr\n").unwrap();
        file.flush().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Starting Routarr\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
