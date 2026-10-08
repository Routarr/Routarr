use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Public TMDB API root.
pub const DEFAULT_TMDB_BASE_URL: &str = "https://api.themoviedb.org/3";

/// `ROUTARR_HTTP_TIMEOUT_SECS` when unset.
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 20;

/// `ROUTARR_MAX_LIBRARY_MIB` when unset.
pub(crate) const DEFAULT_MAX_LIBRARY_MIB: usize = 256;

/// How long an apply follows the Arr's moves of files. A move within one
/// filesystem is a rename the Arr ends in a second. One copying across disks
/// takes minutes a film, longer than anyone watches an apply, and is left to
/// the next sync.
const MOVE_WAIT: Duration = Duration::from_secs(60);

/// How long a webhook delivery is answered within, under the timeout an Arr
/// gives a notification.
const WEBHOOK_ANSWER_WAIT: Duration = Duration::from_secs(20);

/// The directory a database file lives in.
///
/// One place for every caller: `Path::new(":memory:").parent()` is `Some("")`,
/// an *empty* path, which `unwrap_or` never catches and which `join` turns into
/// a path relative to the working directory.
fn data_dir_for(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Where the image keeps the database: `/config`, as Radarr and Sonarr keep
/// theirs.
const IMAGE_DB_PATH: &str = "/config/routarr.db";

/// Where a container that mounts `/data` and not `/config` holds its database.
const DATA_DB_PATH: &str = "/data/routarr.db";

/// The database to open: `configured`, unless it is the image's own path,
/// holds nothing yet, and `/data` holds a database. A volume mounted at
/// `/data` keeps its library then, and the note says how to follow the
/// image's layout, rather than an empty library opened in its place.
fn settled_db_path(
    configured: PathBuf,
    exists: impl Fn(&Path) -> bool,
) -> (PathBuf, Option<String>) {
    let (image, data) = (Path::new(IMAGE_DB_PATH), Path::new(DATA_DB_PATH));
    if configured == image && !exists(image) && exists(data) {
        let note = format!(
            "The database is read from {DATA_DB_PATH}. To follow the image's layout, mount the \
             same folder at /config instead of /data."
        );
        return (data.to_path_buf(), Some(note));
    }
    (configured, None)
}

/// How a caller proves who it is.
///
/// The names follow the Servarr applications, which offer `None`, `Forms` and
/// `External` beside the API key every machine client uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No credential asked for. Only reachable by asking for it.
    None,
    /// The API key, in `X-Api-Key` or `Authorization: Bearer`.
    ApiKey,
    /// A username and a password, exchanged for a session cookie.
    ///
    /// The API key keeps working beside it, as it does in Servarr: a machine
    /// client cannot hold a cookie, and a header is immune to the cross-site
    /// request forgery a cookie invites.
    Forms,
    /// An OpenID Connect provider authenticates, and Routarr lets in the
    /// subjects and the groups the operator names.
    ///
    /// One level of access, and no user table to keep: what it records is the
    /// subject, as the actor on every decision and write.
    Oidc,
    /// A reverse proxy authenticates, and Routarr asks for nothing.
    ///
    /// It reads no identity header, exactly as Radarr's own `External` does:
    /// there is no header to forge, and nothing to get wrong about a trust
    /// boundary. What it demands instead is that nothing reach the port except
    /// through the proxy, which is the operator's to guarantee and Routarr's
    /// to state loudly at startup.
    External,
}

impl AuthMode {
    /// The name `ROUTARR_AUTH` takes, which is also what a session records.
    ///
    /// Stated once because a session is only honoured by the mode whose name it
    /// carries: two spellings of that name would refuse every session the day
    /// they drifted apart.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ApiKey => "apikey",
            Self::Forms => "forms",
            Self::Oidc => "oidc",
            Self::External => "external",
        }
    }

    /// The mode a value of `ROUTARR_AUTH` names, and whether it named none.
    ///
    /// Anything unrecognised falls back to the API key rather than to nothing:
    /// a typo must not be the way an installation ends up open.
    pub fn parse(raw: &str) -> (Self, bool) {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => (Self::None, false),
            "oidc" | "openid" => (Self::Oidc, false),
            "forms" | "form" => (Self::Forms, false),
            "external" | "proxy" => (Self::External, false),
            "apikey" | "api_key" | "" => (Self::ApiKey, false),
            _ => (Self::ApiKey, true),
        }
    }
}

/// The level `raw` names, and whether it named none, `info` standing in.
/// `warning` is read as `warn`, the word the log filter knows: unread, it
/// would leave the filter at its default without a word.
fn log_level_from(raw: &str) -> (String, bool) {
    match raw.trim().to_ascii_lowercase().as_str() {
        "warning" => ("warn".into(), false),
        level @ ("trace" | "debug" | "info" | "warn" | "error" | "off") => (level.into(), false),
        _ => ("info".into(), true),
    }
}

/// The format `raw` names, and whether it named none, `text` standing in.
fn log_format_from(raw: &str) -> (String, bool) {
    match raw.trim().to_ascii_lowercase().as_str() {
        format @ ("text" | "json") => (format.into(), false),
        _ => ("text".into(), true),
    }
}

/// Whether a URL is reached over TLS, or on this machine, where no wire is
/// involved and a provider under test or beside the container is plain http.
pub fn reaches_over_tls(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

/// An entry of `ROUTARR_TRUSTED_PROXIES`: one address, or a range written
/// `address/prefix`, which a proxy recreated on a Docker network stays inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    base: std::net::IpAddr,
    prefix: u8,
}

impl Network {
    pub fn parse(entry: &str) -> Option<Self> {
        let (address, prefix) = match entry.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix.parse::<u8>().ok()?)),
            None => (entry, None),
        };
        let base = address.parse::<std::net::IpAddr>().ok()?.to_canonical();
        let width = if base.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(width);
        (prefix <= width).then_some(Self { base, prefix })
    }

    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        use std::net::IpAddr::{V4, V6};
        let mask = |width: u32| match u32::from(self.prefix) {
            0 => 0,
            prefix => u128::MAX << (width - prefix),
        };
        match (self.base, ip.to_canonical()) {
            (V4(base), V4(ip)) => {
                let mask = mask(32) as u32;
                u32::from(base) & mask == u32::from(ip) & mask
            }
            (V6(base), V6(ip)) => {
                let mask = mask(128);
                u128::from(base) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// One entry of `ROUTARR_CORS_ORIGINS`, as a browser would send it.
///
/// A browser's `Origin` header is `scheme://host[:port]` and nothing else, and
/// tower-http compares the header against these values verbatim. So anything
/// that is not exactly that shape can never match. Accepted in silence, it
/// would be counted in the startup line and leave the operator with CORS that
/// looks configured and does nothing.
///
/// `*` is refused outright rather than translated: this layer sends
/// `Access-Control-Allow-Credentials`, which the Fetch standard forbids
/// combining with a wildcard, and `AllowOrigin::list` answers a wildcard with a
/// panic, so an installation that set it would never start at all.
/// A metadata source's root, refused at start when it is no address a request
/// can go to: a typo would only ever show as the source being unreachable.
fn validate_source_root(variable: &str, root: &str) -> AppResult<()> {
    let bad = |why: &str| AppError::Config(format!("{variable}: '{root}' {why}"));
    let url = reqwest::Url::parse(root).map_err(|_| bad("is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("must start with http:// or https://"));
    }
    if url.host_str().is_none() {
        return Err(bad("names no host"));
    }
    Ok(())
}

fn validate_origin(origin: &str) -> AppResult<()> {
    // Named through a constant rather than inline: the sample-env check scans
    // this file for a quoted `ROUTARR_*` and reads the whole literal as a
    // variable, so a message beginning with the name would be reported as an
    // undocumented variable of its own. Same reason as `Config::from_env`.
    const VARIABLE: &str = "ROUTARR_CORS_ORIGINS";
    let bad = |why: &str| AppError::Config(format!("{VARIABLE}: '{origin}' {why}"));

    if origin == "*" {
        return Err(bad(
            "cannot be a wildcard: Routarr sends credentials with a cross-origin request, \
             which a wildcard origin is not allowed to accompany. List the origins instead",
        ));
    }

    let url = reqwest::Url::parse(origin).map_err(|_| bad("is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("must start with http:// or https://"));
    }
    if url.host_str().is_none() {
        return Err(bad("names no host"));
    }
    // `Url::parse` gives a bare origin the path "/", which the trailing-slash
    // trim in `from_env` has already removed from the stored string. Anything
    // longer is a path, and a browser never sends one.
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(bad("must be an origin (scheme, host and port, with no path)"));
    }
    Ok(())
}

/// The shortest key `ROUTARR_API_KEY` may pin. A generated key is 64
/// characters, so the floor costs no operator anything.
const MIN_PINNED_KEY_LENGTH: usize = 32;

/// Application configuration loaded from environment variables.
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub db_path: PathBuf,
    /// Directory holding the database, and with it the master key, the API key
    /// and the backups. Derived from `db_path` once, by [`data_dir_for`].
    pub data_dir: PathBuf,
    pub log_level: String,
    /// `text` (default) or `json`, which is easier to ship to a log collector.
    pub log_format: String,
    /// What was read and set aside for a default, said as warnings once the
    /// log is up: read before it, nothing would hear them.
    pub startup_notes: Vec<String>,
    pub frontend_dir: PathBuf,
    pub tmdb_api_key: Option<String>,
    /// `ROUTARR_API_KEY`, which pins the key and wins over the stored one.
    ///
    /// Only the environment's value: the key in force is `AppState::api_key`,
    /// which also holds one generated at startup or minted from the interface.
    pub api_key: Option<String>,
    /// How a caller proves who it is.
    ///
    /// Radarr and Sonarr made authentication mandatory in v4 and Routarr
    /// follows: it has to be turned off deliberately, never obtained by
    /// forgetting to set a key.
    pub auth_mode: AuthMode,
    /// The OpenID Connect provider, as an issuer URL to discover from.
    ///
    /// Discovery rather than four endpoints: a provider states its own
    /// addresses at a well-known path, and copying them by hand is four more
    /// values to keep in step with somebody else's deployment.
    pub oidc_issuer: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret: Option<String>,
    /// Where the provider sends the browser back. Absolute, because the
    /// provider compares it against what it was registered with.
    pub oidc_redirect_url: Option<String>,
    /// The `sub` of each person let in. A provider left at its defaults lets
    /// every account of its directory use every client, so Routarr decides.
    pub oidc_allowed_subjects: Vec<String>,
    /// The groups whose members are let in, read from `oidc_groups_claim`.
    pub oidc_allowed_groups: Vec<String>,
    pub oidc_groups_claim: String,
    /// Lets in every account the provider authenticates, which the operator
    /// has to say in so many words.
    pub oidc_allow_anyone: bool,
    /// Explicit CORS allow-list. Empty means "same-origin only" (no CORS layer).
    pub cors_origins: Vec<String>,
    /// The host names Routarr answers to under `ROUTARR_AUTH=none`, beside an
    /// address and `localhost`, lower case and without a port.
    pub allowed_hosts: Vec<String>,
    /// The proxies whose `X-Forwarded-For` names the client, for the sign-in
    /// queue's share per client. Any other peer is the client itself.
    pub trusted_proxies: Vec<Network>,
    /// Timeout applied to every outbound call: the Arrs, the metadata sources,
    /// the identity provider and the notification webhook.
    pub http_timeout: Duration,
    /// Timeout for listing a whole library, where `http_timeout` holds every
    /// other call: tens of thousands of titles from a NAS take far longer than
    /// a probe is given to answer.
    pub library_timeout: Duration,
    /// The largest library listing read, in bytes, where every other answer
    /// is held to a cap of its own: a library of tens of thousands of titles
    /// outgrows any one size.
    pub max_library_bytes: usize,
    /// How long an apply follows the Arr's moves of files before it records
    /// the ones still running as requested, for the next sync to settle.
    pub move_wait: Duration,
    /// How long a webhook delivery is answered within: past it the Arr is told
    /// the delivery was accepted, and the work goes on.
    pub webhook_answer_wait: Duration,
    /// Master key sealing the stored secrets (the Arr and metadata source keys).
    /// Generated beside the database if absent.
    pub secret_key: Option<String>,
    /// Superseded master key, kept readable for one rotation.
    pub previous_secret_key: Option<String>,
    /// Lets a start make a new master key although the database holds values
    /// sealed with the one it had, which then have to be entered again.
    pub allow_new_master_key: bool,
    /// Max concurrent outbound requests per metadata source during enrichment.
    ///
    /// A ceiling, not a target: `services::rate_limit` additionally paces each
    /// source to its published rate, and a source may lower this on its own.
    pub metadata_concurrency: usize,
    /// TMDB API root. Overridable for a mirror, a caching proxy, or tests.
    pub tmdb_base_url: String,
    /// OMDb key. Free, but mandatory: OMDb rejects an unauthenticated request.
    pub omdb_api_key: Option<String>,
    pub omdb_base_url: String,
    /// TheTVDB v4 key, and the subscriber PIN a *user-supported* key needs.
    pub tvdb_api_key: Option<String>,
    pub tvdb_pin: Option<String>,
    pub tvdb_base_url: String,
    /// AniList and Jikan authenticate nothing, so they carry a root and no key.
    pub anilist_base_url: String,
    pub jikan_base_url: String,
    /// Sub-path Routarr is mounted under, Servarr's "URL base".
    ///
    /// Normalised to either an empty string or `/something` with no trailing
    /// slash, so callers can always concatenate without guessing.
    pub base_path: String,
}

/// Every field but the secrets, which read as set or not. Destructured whole,
/// so a field added later is written here too, or the build fails.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            host,
            port,
            db_path,
            data_dir,
            log_level,
            log_format,
            startup_notes,
            frontend_dir,
            tmdb_api_key,
            api_key,
            auth_mode,
            oidc_issuer,
            oidc_client_id,
            oidc_client_secret,
            oidc_redirect_url,
            oidc_allowed_subjects,
            oidc_allowed_groups,
            oidc_groups_claim,
            oidc_allow_anyone,
            cors_origins,
            allowed_hosts,
            trusted_proxies,
            http_timeout,
            library_timeout,
            max_library_bytes,
            move_wait,
            webhook_answer_wait,
            secret_key,
            previous_secret_key,
            allow_new_master_key,
            metadata_concurrency,
            tmdb_base_url,
            omdb_api_key,
            omdb_base_url,
            tvdb_api_key,
            tvdb_pin,
            tvdb_base_url,
            anilist_base_url,
            jikan_base_url,
            base_path,
        } = self;
        let hidden = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("Config")
            .field("host", host)
            .field("port", port)
            .field("db_path", db_path)
            .field("data_dir", data_dir)
            .field("log_level", log_level)
            .field("log_format", log_format)
            .field("startup_notes", startup_notes)
            .field("frontend_dir", frontend_dir)
            .field("tmdb_api_key", &hidden(tmdb_api_key))
            .field("api_key", &hidden(api_key))
            .field("auth_mode", auth_mode)
            .field("oidc_issuer", oidc_issuer)
            .field("oidc_client_id", oidc_client_id)
            .field("oidc_client_secret", &hidden(oidc_client_secret))
            .field("oidc_redirect_url", oidc_redirect_url)
            .field("oidc_allowed_subjects", oidc_allowed_subjects)
            .field("oidc_allowed_groups", oidc_allowed_groups)
            .field("oidc_groups_claim", oidc_groups_claim)
            .field("oidc_allow_anyone", oidc_allow_anyone)
            .field("cors_origins", cors_origins)
            .field("allowed_hosts", allowed_hosts)
            .field("trusted_proxies", trusted_proxies)
            .field("http_timeout", http_timeout)
            .field("library_timeout", library_timeout)
            .field("max_library_bytes", max_library_bytes)
            .field("move_wait", move_wait)
            .field("webhook_answer_wait", webhook_answer_wait)
            .field("secret_key", &hidden(secret_key))
            .field("previous_secret_key", &hidden(previous_secret_key))
            .field("allow_new_master_key", allow_new_master_key)
            .field("metadata_concurrency", metadata_concurrency)
            .field("tmdb_base_url", tmdb_base_url)
            .field("omdb_api_key", &hidden(omdb_api_key))
            .field("omdb_base_url", omdb_base_url)
            .field("tvdb_api_key", &hidden(tvdb_api_key))
            .field("tvdb_pin", &hidden(tvdb_pin))
            .field("tvdb_base_url", tvdb_base_url)
            .field("anilist_base_url", anilist_base_url)
            .field("jikan_base_url", jikan_base_url)
            .field("base_path", base_path)
            .finish()
    }
}

/// Turn whatever the user wrote into either `""` or `/segment[/segment…]`.
///
/// People write `routarr`, `/routarr`, `/routarr/` and `routarr/` and expect all
/// four to work. Getting this wrong produces a double slash or a missing one in
/// every generated URL, which is the kind of bug that only shows up behind the
/// proxy, the one place it cannot be debugged comfortably.
/// The most requests in flight per source a pass may hold.
const MAX_METADATA_CONCURRENCY: usize = 16;

/// The concurrency asked for, held to 1 to [`MAX_METADATA_CONCURRENCY`], and
/// whether it had to be, which the start says rather than keeps quiet.
fn concurrency_from(asked: usize) -> (usize, bool) {
    let kept = asked.clamp(1, MAX_METADATA_CONCURRENCY);
    (kept, kept != asked)
}

/// Refuse a mount point the router cannot serve and a link cannot carry: a
/// segment of anything but letters, digits and `-._~` panics in a route, ends
/// a URL at `?` or `#`, or breaks out of the `<base href>` the page is given.
fn validate_base_path(base_path: &str) -> AppResult<()> {
    let segments: Vec<&str> = base_path.trim_start_matches('/').split('/').collect();
    let served = base_path.is_empty()
        || segments.iter().all(|segment| {
            !matches!(*segment, "" | "." | "..")
                && segment.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
        });
    if served {
        return Ok(());
    }
    let variable = "ROUTARR_BASE_PATH";
    Err(AppError::Config(format!(
        "{variable} is '{base_path}', which no address can carry. Use letters, digits, '-', '.', \
         '_' and '~' between slashes, such as /routarr"
    )))
}

pub fn normalise_base_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() { String::new() } else { format!("/{trimmed}") }
}

/// Where the built interface is, when nobody said.
///
/// Two layouts, both real: in the image the binary sits at `/app` beside
/// `/app/frontend/dist`, while `cargo run` from `backend/` has to look one
/// level up. Without the fallback the second gives a 404 on every route and an
/// "API-only mode" line nobody thinks to read.
///
/// Falling back rather than changing the default keeps the image's layout
/// first, at one `exists()` per startup.
fn default_frontend_dir() -> PathBuf {
    let here = PathBuf::from("./frontend/dist");
    if here.exists() {
        return here;
    }
    let beside = PathBuf::from("../frontend/dist");
    if beside.exists() {
        return beside;
    }
    here
}

impl Config {
    /// Load configuration from environment variables with sensible defaults.
    ///
    /// A value that cannot be read is an error naming the variable, never the
    /// default in its place: with the default, `ROUTARR_PORT=987 6` would run
    /// on 9876 while the operator believes their port is in force.
    pub fn from_env() -> AppResult<Self> {
        let configured =
            path_or(std::env::var("ROUTARR_DB_PATH").ok(), || PathBuf::from("./data/routarr.db"));
        let (db_path, settled) = settled_db_path(configured, Path::exists);
        // Each variable is named apart from its value: the sample-env check
        // scans this file for a quoted `ROUTARR_*` and would read a message
        // beginning with the name as a variable of its own.
        let mut startup_notes: Vec<String> = settled.into_iter().collect();
        let raw_auth = env_or("ROUTARR_AUTH", "apikey");
        let (auth_mode, unknown) = AuthMode::parse(&raw_auth);
        if unknown {
            let variable = "ROUTARR_AUTH";
            startup_notes.push(format!(
                "{variable} is '{raw_auth}', which no mode is called: the API key is required"
            ));
        }
        let asked = env_parse("ROUTARR_METADATA_CONCURRENCY", 4usize)?;
        let (metadata_concurrency, clamped) = concurrency_from(asked);
        if clamped {
            let variable = "ROUTARR_METADATA_CONCURRENCY";
            startup_notes.push(format!(
                "{variable} is {asked}, outside 1 to {MAX_METADATA_CONCURRENCY}: \
                 {metadata_concurrency}"
            ));
        }
        let raw_level = env_or("ROUTARR_LOG_LEVEL", "info");
        let (log_level, unknown) = log_level_from(&raw_level);
        if unknown {
            let variable = "ROUTARR_LOG_LEVEL";
            startup_notes
                .push(format!("{variable} is '{raw_level}', which no level is called: info"));
        }
        let raw_format = env_or("ROUTARR_LOG_FORMAT", "text");
        let (log_format, unknown) = log_format_from(&raw_format);
        if unknown {
            let variable = "ROUTARR_LOG_FORMAT";
            startup_notes
                .push(format!("{variable} is '{raw_format}', neither text nor json: text"));
        }
        Ok(Self {
            host: env_or("ROUTARR_HOST", "0.0.0.0"),
            port: env_parse("ROUTARR_PORT", 9876)?,
            data_dir: data_dir_for(&db_path),
            db_path,
            log_level,
            log_format,
            startup_notes,
            frontend_dir: path_or(std::env::var("ROUTARR_FRONTEND_DIR").ok(), default_frontend_dir),
            tmdb_api_key: non_empty("TMDB_API_KEY"),
            api_key: non_empty("ROUTARR_API_KEY"),
            auth_mode,
            oidc_issuer: non_empty("ROUTARR_OIDC_ISSUER")
                .map(|v| v.trim_end_matches('/').to_string()),
            oidc_client_id: non_empty("ROUTARR_OIDC_CLIENT_ID"),
            oidc_client_secret: non_empty("ROUTARR_OIDC_CLIENT_SECRET"),
            oidc_redirect_url: non_empty("ROUTARR_OIDC_REDIRECT_URL"),
            oidc_allowed_subjects: list("ROUTARR_OIDC_ALLOWED_SUBJECTS"),
            oidc_allowed_groups: list("ROUTARR_OIDC_ALLOWED_GROUPS"),
            oidc_groups_claim: env_or("ROUTARR_OIDC_GROUPS_CLAIM", "groups"),
            oidc_allow_anyone: env_parse("ROUTARR_OIDC_ALLOW_ANYONE", false)?,
            cors_origins: non_empty("ROUTARR_CORS_ORIGINS")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().trim_end_matches('/').to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            trusted_proxies: non_empty("ROUTARR_TRUSTED_PROXIES")
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|entry| !entry.is_empty())
                        .map(|entry| {
                            Network::parse(entry).ok_or_else(|| {
                                AppError::Config(format!(
                                    "'{entry}' in ROUTARR_TRUSTED_PROXIES is neither an IP address \
                                     nor a range such as 172.18.0.0/16"
                                ))
                            })
                        })
                        .collect::<AppResult<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default(),
            allowed_hosts: non_empty("ROUTARR_ALLOWED_HOSTS")
                .map(|v| {
                    v.split(',')
                        .map(|host| host_name(host.trim()))
                        .filter(|host| !host.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            http_timeout: Duration::from_secs(env_parse(
                "ROUTARR_HTTP_TIMEOUT_SECS",
                DEFAULT_HTTP_TIMEOUT_SECS,
            )?),
            library_timeout: Duration::from_secs(env_parse(
                "ROUTARR_LIBRARY_TIMEOUT_SECS",
                crate::http::LIBRARY_TIMEOUT.as_secs(),
            )?),
            max_library_bytes: env_parse("ROUTARR_MAX_LIBRARY_MIB", DEFAULT_MAX_LIBRARY_MIB)?
                .saturating_mul(1 << 20),
            move_wait: MOVE_WAIT,
            webhook_answer_wait: WEBHOOK_ANSWER_WAIT,
            secret_key: non_empty("ROUTARR_SECRET_KEY"),
            previous_secret_key: non_empty("ROUTARR_PREVIOUS_SECRET_KEY"),
            allow_new_master_key: env_parse("ROUTARR_ALLOW_NEW_MASTER_KEY", false)?,
            // Bounds how many requests are *open* per source, and `rate_limit`
            // bounds how many are made.
            metadata_concurrency,
            tmdb_base_url: env_or("ROUTARR_TMDB_BASE_URL", DEFAULT_TMDB_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            omdb_api_key: non_empty("OMDB_API_KEY"),
            omdb_base_url: env_or(
                "ROUTARR_OMDB_BASE_URL",
                crate::integrations::omdb::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            tvdb_api_key: non_empty("TVDB_API_KEY"),
            tvdb_pin: non_empty("TVDB_SUBSCRIBER_PIN"),
            tvdb_base_url: env_or(
                "ROUTARR_TVDB_BASE_URL",
                crate::integrations::tvdb::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            anilist_base_url: env_or(
                "ROUTARR_ANILIST_BASE_URL",
                crate::integrations::anilist::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            jikan_base_url: env_or(
                "ROUTARR_JIKAN_BASE_URL",
                crate::integrations::jikan::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            base_path: normalise_base_path(&env_or("ROUTARR_BASE_PATH", "")),
        })
    }

    /// Each metadata source's root, by the variable that sets it, and whether
    /// a key travels with its requests.
    fn source_roots(&self) -> [(&'static str, &str, bool); 5] {
        [
            ("ROUTARR_TMDB_BASE_URL", &self.tmdb_base_url, true),
            ("ROUTARR_OMDB_BASE_URL", &self.omdb_base_url, true),
            ("ROUTARR_TVDB_BASE_URL", &self.tvdb_base_url, true),
            ("ROUTARR_ANILIST_BASE_URL", &self.anilist_base_url, false),
            ("ROUTARR_JIKAN_BASE_URL", &self.jikan_base_url, false),
        ]
    }

    /// What start says of the sources' roots: a key sent over plain http to
    /// another host travels in the clear, TMDB's and OMDb's in the address
    /// itself.
    pub fn source_notes(&self) -> Vec<String> {
        let in_clear = |root: &str| {
            root.to_ascii_lowercase().starts_with("http://")
                && !crate::services::connection::is_loopback(root)
        };
        self.source_roots()
            .into_iter()
            .filter(|(_, root, keyed)| *keyed && in_clear(root))
            .map(|(variable, root, _)| {
                format!("{variable} is '{root}': the source's key travels in the clear over http")
            })
            .collect()
    }

    /// Returns the socket address to bind to.
    pub fn bind_address(&self) -> String {
        format!("{}:{}", bracketed(&self.host), self.port)
    }

    /// Where `routarr healthcheck` asks whether the server is up: the host, the
    /// port and the mount point as the server itself reads them, so a server
    /// bound to one address, IPv6 included, is asked there.
    pub fn ping_url(&self) -> String {
        format!("http://{}:{}{}/api/v1/ping", self.probe_host(), self.port, self.base_path)
    }

    /// The address the probe reaches the server at: the loopback when it binds
    /// every address, which only a client can not name.
    fn probe_host(&self) -> String {
        match self.host.trim() {
            "" | "0.0.0.0" => "127.0.0.1".into(),
            "::" | "[::]" => "[::1]".into(),
            host => bracketed(host),
        }
    }

    /// Point the configuration at a database, moving its data directory with
    /// it.
    ///
    /// The two are one setting expressed as two fields, so assigning `db_path`
    /// alone leaves the key files and the backups behind at the old location.
    /// Production sets both in `from_env`, and only tests relocate a live config.
    #[cfg(test)]
    pub fn set_db_path(&mut self, path: PathBuf) {
        self.data_dir = data_dir_for(&path);
        self.db_path = path;
    }

    /// Path of the auto-generated master key, kept next to the database.
    pub fn secret_key_path(&self) -> PathBuf {
        self.data_dir.join("routarr.key")
    }

    /// Where the generated password is written on first start.
    ///
    /// Beside the database and the API key, 0600, for the same reason: it is
    /// printed once and an operator who missed the line needs it back.
    pub fn password_path(&self) -> PathBuf {
        self.data_dir.join("routarr.password")
    }

    /// Refuse a configuration the server cannot honour, before it serves.
    ///
    /// Called once by `main`, so a value that would fail later fails at the
    /// start with a sentence naming the variable, rather than as a panic from
    /// a dependency, or as a feature that silently does nothing.
    pub fn validate(&self) -> AppResult<()> {
        validate_base_path(&self.base_path)?;
        for origin in &self.cors_origins {
            validate_origin(origin)?;
        }
        for (variable, root, _) in self.source_roots() {
            validate_source_root(variable, root)?;
        }
        if let Some(key) = &self.api_key
            && key.chars().count() < MIN_PINNED_KEY_LENGTH
        {
            const VARIABLE: &str = "ROUTARR_API_KEY";
            return Err(AppError::Config(format!(
                "{VARIABLE} is {} characters long, short enough to be guessed, and it opens \
                 everything. Use at least {MIN_PINNED_KEY_LENGTH}, such as the output of \
                 `openssl rand -hex 32`, or unset it to use the generated key",
                key.chars().count()
            )));
        }
        if matches!(self.auth_mode, AuthMode::Oidc) {
            self.validate_oidc()?;
        }
        // Zero is not "no timeout": every outbound call fails at once as
        // unreachable, and nothing on screen says why.
        if self.http_timeout.is_zero() {
            return Err(AppError::Config(
                "ROUTARR_HTTP_TIMEOUT_SECS: 0 would make every call to an Arr or a source fail \
                 at once. Unset it for the default of 20 seconds"
                    .into(),
            ));
        }
        if self.library_timeout.is_zero() {
            return Err(AppError::Config(
                "ROUTARR_LIBRARY_TIMEOUT_SECS: 0 would make every library listing fail at once. \
                 Unset it for the default of 300 seconds"
                    .into(),
            ));
        }
        if self.max_library_bytes == 0 {
            return Err(AppError::Config(
                "ROUTARR_MAX_LIBRARY_MIB: 0 would refuse every library. Unset it for the default \
                 of 256 MiB"
                    .into(),
            ));
        }
        Ok(())
    }

    /// The four an OIDC sign-in needs, and its two URLs over TLS.
    ///
    /// The flow does not verify the ID token's signature (see
    /// `services::oidc`) because the exchange happens over TLS. A provider
    /// reached in the clear voids that, so it is refused here rather than
    /// trusted at sign-in. Missing values are refused here too: each of the
    /// four is needed before anyone can sign in, and a start that names the
    /// missing one beats a 500 on the first attempt.
    fn validate_oidc(&self) -> AppResult<()> {
        const ISSUER: &str = "ROUTARR_OIDC_ISSUER";
        const CLIENT_ID: &str = "ROUTARR_OIDC_CLIENT_ID";
        const CLIENT_SECRET: &str = "ROUTARR_OIDC_CLIENT_SECRET";
        const REDIRECT: &str = "ROUTARR_OIDC_REDIRECT_URL";
        let required = |value: &Option<String>, name: &str| {
            value.clone().ok_or_else(|| {
                AppError::Config(format!("{name} is not set, and ROUTARR_AUTH=oidc needs it"))
            })
        };
        let issuer = required(&self.oidc_issuer, ISSUER)?;
        required(&self.oidc_client_id, CLIENT_ID)?;
        required(&self.oidc_client_secret, CLIENT_SECRET)?;
        let redirect = required(&self.oidc_redirect_url, REDIRECT)?;
        if self.oidc_allowed_subjects.is_empty()
            && self.oidc_allowed_groups.is_empty()
            && !self.oidc_allow_anyone
        {
            const SUBJECTS: &str = "ROUTARR_OIDC_ALLOWED_SUBJECTS";
            const GROUPS: &str = "ROUTARR_OIDC_ALLOWED_GROUPS";
            const ANYONE: &str = "ROUTARR_OIDC_ALLOW_ANYONE";
            return Err(AppError::Config(format!(
                "With ROUTARR_AUTH=oidc and nobody named, every account the provider \
                 authenticates would get full access. Set {SUBJECTS} to the sub of each person, \
                 or {GROUPS} to the groups whose members may sign in, or {ANYONE}=true to let \
                 every account of the provider in"
            )));
        }
        for (name, url) in [(ISSUER, issuer), (REDIRECT, redirect)] {
            if !reaches_over_tls(&url) {
                return Err(AppError::Config(format!(
                    "{name}: '{url}' must start with https://, because the sign-in trusts the channel \
                     to the provider in place of a token signature. Plain http:// is accepted \
                     for localhost only"
                )));
            }
        }
        Ok(())
    }

    /// Path of the auto-generated API key, kept next to the database.
    ///
    /// A separate file from the master key on purpose: this one is meant to be
    /// read and copied into a client, the other must never leave the host.
    pub fn api_key_path(&self) -> PathBuf {
        self.data_dir.join("routarr.api_key")
    }

    /// Config used by tests: in-memory database, no external calls.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        const UNREACHABLE_SOURCE: &str = "http://127.0.0.1:1";
        Self {
            base_path: String::new(),
            host: "127.0.0.1".into(),
            port: 0,
            db_path: PathBuf::from(":memory:"),
            // `:memory:` is not a file, so there is no directory beside it.
            // Deriving one anyway yields a *relative* path, and a test that
            // reaches the backup stage would create `backups/` in whatever
            // directory `cargo test` runs from.
            data_dir: std::env::temp_dir().join(format!("routarr-tests-{}", std::process::id())),
            log_level: "error".into(),
            log_format: "text".into(),
            startup_notes: Vec::new(),
            frontend_dir: PathBuf::from("/nonexistent"),
            tmdb_api_key: None,
            api_key: None,
            // Tests drive the middleware directly and set a key when they mean
            // to. Generating one here would make every unauthenticated case
            // untestable.
            auth_mode: AuthMode::None,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_redirect_url: None,
            oidc_allowed_subjects: vec![],
            oidc_allowed_groups: vec![],
            oidc_groups_claim: "groups".into(),
            oidc_allow_anyone: false,
            cors_origins: vec![],
            allowed_hosts: vec![],
            trusted_proxies: vec![],
            http_timeout: Duration::from_millis(300),
            library_timeout: Duration::from_millis(900),
            max_library_bytes: 1 << 20,
            move_wait: Duration::from_secs(2),
            webhook_answer_wait: Duration::from_secs(5),
            secret_key: Some("dGVzdC1rZXktMzItYnl0ZXMtZm9yLXVuaXQtdGVzdCE=".into()),
            previous_secret_key: None,
            allow_new_master_key: false,
            metadata_concurrency: 2,
            // Port 1 on the loopback, where nothing listens: a test that lists
            // a keyless source, or sets a key, and then probes or enriches is
            // refused at once instead of reaching the real service. A test
            // that means to reach one points the source at a stand-in.
            tmdb_base_url: UNREACHABLE_SOURCE.to_string(),
            omdb_api_key: None,
            omdb_base_url: UNREACHABLE_SOURCE.to_string(),
            tvdb_api_key: None,
            tvdb_pin: None,
            tvdb_base_url: UNREACHABLE_SOURCE.to_string(),
            anilist_base_url: UNREACHABLE_SOURCE.to_string(),
            jikan_base_url: UNREACHABLE_SOURCE.to_string(),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

/// A `Host` value without its port, in lower case: `NAS.lan:9876` is
/// `nas.lan`, `[::1]:9876` is `::1`.
pub fn host_name(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default().to_string();
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            name.to_string()
        }
        _ => host,
    }
}

/// A host as it is written beside a port: an IPv6 address in brackets.
fn bracketed(host: &str) -> String {
    match host.parse::<std::net::Ipv6Addr>() {
        Ok(_) => format!("[{host}]"),
        Err(_) => host.to_string(),
    }
}

/// A path variable, or its default when unset or blank: an empty
/// `ROUTARR_DB_PATH` would open a private temporary database per pooled
/// connection, and an empty frontend directory would switch to API only.
fn path_or(raw: Option<String>, default: impl FnOnce() -> PathBuf) -> PathBuf {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default)
}

/// A comma separated variable, each entry trimmed, the empty ones dropped.
fn list(key: &str) -> Vec<String> {
    non_empty(key)
        .map(|value| {
            value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(String::from).collect()
        })
        .unwrap_or_default()
}

/// The passphrase `routarr restore` and `routarr decrypt-backup` open an
/// encrypted archive with, when the database in place does not hold it. Read
/// by those commands alone, and kept out of `Config`, which the server holds
/// for as long as it runs.
pub fn archive_passphrase() -> Option<String> {
    non_empty("ROUTARR_BACKUP_PASSPHRASE")
}

fn non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> AppResult<T> {
    parse_setting(key, std::env::var(key).ok(), default)
}

/// The value of a variable, or its default when unset or blank, and a
/// refusal naming the variable when it is set to something unreadable.
fn parse_setting<T: std::str::FromStr>(key: &str, raw: Option<String>, default: T) -> AppResult<T> {
    match raw.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        None => Ok(default),
        Some(value) => value.parse().map_err(|_| {
            AppError::Config(format!(
                "{key}: '{value}' is not a value this setting can take. Unset it for the default"
            ))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blank path is no path: unset, the default stands.
    #[test]
    fn a_blank_path_variable_reads_as_unset() {
        let default = || PathBuf::from("./data/routarr.db");
        for blank in [Some(String::new()), Some("   ".into()), None] {
            assert_eq!(path_or(blank.clone(), default), default(), "{blank:?}");
        }
        assert_eq!(
            path_or(Some(" /srv/routarr.db ".into()), default),
            PathBuf::from("/srv/routarr.db")
        );
    }

    /// A value read and set aside is said once logging is up: a typo in the
    /// mode keeps the API key and says so, `warning` is the level it names,
    /// and an unknown level or format falls back with a word.
    #[test]
    fn a_mount_point_the_router_cannot_serve_is_refused_by_name() {
        let mut config = Config::for_tests();
        for refused in ["{x}", "a{b", "*x", "a?b", "a#b", "my app", "a\"b", "a//b", "a/../b"] {
            config.base_path = normalise_base_path(refused);
            let err = config.validate().expect_err(refused).to_string();
            assert!(err.contains("ROUTARR_BASE_PATH"), "{refused}: {err}");
        }
        for served in ["routarr", "apps/routarr", "", "r.a_t~-1"] {
            config.base_path = normalise_base_path(served);
            assert!(config.validate().is_ok(), "{served}");
        }
    }

    #[test]
    fn a_value_not_understood_is_said_rather_than_swallowed() {
        assert_eq!(AuthMode::parse("nome"), (AuthMode::ApiKey, true));
        assert_eq!(AuthMode::parse("oidc"), (AuthMode::Oidc, false));
        assert_eq!(log_level_from("warning"), ("warn".to_string(), false));
        assert_eq!(log_level_from("DEBUG"), ("debug".to_string(), false));
        assert_eq!(log_level_from("verbose"), ("info".to_string(), true));
        assert_eq!(log_format_from("JSON"), ("json".to_string(), false));
        assert_eq!(log_format_from("pretty"), ("text".to_string(), true));
        assert_eq!(concurrency_from(100), (16, true));
        assert_eq!(concurrency_from(0), (1, true));
        assert_eq!(concurrency_from(4), (4, false));
    }

    /// The image's probe reads this address. Written raw from the variable,
    /// `routarr` would give `:9876routarr` and a container unhealthy for ever,
    /// and `/routarr/` a double slash the interface's fallback page answers.
    #[test]
    fn the_probe_asks_the_address_the_server_serves_however_the_mount_point_is_written() {
        let mut config = Config::for_tests();
        config.port = 9876;
        for written in ["routarr", "/routarr", "/routarr/", " routarr/ "] {
            config.base_path = normalise_base_path(written);
            assert_eq!(
                config.ping_url(),
                "http://127.0.0.1:9876/routarr/api/v1/ping",
                "{written:?}"
            );
        }
        config.base_path = normalise_base_path("");
        assert_eq!(config.ping_url(), "http://127.0.0.1:9876/api/v1/ping");

        // The host it binds, or the loopback when it binds every address.
        for (host, asked) in [
            ("0.0.0.0", "127.0.0.1"),
            ("::", "[::1]"),
            ("::1", "[::1]"),
            ("192.168.1.4", "192.168.1.4"),
        ] {
            config.host = host.into();
            assert_eq!(config.ping_url(), format!("http://{asked}:9876/api/v1/ping"), "{host}");
        }
    }

    /// `.env.example` is the only place most people will ever read the list of
    /// knobs, so a variable the code honours but the sample omits is invisible,
    /// `ROUTARR_BASE_PATH` among them, the one people need when the app
    /// misbehaves behind a reverse proxy, the hardest place to guess.
    ///
    /// Read at run time rather than `include_str!`'d: the Docker build copies
    /// only `src`, `migrations` and `locales`, so embedding the sample would
    /// make the image unbuildable.
    #[test]
    fn every_environment_variable_the_code_reads_is_in_the_sample_env() {
        let root = env!("CARGO_MANIFEST_DIR");
        let source = std::fs::read_to_string(format!("{root}/src/config.rs")).unwrap();
        let sample = std::fs::read_to_string(format!("{root}/.env.example")).unwrap();

        let mut read: Vec<String> = Vec::new();
        for line in source.lines() {
            // Every quoted name with a known prefix, the `env_or("NAME", …)` and
            // `non_empty("NAME")` call sites among them.
            for prefix in ["ROUTARR_", "TMDB_", "OMDB_", "TVDB_"] {
                let Some(at) = line.find(&format!("\"{prefix}")) else { continue };
                let rest = &line[at + 1..];
                let Some(end) = rest.find('"') else { continue };
                read.push(rest[..end].to_string());
            }
        }
        read.sort();
        read.dedup();
        // The guard on the scan: stop matching and the assertion below passes
        // having read nothing at all.
        assert!(read.len() >= 15, "only {} variable(s) found in config.rs", read.len());

        let missing: Vec<&String> = read.iter().filter(|name| !sample.contains(*name)).collect();
        assert!(missing.is_empty(), "undocumented in .env.example: {missing:?}");
    }

    /// And nothing is documented that the code does not read.
    ///
    /// The other direction, and it fails differently: an operator sets a
    /// variable the sample still advertises, nothing happens, and nothing says
    /// so. A setting that silently does nothing is worse than one that is
    /// missing, because the sample is where people look for what exists.
    ///
    /// `config.rs` is the only file outside the tests that reads the
    /// environment (`grep 'env::var'` finds only `tests/live_sources.rs`
    /// elsewhere), which is what makes scanning this one file enough.
    #[test]
    fn every_variable_the_sample_env_advertises_is_one_the_code_reads() {
        let root = env!("CARGO_MANIFEST_DIR");
        let source = std::fs::read_to_string(format!("{root}/src/config.rs")).unwrap();
        let sample = std::fs::read_to_string(format!("{root}/.env.example")).unwrap();

        let advertised: Vec<&str> = sample
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix('#').unwrap_or(line).trim().split('=').next())
            .map(str::trim)
            .filter(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                    && ["ROUTARR_", "TMDB_", "OMDB_", "TVDB_"].iter().any(|p| name.starts_with(p))
            })
            .collect();
        assert!(
            advertised.len() >= 15,
            "only {} variable(s) read out of .env.example",
            advertised.len()
        );

        let orphans: Vec<&&str> =
            advertised.iter().filter(|name| !source.contains(&format!("\"{name}\""))).collect();
        assert!(orphans.is_empty(), "advertised but never read: {orphans:?}");
    }

    #[test]
    fn an_in_memory_database_never_derives_a_relative_data_directory() {
        // `Path::new(":memory:").parent()` is `Some("")`, so the obvious
        // derivation yields `backups`, `routarr.key` and `routarr.api_key`
        // relative to the working directory, and a test that reaches the
        // backup stage would leave a directory in the source tree.
        assert_eq!(data_dir_for(Path::new(":memory:")), PathBuf::from("."));

        let config = Config::for_tests();
        for path in [config.data_dir.clone(), config.secret_key_path(), config.api_key_path()] {
            assert!(
                path.is_absolute(),
                "{} is relative, so a test would write it beside the sources",
                path.display()
            );
        }
    }

    #[test]
    fn the_data_directory_is_the_one_holding_the_database() {
        assert_eq!(
            data_dir_for(Path::new("/var/lib/routarr/routarr.db")),
            PathBuf::from("/var/lib/routarr")
        );
        assert_eq!(data_dir_for(Path::new("./data/routarr.db")), PathBuf::from("./data"));
        // No directory at all: the working directory, spelled out.
        assert_eq!(data_dir_for(Path::new("routarr.db")), PathBuf::from("."));
    }

    /// Everything on disk hangs off `data_dir`, and `set_db_path` is the only
    /// thing that moves it. Assigning `db_path` alone would leave the master
    /// key and the backups at the previous location, where the next start
    /// would not find them, and would generate a *new* key that opens none of
    /// the stored Arr credentials.
    #[test]
    fn moving_the_database_takes_the_keys_and_the_backups_with_it() {
        let mut config = Config::for_tests();
        config.set_db_path(PathBuf::from("/srv/routarr/data/routarr.db"));

        assert_eq!(config.data_dir, PathBuf::from("/srv/routarr/data"));
        assert_eq!(config.secret_key_path(), PathBuf::from("/srv/routarr/data/routarr.key"));
        assert_eq!(config.api_key_path(), PathBuf::from("/srv/routarr/data/routarr.api_key"));
    }

    /// A wildcard handed to `AllowOrigin::list` panics, so an installation that
    /// set the most obvious value would never start, with a message naming
    /// tower-http rather than the variable.
    #[test]
    fn a_wildcard_origin_is_refused_by_name() {
        let err = validate_origin("*").expect_err("a wildcard has to be refused");
        let message = err.to_string();
        assert!(message.contains("ROUTARR_CORS_ORIGINS"), "the message names the variable");
        assert!(message.contains("wildcard"), "and says what is wrong: {message}");
    }

    /// A browser sends `scheme://host[:port]` and tower-http compares the
    /// header verbatim, so anything else can never match. Accepted in silence,
    /// it would leave CORS looking configured and doing nothing, while the
    /// startup line still counts it.
    #[test]
    fn an_origin_that_could_never_match_is_refused() {
        for bad in ["example.com", "//example.com", "ftp://example.com", "https://", "not a url"] {
            assert!(validate_origin(bad).is_err(), "'{bad}' was accepted");
        }
        // A path is not part of an origin.
        assert!(validate_origin("https://example.com/app").is_err());
        assert!(validate_origin("https://example.com/?a=1").is_err());
    }

    #[test]
    fn a_real_origin_is_accepted_with_or_without_a_port() {
        for good in ["http://localhost:3000", "https://routarr.example", "http://192.168.1.4:9876"]
        {
            assert!(validate_origin(good).is_ok(), "'{good}' was refused");
        }
    }

    #[test]
    fn a_config_with_no_origins_validates() {
        assert!(Config::for_tests().validate().is_ok());
    }

    /// `unwrap_or(default)` on a failed parse would run the server on the
    /// default while the operator believes their value is in force.
    #[test]
    fn an_unreadable_variable_is_refused_by_name_rather_than_replaced_by_its_default() {
        assert_eq!(parse_setting("ROUTARR_PORT", None, 9876u16).unwrap(), 9876);
        assert_eq!(parse_setting("ROUTARR_PORT", Some("  ".into()), 9876u16).unwrap(), 9876);
        assert_eq!(parse_setting("ROUTARR_PORT", Some(" 8080 ".into()), 9876u16).unwrap(), 8080);
        let err = parse_setting("ROUTARR_PORT", Some("987 6".into()), 9876u16).unwrap_err();
        assert!(err.to_string().contains("ROUTARR_PORT"), "{err}");
        let err =
            parse_setting("ROUTARR_HTTP_TIMEOUT_SECS", Some("abc".into()), 20u64).unwrap_err();
        assert!(err.to_string().contains("ROUTARR_HTTP_TIMEOUT_SECS"), "{err}");
    }

    /// One `{config:?}` written while diagnosing would otherwise print the
    /// master key, the OIDC secret and the API key into the log.
    #[test]
    fn the_configuration_prints_no_secret() {
        let mut config = Config::for_tests();
        let secrets = [
            "tmdb-secret-1",
            "api-secret-2",
            "oidc-secret-3",
            "master-secret-4",
            "previous-secret-5",
            "omdb-secret-6",
            "tvdb-secret-7",
            "tvdb-pin-8",
        ];
        config.tmdb_api_key = Some(secrets[0].into());
        config.api_key = Some(secrets[1].into());
        config.oidc_client_secret = Some(secrets[2].into());
        config.secret_key = Some(secrets[3].into());
        config.previous_secret_key = Some(secrets[4].into());
        config.omdb_api_key = Some(secrets[5].into());
        config.tvdb_api_key = Some(secrets[6].into());
        config.tvdb_pin = Some(secrets[7].into());

        let printed = format!("{config:?} {config:#?}");
        for secret in secrets {
            assert!(!printed.contains(secret), "{secret} is printed");
        }
        assert!(printed.contains("api_key: Some(\"<redacted>\")"), "{printed}");
        assert!(printed.contains("tmdb_base_url"), "the other fields are printed: {printed}");
    }

    /// The pinned key opens everything in every mode, so one short enough to
    /// guess is refused by name rather than served.
    #[test]
    fn a_pinned_api_key_too_short_to_hold_is_refused_by_name() {
        let mut config = Config::for_tests();
        config.api_key = Some("routarr".into());
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ROUTARR_API_KEY") && err.contains("32"), "{err}");

        config.api_key = Some("a".repeat(MIN_PINNED_KEY_LENGTH));
        assert!(config.validate().is_ok());
    }

    /// An address, or a range a proxy recreated on its network stays inside.
    #[test]
    fn a_trusted_proxy_is_an_address_or_a_range() {
        let range = Network::parse("172.18.0.0/16").unwrap();
        assert!(range.contains("172.18.4.7".parse().unwrap()));
        assert!(!range.contains("172.19.0.1".parse().unwrap()));
        assert!(range.contains("::ffff:172.18.0.3".parse().unwrap()));
        let one = Network::parse("172.18.0.2").unwrap();
        assert!(one.contains("172.18.0.2".parse().unwrap()));
        assert!(!one.contains("172.18.0.3".parse().unwrap()));
        let v6 = Network::parse("fd00:1::/64").unwrap();
        assert!(v6.contains("fd00:1::9".parse().unwrap()));
        assert!(!v6.contains("fd00:2::9".parse().unwrap()));
        for refused in ["172.18.0.0/33", "fd00::/129", "proxy", "172.18.0.0/", ""] {
            assert!(Network::parse(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_zero_timeout_or_cap_is_refused_at_startup() {
        let refused = |change: fn(&mut Config)| {
            let mut config = Config::for_tests();
            change(&mut config);
            config.validate().unwrap_err().to_string()
        };
        let err = refused(|config| config.http_timeout = Duration::ZERO);
        assert!(err.contains("ROUTARR_HTTP_TIMEOUT_SECS"), "{err}");
        let err = refused(|config| config.library_timeout = Duration::ZERO);
        assert!(err.contains("ROUTARR_LIBRARY_TIMEOUT_SECS"), "{err}");
        let err = refused(|config| config.max_library_bytes = 0);
        assert!(err.contains("ROUTARR_MAX_LIBRARY_MIB"), "{err}");
    }

    /// A container that mounts `/data` and not `/config` keeps its library,
    /// and is told how to follow the image's layout. A path set to anything
    /// else, or a `/config` that holds a database, is opened as it is.
    #[test]
    fn a_database_left_in_data_is_still_opened() {
        let image = PathBuf::from(IMAGE_DB_PATH);
        let only_data = |path: &Path| path == Path::new(DATA_DB_PATH);
        let (opened, note) = settled_db_path(image.clone(), only_data);
        assert_eq!(opened, PathBuf::from(DATA_DB_PATH));
        assert!(note.is_some_and(|note| note.contains("/config")));

        let both = |_: &Path| true;
        assert_eq!(settled_db_path(image.clone(), both), (image.clone(), None));
        let neither = |_: &Path| false;
        assert_eq!(settled_db_path(image, neither).0, PathBuf::from(IMAGE_DB_PATH));
        let elsewhere = PathBuf::from("/srv/routarr/routarr.db");
        assert_eq!(settled_db_path(elsewhere.clone(), only_data), (elsewhere, None));
    }

    /// A source's root that is no address a request can go to stops the start,
    /// naming its variable, where it would only show as the source being
    /// unreachable. A key sent over plain http is said at start.
    #[test]
    fn a_source_root_is_checked_at_start() {
        let mut config = Config::for_tests();
        config.tmdb_base_url = "api.themoviedb.org/3".into();
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ROUTARR_TMDB_BASE_URL"), "{err}");
        config.tmdb_base_url = "ftp://mirror.lan/3".into();
        assert!(config.validate().is_err());

        let mut config = Config::for_tests();
        config.omdb_base_url = "http://mirror.lan".into();
        config.anilist_base_url = "http://mirror.lan/graphql".into();
        config.validate().unwrap();
        let notes = config.source_notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("ROUTARR_OMDB_BASE_URL"), "{notes:?}");
    }

    fn oidc_config() -> Config {
        let mut config = Config::for_tests();
        config.auth_mode = AuthMode::Oidc;
        config.oidc_issuer = Some("https://id.example".into());
        config.oidc_client_id = Some("routarr".into());
        config.oidc_client_secret = Some("shhh".into());
        config.oidc_redirect_url = Some("https://routarr.example/api/v1/auth/oidc/callback".into());
        config.oidc_allowed_subjects = vec!["u-42".into()];
        config
    }

    /// A provider left at its defaults lets every account of its directory use
    /// every client. Naming nobody, the mode would let all of them in with full
    /// access, so it refuses to start unless the operator says who, or says
    /// in so many words that anyone may.
    #[test]
    fn oidc_mode_without_an_allow_list_refuses_to_start_unless_told_to() {
        let mut config = oidc_config();
        config.oidc_allowed_subjects.clear();
        let err = config.validate().unwrap_err().to_string();
        for name in [
            "ROUTARR_OIDC_ALLOWED_SUBJECTS",
            "ROUTARR_OIDC_ALLOWED_GROUPS",
            "ROUTARR_OIDC_ALLOW_ANYONE",
        ] {
            assert!(err.contains(name), "{name} missing from: {err}");
        }

        config.oidc_allowed_groups = vec!["media".into()];
        assert!(config.validate().is_ok());
        config.oidc_allowed_groups.clear();
        config.oidc_allow_anyone = true;
        assert!(config.validate().is_ok());
    }

    /// Each of the four is needed before anyone can sign in, and a start that
    /// names the missing one beats a 500 on the first attempt.
    #[test]
    fn oidc_mode_names_each_variable_it_is_missing_at_startup() {
        assert!(oidc_config().validate().is_ok());
        for (name, clear) in [
            ("ROUTARR_OIDC_ISSUER", (|c: &mut Config| c.oidc_issuer = None) as fn(&mut Config)),
            ("ROUTARR_OIDC_CLIENT_ID", |c| c.oidc_client_id = None),
            ("ROUTARR_OIDC_CLIENT_SECRET", |c| c.oidc_client_secret = None),
            ("ROUTARR_OIDC_REDIRECT_URL", |c| c.oidc_redirect_url = None),
        ] {
            let mut config = oidc_config();
            clear(&mut config);
            let err = config.validate().expect_err(name).to_string();
            assert!(err.contains(name), "{err}");
        }
    }

    /// The flow does not verify the token's signature because the exchange
    /// happens over TLS, and a provider reached in the clear voids that. The
    /// machine itself is the one exception, since nothing else is on the wire.
    #[test]
    fn oidc_mode_refuses_a_provider_or_a_redirect_reached_in_the_clear() {
        for (name, set) in [
            (
                "ROUTARR_OIDC_ISSUER",
                (|c: &mut Config, v: &str| c.oidc_issuer = Some(v.into())) as fn(&mut Config, &str),
            ),
            ("ROUTARR_OIDC_REDIRECT_URL", |c, v| c.oidc_redirect_url = Some(v.into())),
        ] {
            let mut config = oidc_config();
            set(&mut config, "http://authelia:9091/x");
            let err = config.validate().expect_err(name).to_string();
            assert!(err.contains(name) && err.contains("https"), "{err}");

            for local in
                ["http://localhost:9091/x", "http://127.0.0.1:9091/x", "http://[::1]:9091/x"]
            {
                let mut config = oidc_config();
                set(&mut config, local);
                assert!(config.validate().is_ok(), "{local} is the machine itself");
            }
        }
        // Not a mode that reads them: an unused variable is not a fault.
        let mut config = oidc_config();
        config.auth_mode = AuthMode::Forms;
        config.oidc_issuer = Some("http://authelia:9091".into());
        assert!(config.validate().is_ok());
    }

    /// `bind_address` is what the listener is given. A mistake here is a server
    /// that answers on an address nobody documented.
    #[test]
    fn the_bind_address_joins_the_configured_host_and_port() {
        let mut config = Config::for_tests();
        config.host = "127.0.0.1".into();
        config.port = 9876;
        assert_eq!(config.bind_address(), "127.0.0.1:9876");
        config.host = "::1".into();
        assert_eq!(config.bind_address(), "[::1]:9876");
    }
}
