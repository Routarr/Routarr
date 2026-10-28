use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_TMDB_BASE_URL: &str = "https://api.themoviedb.org/3";

/// `ROUTARR_HTTP_TIMEOUT_SECS` when unset.
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 20;

/// `ROUTARR_MAX_LIBRARY_MIB` when unset.
pub(crate) const DEFAULT_MAX_LIBRARY_MIB: usize = 256;

/// How long a move of files is polled at its quickest: a rename ends in a second, a copy
/// across disks takes minutes per film.
const MOVE_WAIT: Duration = Duration::from_secs(60);

/// How long a webhook delivery is answered within, under the timeout an Arr
/// gives a notification.
const WEBHOOK_ANSWER_WAIT: Duration = Duration::from_secs(20);

/// How long an unattended apply waits for the one running. A run moving files holds the lock as
/// long as its copies last, hours for a library: past this wait the decision stays pending for
/// the sweep after that run.
const UNATTENDED_WAIT: Duration = Duration::from_secs(300);

/// How long a webhook delivery waits for the one before it, on its own task. A season import
/// arrives one file at a time, each answered before the next is sent, so it never reaches it.
const DELIVERY_WAIT: Duration = Duration::from_secs(20);

/// The directory a database file lives in. The parent of `:memory:` is `Some("")`, an empty
/// path that `unwrap_or` never catches and `join` turns into one relative to the working directory.
fn data_dir_for(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Where the image keeps the database, as Radarr and Sonarr keep theirs.
const IMAGE_DB_PATH: &str = "/config/routarr.db";

/// Where a container that mounts `/data` and not `/config` holds its database.
const DATA_DB_PATH: &str = "/data/routarr.db";

/// The database to open: `configured`, unless it is the image's own path, holds nothing yet,
/// and `/data` holds a database, which is opened rather than an empty library in its place.
/// The note says how to follow the image's layout.
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

/// How a caller proves who it is, named after the Servarr applications' `None`, `Forms` and
/// `External`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No credential asked for. Only reachable by asking for it.
    None,
    /// The API key, in `X-Api-Key` or `Authorization: Bearer`.
    ApiKey,
    /// A username and a password, exchanged for a session cookie. The API key keeps
    /// working beside it: a machine client cannot hold a cookie, and a header is immune to
    /// the cross-site request forgery a cookie invites.
    Forms,
    /// An OpenID Connect provider authenticates, and Routarr lets in the subjects and groups
    /// the operator names, with full access or to read alone. No user table: the subject is
    /// what it records.
    Oidc,
    /// A reverse proxy authenticates, and Routarr asks for nothing. It reads no identity
    /// header, as Radarr's `External` does, so there is none to forge: nothing may reach the
    /// port but through the proxy, which the operator guarantees and the start states.
    External,
}

impl AuthMode {
    /// The name `ROUTARR_AUTH` takes and a session records. Stated once: a session is honoured
    /// only by the mode whose name it carries, so two spellings would refuse every session.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ApiKey => "apikey",
            Self::Forms => "forms",
            Self::Oidc => "oidc",
            Self::External => "external",
        }
    }

    /// The mode a value of `ROUTARR_AUTH` names, and whether it named none. Anything else falls
    /// back to the API key: a typo must not be the way an installation ends up open.
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

/// The tracing filter `RUST_LOG` names, kept when it parses, and what the
/// start says of it: a filter left in an environment replaces the level set
/// in `ROUTARR_LOG_LEVEL` without a word otherwise.
fn log_filter_from(raw: Option<String>) -> (Option<String>, Option<String>) {
    let Some(filter) = raw else { return (None, None) };
    let variable = "RUST_LOG";
    if tracing_subscriber::EnvFilter::try_new(&filter).is_ok() {
        let note = format!("{variable} is '{filter}' and replaces the level of ROUTARR_LOG_LEVEL");
        (Some(filter), Some(note))
    } else {
        let note = format!("{variable} is '{filter}', which no filter reads: it is not applied");
        (None, Some(note))
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

/// What a start says of the ranges `ROUTARR_TRUSTED_PROXIES` lists: every peer inside one
/// chooses the address its sign-ins count against.
fn proxy_range_notes(proxies: &[Network]) -> Vec<String> {
    let ranges: Vec<String> = proxies
        .iter()
        .filter(|network| network.prefix < if network.base.is_ipv4() { 32 } else { 128 })
        .map(|network| format!("{}/{}", network.base, network.prefix))
        .collect();
    if ranges.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "ROUTARR_TRUSTED_PROXIES lists {}: every peer inside, each container of a shared Docker \
         network included, chooses the address its sign-ins are counted against. List the \
         proxy's own address instead",
        ranges.join(", ")
    )]
}

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

/// One entry of `ROUTARR_CORS_ORIGINS`, as a browser sends it: `scheme://host[:port]`, which
/// tower-http compares verbatim, so any other shape would look configured and never match.
/// `*` is refused: this layer sends credentials, which the Fetch standard forbids beside a
/// wildcard, and `AllowOrigin::list` panics on one.
fn validate_origin(origin: &str) -> AppResult<()> {
    // Named apart from the message, as in `Config::from_env`.
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
    // `Url::parse` gives a bare origin the path "/": anything longer is a path no browser sends.
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(bad("must be an origin (scheme, host and port, with no path)"));
    }
    Ok(())
}

/// The origins of `ROUTARR_CORS_ORIGINS` as a browser sends them, since the comparison is
/// verbatim: scheme and host in lower case, the default port dropped. A value that is no
/// origin stays as written, for `validate_origin` to name.
fn origins_from(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(|origin| origin.trim().trim_end_matches('/'))
        .filter(|origin| !origin.is_empty())
        .map(|origin| {
            reqwest::Url::parse(origin)
                .ok()
                .filter(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.path() == "/"
                        && url.query().is_none()
                        && url.fragment().is_none()
                })
                .map_or_else(|| origin.to_string(), |url| url.origin().ascii_serialization())
        })
        .collect()
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
    /// `ROUTARR_BACKUP_COPY_DIR`, a second folder each archive is copied to, mounted from
    /// another disk (`services::backup::copy`).
    pub backup_copy_dir: Option<PathBuf>,
    pub log_level: String,
    /// `text` (default) or `json`, which is easier to ship to a log collector.
    pub log_format: String,
    /// `RUST_LOG`, a tracing filter that replaces `log_level` whole when it
    /// parses.
    pub log_filter: Option<String>,
    /// What was read and set aside for a default, said as warnings once the
    /// log is up: read before it, nothing would hear them.
    pub startup_notes: Vec<String>,
    pub frontend_dir: PathBuf,
    pub tmdb_api_key: Option<String>,
    /// `ROUTARR_API_KEY`, which wins over the stored key. The key in force is
    /// `AppState::api_key`, which also holds one generated at startup or minted from the interface.
    pub api_key: Option<String>,
    /// Turned off only deliberately, never by forgetting to set a key, as in Radarr and Sonarr.
    pub auth_mode: AuthMode,
    /// The OpenID Connect provider's issuer URL, whose well-known path states its endpoints:
    /// copied by hand, they are four more values to keep in step with another deployment.
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
    /// Those let in to read alone, what an application key with the read scope reads. Named
    /// in the lists above too, a person has full access.
    pub oidc_read_subjects: Vec<String>,
    pub oidc_read_groups: Vec<String>,
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
    /// How long an apply polls a move of files at its quickest. An Arr unread for five times it,
    /// or one move running past 360 times it, stops the run, the title left for a sync to settle.
    pub move_wait: Duration,
    /// How long a webhook delivery is answered within: past it the Arr is told
    /// the delivery was accepted, and the work goes on.
    pub webhook_answer_wait: Duration,
    /// How long an unattended apply waits for the apply running before it leaves its moves
    /// pending.
    pub unattended_wait: Duration,
    /// How long a webhook delivery waits for the one before it, and a delete then for a sync
    /// running, before it is dropped.
    pub delivery_wait: Duration,
    /// Master key sealing the stored secrets (the Arr and metadata source keys).
    /// Generated beside the database if absent.
    pub secret_key: Option<String>,
    /// Superseded master key, kept readable for one rotation.
    pub previous_secret_key: Option<String>,
    /// Lets a start make a new master key although the database holds values
    /// sealed with the one it had, which then have to be entered again.
    pub allow_new_master_key: bool,
    /// Max requests in flight per metadata source during enrichment, a ceiling: `rate_limit`
    /// paces how many are made, and a source may lower this on its own.
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
    /// AniList and Tenrai authenticate nothing, so they carry a root and no key.
    pub anilist_base_url: String,
    pub tenrai_base_url: String,
    /// The client ID MyAnimeList's official API reads public data with, one per installation.
    pub mal_client_id: Option<String>,
    pub myanimelist_base_url: String,
    /// Where the anime identifier table is downloaded from, when a person asks for it.
    pub anime_ids_url: String,
    /// Sub-path Routarr is mounted under, Servarr's "URL base": empty or `/something` with no
    /// trailing slash, so callers concatenate without guessing.
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
            backup_copy_dir,
            log_level,
            log_format,
            log_filter,
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
            oidc_read_subjects,
            oidc_read_groups,
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
            unattended_wait,
            delivery_wait,
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
            tenrai_base_url,
            mal_client_id,
            myanimelist_base_url,
            anime_ids_url,
            base_path,
        } = self;
        let hidden = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("Config")
            .field("host", host)
            .field("port", port)
            .field("db_path", db_path)
            .field("data_dir", data_dir)
            .field("backup_copy_dir", backup_copy_dir)
            .field("log_level", log_level)
            .field("log_format", log_format)
            .field("log_filter", log_filter)
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
            .field("oidc_read_subjects", oidc_read_subjects)
            .field("oidc_read_groups", oidc_read_groups)
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
            .field("unattended_wait", unattended_wait)
            .field("delivery_wait", delivery_wait)
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
            .field("tenrai_base_url", tenrai_base_url)
            .field("mal_client_id", &hidden(mal_client_id))
            .field("myanimelist_base_url", myanimelist_base_url)
            .field("anime_ids_url", anime_ids_url)
            .field("base_path", base_path)
            .finish()
    }
}

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

/// The mount point as `""` or `/segment[/segment…]`, however it is written: `routarr`,
/// `/routarr/` and `routarr/` give no double or missing slash in a generated URL.
pub fn normalise_base_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() { String::new() } else { format!("/{trimmed}") }
}

/// Where the built interface is, when nobody said: beside the binary in the image, one level
/// up under `cargo run` from `backend/`, which otherwise answers a 404 on every route.
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
    /// The configuration the environment sets. A value that cannot be read is an error naming
    /// its variable, never the default: `ROUTARR_PORT=987 6` would run on 9876 unnoticed.
    pub fn from_env() -> AppResult<Self> {
        Self::from_lookup(&process_env)
    }

    /// The configuration the variables `env` answers set, as [`Config::from_env`] reads them.
    fn from_lookup(env: Lookup<'_>) -> AppResult<Self> {
        let configured = path_or(env("ROUTARR_DB_PATH"), || PathBuf::from("./data/routarr.db"));
        let (db_path, settled) = settled_db_path(configured, Path::exists);
        // Each variable is named apart from its message: the sample-env test reads a quoted
        // `ROUTARR_*` as a variable, the whole literal when a message begins with the name.
        let mut startup_notes: Vec<String> = settled.into_iter().collect();
        let raw_auth = env_or(env, "ROUTARR_AUTH", "apikey");
        let (auth_mode, unknown) = AuthMode::parse(&raw_auth);
        if unknown {
            let variable = "ROUTARR_AUTH";
            startup_notes.push(format!(
                "{variable} is '{raw_auth}', which no mode is called: the API key is required"
            ));
        }
        let asked = env_parse(env, "ROUTARR_METADATA_CONCURRENCY", 4usize)?;
        let (metadata_concurrency, clamped) = concurrency_from(asked);
        if clamped {
            let variable = "ROUTARR_METADATA_CONCURRENCY";
            startup_notes.push(format!(
                "{variable} is {asked}, outside 1 to {MAX_METADATA_CONCURRENCY}: \
                 {metadata_concurrency}"
            ));
        }
        let raw_level = env_or(env, "ROUTARR_LOG_LEVEL", "info");
        let (log_level, unknown) = log_level_from(&raw_level);
        if unknown {
            let variable = "ROUTARR_LOG_LEVEL";
            startup_notes
                .push(format!("{variable} is '{raw_level}', which no level is called: info"));
        }
        let raw_format = env_or(env, "ROUTARR_LOG_FORMAT", "text");
        let (log_format, unknown) = log_format_from(&raw_format);
        if unknown {
            let variable = "ROUTARR_LOG_FORMAT";
            startup_notes
                .push(format!("{variable} is '{raw_format}', neither text nor json: text"));
        }
        let (log_filter, note) = log_filter_from(non_empty(env, "RUST_LOG"));
        startup_notes.extend(note);
        let trusted_proxies = non_empty(env, "ROUTARR_TRUSTED_PROXIES")
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
            .unwrap_or_default();
        startup_notes.extend(proxy_range_notes(&trusted_proxies));
        startup_notes.extend(time_zone_note(env("TZ").as_deref(), zone_known));
        startup_notes.extend(owner_variables_note(env("PUID").as_deref(), env("PGID").as_deref()));
        Ok(Self {
            host: env_or(env, "ROUTARR_HOST", "0.0.0.0"),
            port: env_parse(env, "ROUTARR_PORT", 9876)?,
            data_dir: data_dir_for(&db_path),
            db_path,
            backup_copy_dir: non_empty(env, "ROUTARR_BACKUP_COPY_DIR")
                .map(|dir| lexical(Path::new(&dir))),
            log_level,
            log_format,
            log_filter,
            startup_notes,
            frontend_dir: path_or(env("ROUTARR_FRONTEND_DIR"), default_frontend_dir),
            tmdb_api_key: non_empty(env, "TMDB_API_KEY"),
            api_key: non_empty(env, "ROUTARR_API_KEY"),
            auth_mode,
            oidc_issuer: non_empty(env, "ROUTARR_OIDC_ISSUER")
                .map(|v| v.trim_end_matches('/').to_string()),
            oidc_client_id: non_empty(env, "ROUTARR_OIDC_CLIENT_ID"),
            oidc_client_secret: non_empty(env, "ROUTARR_OIDC_CLIENT_SECRET"),
            oidc_redirect_url: non_empty(env, "ROUTARR_OIDC_REDIRECT_URL"),
            oidc_allowed_subjects: list(env, "ROUTARR_OIDC_ALLOWED_SUBJECTS"),
            oidc_allowed_groups: list(env, "ROUTARR_OIDC_ALLOWED_GROUPS"),
            oidc_read_subjects: list(env, "ROUTARR_OIDC_READ_SUBJECTS"),
            oidc_read_groups: list(env, "ROUTARR_OIDC_READ_GROUPS"),
            oidc_groups_claim: env_or(env, "ROUTARR_OIDC_GROUPS_CLAIM", "groups"),
            oidc_allow_anyone: env_switch(env, "ROUTARR_OIDC_ALLOW_ANYONE", false)?,
            cors_origins: origins_from(non_empty(env, "ROUTARR_CORS_ORIGINS")),
            trusted_proxies,
            allowed_hosts: allowed_hosts_from(non_empty(env, "ROUTARR_ALLOWED_HOSTS"))?,
            http_timeout: Duration::from_secs(env_parse(
                env,
                "ROUTARR_HTTP_TIMEOUT_SECS",
                DEFAULT_HTTP_TIMEOUT_SECS,
            )?),
            library_timeout: Duration::from_secs(env_parse(
                env,
                "ROUTARR_LIBRARY_TIMEOUT_SECS",
                crate::http::LIBRARY_TIMEOUT.as_secs(),
            )?),
            max_library_bytes: env_parse(env, "ROUTARR_MAX_LIBRARY_MIB", DEFAULT_MAX_LIBRARY_MIB)?
                .saturating_mul(1 << 20),
            move_wait: MOVE_WAIT,
            webhook_answer_wait: WEBHOOK_ANSWER_WAIT,
            unattended_wait: UNATTENDED_WAIT,
            delivery_wait: DELIVERY_WAIT,
            secret_key: non_empty(env, "ROUTARR_SECRET_KEY"),
            previous_secret_key: non_empty(env, "ROUTARR_PREVIOUS_SECRET_KEY"),
            allow_new_master_key: env_switch(env, "ROUTARR_ALLOW_NEW_MASTER_KEY", false)?,
            metadata_concurrency,
            tmdb_base_url: env_or(env, "ROUTARR_TMDB_BASE_URL", DEFAULT_TMDB_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            omdb_api_key: non_empty(env, "OMDB_API_KEY"),
            omdb_base_url: env_or(
                env,
                "ROUTARR_OMDB_BASE_URL",
                crate::integrations::omdb::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            tvdb_api_key: non_empty(env, "TVDB_API_KEY"),
            tvdb_pin: non_empty(env, "TVDB_SUBSCRIBER_PIN"),
            tvdb_base_url: env_or(
                env,
                "ROUTARR_TVDB_BASE_URL",
                crate::integrations::tvdb::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            anilist_base_url: env_or(
                env,
                "ROUTARR_ANILIST_BASE_URL",
                crate::integrations::anilist::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            tenrai_base_url: env_or(
                env,
                "ROUTARR_TENRAI_BASE_URL",
                crate::integrations::tenrai::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            mal_client_id: non_empty(env, "MAL_CLIENT_ID"),
            myanimelist_base_url: env_or(
                env,
                "ROUTARR_MYANIMELIST_BASE_URL",
                crate::integrations::myanimelist::DEFAULT_BASE_URL,
            )
            .trim_end_matches('/')
            .to_string(),
            anime_ids_url: env_or(
                env,
                "ROUTARR_ANIME_IDS_URL",
                crate::services::anime_ids::DEFAULT_URL,
            ),
            base_path: normalise_base_path(&env_or(env, "ROUTARR_BASE_PATH", "")),
        })
    }

    /// Each metadata source's root, by the variable that sets it, and whether
    /// a key travels with its requests.
    fn source_roots(&self) -> [(&'static str, &str, bool); 7] {
        [
            ("ROUTARR_TMDB_BASE_URL", &self.tmdb_base_url, true),
            ("ROUTARR_OMDB_BASE_URL", &self.omdb_base_url, true),
            ("ROUTARR_TVDB_BASE_URL", &self.tvdb_base_url, true),
            ("ROUTARR_ANILIST_BASE_URL", &self.anilist_base_url, false),
            ("ROUTARR_TENRAI_BASE_URL", &self.tenrai_base_url, false),
            ("ROUTARR_MYANIMELIST_BASE_URL", &self.myanimelist_base_url, true),
            ("ROUTARR_ANIME_IDS_URL", &self.anime_ids_url, false),
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
    /// every address, which a client cannot name.
    fn probe_host(&self) -> String {
        match self.host.trim() {
            "" | "0.0.0.0" => "127.0.0.1".into(),
            "::" | "[::]" => "[::1]".into(),
            host => bracketed(host),
        }
    }

    /// Point the configuration at a database and its data directory with it: assigning
    /// `db_path` alone leaves the key files and the backups at the old location.
    #[cfg(test)]
    pub fn set_db_path(&mut self, path: PathBuf) {
        self.data_dir = data_dir_for(&path);
        self.db_path = path;
    }

    /// Path of the auto-generated master key, kept next to the database.
    pub fn secret_key_path(&self) -> PathBuf {
        self.data_dir.join("routarr.key")
    }

    /// Where the generated password is written on first start, 0600, for the owner to read once.
    pub fn password_path(&self) -> PathBuf {
        self.data_dir.join("routarr.password")
    }

    /// Refuse a configuration the server cannot honour before it serves: a value that would
    /// fail later fails at the start, naming its variable, rather than as a panic from a
    /// dependency or a feature that does nothing.
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
        self.validate_backup_copy_dir()?;
        // Zero is not "no timeout": every outbound call would fail at once as unreachable.
        if self.http_timeout.is_zero() {
            return Err(AppError::Config(format!(
                "ROUTARR_HTTP_TIMEOUT_SECS: 0 would make every call to an Arr or a source fail \
                 at once. Unset it for the default of {DEFAULT_HTTP_TIMEOUT_SECS} seconds"
            )));
        }
        if self.library_timeout.is_zero() {
            return Err(AppError::Config(format!(
                "ROUTARR_LIBRARY_TIMEOUT_SECS: 0 would make every library listing fail at once. \
                 Unset it for the default of {} seconds",
                crate::http::LIBRARY_TIMEOUT.as_secs()
            )));
        }
        if self.max_library_bytes == 0 {
            return Err(AppError::Config(format!(
                "ROUTARR_MAX_LIBRARY_MIB: 0 would refuse every library. Unset it for the default \
                 of {DEFAULT_MAX_LIBRARY_MIB} MiB"
            )));
        }
        if self.port == 0 {
            return Err(AppError::Config(
                "ROUTARR_PORT: 0 asks the system for any free port, which no client and no \
                 health check can find. Unset it for 9876"
                    .into(),
            ));
        }
        Ok(())
    }

    /// The copy folder, whole and apart from the data folder: a copy on the disk it copies
    /// saves nothing from that disk's loss. Never looked at here, since a NAS may sleep.
    fn validate_backup_copy_dir(&self) -> AppResult<()> {
        const VARIABLE: &str = "ROUTARR_BACKUP_COPY_DIR";
        let Some(copy) = &self.backup_copy_dir else { return Ok(()) };
        if !copy.is_absolute() {
            return Err(AppError::Config(format!(
                "{VARIABLE} is {}, a relative path. Give the whole path of the folder the \
                 copies go to",
                copy.display()
            )));
        }
        let data = std::path::absolute(&self.data_dir).unwrap_or_else(|_| self.data_dir.clone());
        let (copy, data) = (lexical(copy), lexical(&data));
        if copy.starts_with(&data) || data.starts_with(&copy) {
            return Err(AppError::Config(format!(
                "{VARIABLE} is {}, which holds or lies in the data folder {}. Copy to a folder \
                 of its own, mounted from another disk",
                copy.display(),
                data.display()
            )));
        }
        Ok(())
    }

    /// The four values an OIDC sign-in needs, and its two URLs over TLS: the flow trusts the
    /// TLS channel in place of the ID token's signature (`services::oidc`), so a provider in
    /// the clear is refused here, as a missing value is, rather than at the first sign-in.
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
        const SUBJECTS: &str = "ROUTARR_OIDC_ALLOWED_SUBJECTS";
        const GROUPS: &str = "ROUTARR_OIDC_ALLOWED_GROUPS";
        const ANYONE: &str = "ROUTARR_OIDC_ALLOW_ANYONE";
        const READ_SUBJECTS: &str = "ROUTARR_OIDC_READ_SUBJECTS";
        const READ_GROUPS: &str = "ROUTARR_OIDC_READ_GROUPS";
        let readers = !self.oidc_read_subjects.is_empty() || !self.oidc_read_groups.is_empty();
        if self.oidc_allowed_subjects.is_empty()
            && self.oidc_allowed_groups.is_empty()
            && !self.oidc_allow_anyone
        {
            if readers {
                return Err(AppError::Config(format!(
                    "{READ_SUBJECTS} or {READ_GROUPS} names who may read, and nobody may sign \
                     in to run Routarr. Set {SUBJECTS} or {GROUPS} for whoever does"
                )));
            }
            return Err(AppError::Config(format!(
                "With ROUTARR_AUTH=oidc and nobody named, every account the provider \
                 authenticates would get full access. Set {SUBJECTS} to the sub of each person, \
                 or {GROUPS} to the groups whose members may sign in, or {ANYONE}=true to let \
                 every account of the provider in"
            )));
        }
        if self.oidc_allow_anyone && readers {
            return Err(AppError::Config(format!(
                "{ANYONE}=true gives every account of the provider full access, so {READ_SUBJECTS} \
                 and {READ_GROUPS} would make nobody a reader. Unset one or the other"
            )));
        }
        for (full, read, full_list, read_list) in [
            (SUBJECTS, READ_SUBJECTS, &self.oidc_allowed_subjects, &self.oidc_read_subjects),
            (GROUPS, READ_GROUPS, &self.oidc_allowed_groups, &self.oidc_read_groups),
        ] {
            if let Some(both) = read_list.iter().find(|name| full_list.contains(name)) {
                return Err(AppError::Config(format!(
                    "{full} and {read} both name '{both}'. Name it in {full} for full access, or \
                     in {read} to read alone"
                )));
            }
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

    /// Path of the generated API key, next to the database and apart from the master key: this
    /// one is copied into a client, the other never leaves the host.
    pub fn api_key_path(&self) -> PathBuf {
        self.data_dir.join("routarr.api_key")
    }

    /// A digest of who the OIDC settings let in and how far, which an OIDC session is bound
    /// to: a restart that changes any of them ends the sessions opened before. The order a
    /// list is written in lets nobody else in, so it is left out.
    pub fn oidc_access_digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let sorted = |names: &[String]| {
            let mut names = names.to_vec();
            names.sort();
            names
        };
        let access = serde_json::json!([
            self.oidc_issuer,
            sorted(&self.oidc_allowed_subjects),
            sorted(&self.oidc_allowed_groups),
            sorted(&self.oidc_read_subjects),
            sorted(&self.oidc_read_groups),
            self.oidc_groups_claim,
            self.oidc_allow_anyone,
        ]);
        let digest = Sha256::digest(access.to_string().as_bytes());
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Config used by tests: in-memory database, no external calls.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        const UNREACHABLE_SOURCE: &str = "http://127.0.0.1:1";
        Self {
            base_path: String::new(),
            host: "127.0.0.1".into(),
            // Port 1, where nothing listens: a test pinging it never reaches a server running here.
            port: 1,
            db_path: PathBuf::from(":memory:"),
            // Not derived: from `:memory:` it is relative, and a backup would land in the sources.
            data_dir: test_data_dir(),
            backup_copy_dir: None,
            log_level: "error".into(),
            log_format: "text".into(),
            log_filter: None,
            startup_notes: Vec::new(),
            frontend_dir: PathBuf::from("/nonexistent"),
            tmdb_api_key: None,
            api_key: None,
            // Off, so every unauthenticated case is testable: a test that checks a key sets one.
            auth_mode: AuthMode::None,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_redirect_url: None,
            oidc_allowed_subjects: vec![],
            oidc_allowed_groups: vec![],
            oidc_read_subjects: vec![],
            oidc_read_groups: vec![],
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
            unattended_wait: UNATTENDED_WAIT,
            delivery_wait: DELIVERY_WAIT,
            secret_key: Some("dGVzdC1rZXktMzItYnl0ZXMtZm9yLXVuaXQtdGVzdCE=".into()),
            previous_secret_key: None,
            allow_new_master_key: false,
            metadata_concurrency: 2,
            // Port 1 on the loopback, where nothing listens: a test that probes or enriches is
            // refused at once rather than reaching the real service, unless it sets a stand-in.
            tmdb_base_url: UNREACHABLE_SOURCE.to_string(),
            omdb_api_key: None,
            omdb_base_url: UNREACHABLE_SOURCE.to_string(),
            tvdb_api_key: None,
            tvdb_pin: None,
            tvdb_base_url: UNREACHABLE_SOURCE.to_string(),
            anilist_base_url: UNREACHABLE_SOURCE.to_string(),
            tenrai_base_url: UNREACHABLE_SOURCE.to_string(),
            mal_client_id: None,
            myanimelist_base_url: UNREACHABLE_SOURCE.to_string(),
            anime_ids_url: UNREACHABLE_SOURCE.to_string(),
        }
    }
}

/// Where a variable is read: the process's environment, or a test's table.
type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn env_or(env: Lookup<'_>, key: &str, default: &str) -> String {
    env(key).filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
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

/// The names `ROUTARR_ALLOWED_HOSTS` lists: an address written as a URL names its host, and an
/// entry holding a path, which no `Host` header carries, is refused by name.
fn allowed_hosts_from(raw: Option<String>) -> AppResult<Vec<String>> {
    let Some(raw) = raw else { return Ok(Vec::new()) };
    let mut hosts = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|entry| !entry.is_empty()) {
        let host = match reqwest::Url::parse(entry) {
            Ok(url) if entry.contains("://") => url.host_str().map(str::to_string),
            _ if entry.contains('/') => None,
            _ => Some(entry.to_string()),
        };
        let Some(host) = host.map(|host| host_name(&host)).filter(|host| !host.is_empty()) else {
            return Err(AppError::Config(format!(
                "'{entry}' in ROUTARR_ALLOWED_HOSTS is no host name. Write the name alone, as \
                 nas.lan"
            )));
        };
        hosts.push(host);
    }
    Ok(hosts)
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

/// `path` with its `.` and `..` worked out on the letters alone, links left as they are, and no
/// separator at its end.
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut worked = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                worked.pop();
            }
            other => worked.push(other),
        }
    }
    worked
}

/// A comma separated variable, each entry trimmed, the empty ones dropped.
fn list(env: Lookup<'_>, key: &str) -> Vec<String> {
    non_empty(env, key)
        .map(|value| {
            value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(String::from).collect()
        })
        .unwrap_or_default()
}

/// The passphrase `routarr restore` and `routarr decrypt-backup` open an encrypted archive
/// with when the database in place does not hold it. Kept out of `Config`, which the server
/// holds for as long as it runs.
pub fn archive_passphrase() -> Option<String> {
    non_empty(&process_env, "ROUTARR_BACKUP_PASSPHRASE")
}

fn non_empty(env: Lookup<'_>, key: &str) -> Option<String> {
    env(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn env_parse<T: std::str::FromStr>(env: Lookup<'_>, key: &str, default: T) -> AppResult<T> {
    parse_setting(key, env(key), default)
}

fn env_switch(env: Lookup<'_>, key: &str, default: bool) -> AppResult<bool> {
    parse_switch(key, env(key), default)
}

/// A switch variable: `true` or `false` in any case, as a stored setting is read, or its default
/// when unset or blank.
fn parse_switch(key: &str, raw: Option<String>, default: bool) -> AppResult<bool> {
    match raw.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        None => Ok(default),
        Some(value) => crate::state::parse_switch(&value).ok_or_else(|| {
            AppError::Config(format!(
                "{key}: '{value}' is neither true nor false. Unset it for the default"
            ))
        }),
    }
}

/// The `.env` of `dir`, the working directory, once every line of it reads: a file in a folder
/// above configures nothing, and a line it cannot read stops the start, named by its variable.
/// The caller loads it, since setting a variable is the process's business.
pub fn read_dotenv(dir: &Path) -> AppResult<Option<PathBuf>> {
    let file = dir.join(".env");
    let unreadable = |e: &dyn std::fmt::Display| {
        AppError::Config(format!("{} cannot be read: {e}", file.display()))
    };
    let content = match std::fs::read_to_string(&file) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(&e)),
    };
    for line in dotenvy::from_read_iter(content.as_bytes()) {
        match line {
            Ok(_) => {}
            // The variable alone, from the line holding what failed: a value may be a key.
            Err(dotenvy::Error::LineParse(failed, _)) => {
                let failed = failed.trim();
                let line = content.lines().find(|line| line.contains(failed)).unwrap_or_default();
                let name = line.split_once('=').map_or("a variable", |(name, _)| name.trim());
                return Err(AppError::Config(format!(
                    "{}: the line of {name} cannot be read, so it and every line after it would \
                     be left out. Quote a value holding a space or a quote",
                    file.display()
                )));
            }
            Err(e) => return Err(unreadable(&e)),
        }
    }
    Ok(Some(file))
}

/// What the start says of `TZ` when it names no zone `known` holds, which chrono reads as UTC.
/// A POSIX rule (`CET-1CEST`) names no file, and is not judged.
fn time_zone_note(tz: Option<&str>, known: impl Fn(&str) -> bool) -> Option<String> {
    let zone = tz?.trim().trim_start_matches(':');
    let utc = ["UTC", "Etc/UTC", "GMT", "Etc/GMT", "Z"].contains(&zone);
    if zone.is_empty() || utc || zone.chars().any(|c| c.is_ascii_digit()) || known(zone) {
        return None;
    }
    let variable = "TZ";
    Some(format!(
        "{variable} is '{zone}', which names no time zone this system knows: the log is in UTC"
    ))
}

/// What the start says of `PUID` and `PGID`, which the linuxserver.io and hotio images read and
/// this one does not: unsaid, the owner they name is dropped without a word.
fn owner_variables_note(puid: Option<&str>, pgid: Option<&str>) -> Option<String> {
    let set = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
    let named = match (set(puid), set(pgid)) {
        (true, true) => "PUID and PGID are set, and Routarr reads neither",
        (true, false) => "PUID is set, and Routarr does not read it",
        (false, true) => "PGID is set, and Routarr does not read it",
        (false, false) => return None,
    };
    Some(format!(
        "{named}: it runs as the user it is started as. To run it as the data folder's owner, set \
         `user:` in the compose file or pass `--user` to docker run"
    ))
}

/// Whether the system's zone files hold `zone`, or `true` on a system without them, which
/// cannot tell.
fn zone_known(zone: &str) -> bool {
    let zones = Path::new("/usr/share/zoneinfo");
    !zones.is_dir() || zones.join(zone).is_file()
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

/// The data folder of this test run, named by its process. Those of finished runs go on first
/// use, since no hook runs after the last test, and a run still going keeps its own.
#[cfg(test)]
fn test_data_dir() -> PathBuf {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    let base = std::env::temp_dir();
    SWEPT.call_once(|| {
        let Ok(entries) = std::fs::read_dir(&base) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let pid = name.to_str().and_then(|name| name.strip_prefix("routarr-tests-"));
            let finished = pid.is_some_and(|pid| {
                pid.parse::<u32>().is_ok_and(|pid| {
                    pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists()
                })
            });
            if finished && Path::new("/proc/self").exists() {
                std::fs::remove_dir_all(entry.path()).ok();
            }
        }
    });
    base.join(format!("routarr-tests-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn an_owner_given_as_puid_or_pgid_is_said_to_be_ignored() {
        assert_eq!(owner_variables_note(None, Some(" ")), None);
        let note = owner_variables_note(Some("99"), None).unwrap();
        assert!(note.starts_with("PUID is set") && note.contains("--user"), "{note}");
        assert!(owner_variables_note(None, Some("100")).unwrap().starts_with("PGID is set"));
    }

    #[test]
    fn a_listed_origin_is_written_as_a_browser_sends_it() {
        let listed = origins_from(Some(
            " HTTPS://Pages.Example:443/, http://Nas.Lan:80, http://nas.lan:8080 ,, *, /path"
                .into(),
        ));
        assert_eq!(
            listed,
            ["https://pages.example", "http://nas.lan", "http://nas.lan:8080", "*", "/path"]
        );
        assert!(origins_from(None).is_empty());
    }

    #[test]
    fn rust_log_is_read_here_and_said_when_it_overrides() {
        assert_eq!(log_filter_from(None), (None, None));
        let (kept, note) = log_filter_from(Some("routarr=debug,sqlx=info".into()));
        assert_eq!(kept.as_deref(), Some("routarr=debug,sqlx=info"));
        assert!(note.is_some_and(|note| note.contains("replaces")));
        let (kept, note) = log_filter_from(Some("routarr=loud".into()));
        assert_eq!(kept, None);
        assert!(note.is_some_and(|note| note.contains("not applied")));
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

    /// `.env.example` is where people read the list of variables, so one the code reads and the
    /// sample omits is invisible. Read at run time, not `include_str!`'d: the Docker build
    /// copies only `src`, `migrations` and `locales`.
    #[test]
    fn every_environment_variable_the_code_reads_is_in_the_sample_env() {
        let root = env!("CARGO_MANIFEST_DIR");
        let source = std::fs::read_to_string(format!("{root}/src/config.rs")).unwrap();
        let sample = std::fs::read_to_string(format!("{root}/.env.example")).unwrap();

        let mut read: Vec<String> = Vec::new();
        for line in source.lines() {
            for prefix in ["ROUTARR_", "TMDB_", "OMDB_", "TVDB_", "MAL_", "RUST_"] {
                let Some(at) = line.find(&format!("\"{prefix}")) else { continue };
                let rest = &line[at + 1..];
                let Some(end) = rest.find('"') else { continue };
                read.push(rest[..end].to_string());
            }
        }
        read.sort();
        read.dedup();
        // Without this guard, a scan that matched nothing would pass.
        assert!(read.len() >= 15, "only {} variable(s) found in config.rs", read.len());

        let missing: Vec<&String> = read.iter().filter(|name| !sample.contains(*name)).collect();
        assert!(missing.is_empty(), "undocumented in .env.example: {missing:?}");
    }

    /// Scanning this file is enough: it alone reads the environment outside the tests.
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
                    && ["ROUTARR_", "TMDB_", "OMDB_", "TVDB_", "MAL_", "RUST_"]
                        .iter()
                        .any(|p| name.starts_with(p))
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

    /// Each variable holds a value of its own, so one read into another field fails here, and
    /// one the start reads that the table lacks fails the comparison of the names asked.
    #[test]
    fn every_variable_lands_in_its_field() {
        let set = std::collections::BTreeMap::from([
            ("ROUTARR_DB_PATH", " /srv/routarr/library.db "),
            ("ROUTARR_AUTH", "oidc"),
            ("ROUTARR_METADATA_CONCURRENCY", "7"),
            ("ROUTARR_LOG_LEVEL", "DEBUG"),
            ("ROUTARR_LOG_FORMAT", "json"),
            ("RUST_LOG", "routarr=trace"),
            ("ROUTARR_TRUSTED_PROXIES", "10.0.0.1, ,172.18.0.0/16"),
            ("TZ", "Mars/Olympus"),
            ("PUID", "99"),
            ("PGID", "100"),
            ("ROUTARR_HOST", "10.1.2.3"),
            ("ROUTARR_PORT", "9123"),
            ("ROUTARR_BACKUP_COPY_DIR", "/mnt/copies/./routarr/"),
            ("ROUTARR_FRONTEND_DIR", " /opt/routarr/ui "),
            ("TMDB_API_KEY", " tmdb-k1 "),
            ("ROUTARR_API_KEY", "api-k2"),
            ("ROUTARR_OIDC_ISSUER", "https://auth.example/"),
            ("ROUTARR_OIDC_CLIENT_ID", "routarr"),
            ("ROUTARR_OIDC_CLIENT_SECRET", "oidc-k3"),
            ("ROUTARR_OIDC_REDIRECT_URL", "https://routarr.example/callback"),
            ("ROUTARR_OIDC_ALLOWED_SUBJECTS", "alice, ,bob"),
            ("ROUTARR_OIDC_ALLOWED_GROUPS", "admins"),
            ("ROUTARR_OIDC_READ_SUBJECTS", "carol"),
            ("ROUTARR_OIDC_READ_GROUPS", "family, guests"),
            ("ROUTARR_OIDC_GROUPS_CLAIM", "roles"),
            ("ROUTARR_OIDC_ALLOW_ANYONE", "TRUE"),
            ("ROUTARR_CORS_ORIGINS", "HTTPS://Pages.Example:443/"),
            ("ROUTARR_ALLOWED_HOSTS", "https://NAS.lan:9876/"),
            ("ROUTARR_HTTP_TIMEOUT_SECS", "31"),
            ("ROUTARR_LIBRARY_TIMEOUT_SECS", "93"),
            ("ROUTARR_MAX_LIBRARY_MIB", "3"),
            ("ROUTARR_SECRET_KEY", "master-k4"),
            ("ROUTARR_PREVIOUS_SECRET_KEY", "former-k5"),
            ("ROUTARR_ALLOW_NEW_MASTER_KEY", "true"),
            ("ROUTARR_TMDB_BASE_URL", "http://127.0.0.1:1/tmdb/"),
            ("OMDB_API_KEY", "omdb-k6"),
            ("ROUTARR_OMDB_BASE_URL", "http://127.0.0.1:1/omdb/"),
            ("TVDB_API_KEY", "tvdb-k7"),
            ("TVDB_SUBSCRIBER_PIN", "pin-8"),
            ("ROUTARR_TVDB_BASE_URL", "http://127.0.0.1:1/tvdb/"),
            ("ROUTARR_ANILIST_BASE_URL", "http://127.0.0.1:1/anilist/"),
            ("ROUTARR_TENRAI_BASE_URL", "http://127.0.0.1:1/tenrai/"),
            ("MAL_CLIENT_ID", "mal-k9"),
            ("ROUTARR_MYANIMELIST_BASE_URL", "http://127.0.0.1:1/mal/"),
            ("ROUTARR_ANIME_IDS_URL", "http://127.0.0.1:1/ids.json"),
            ("ROUTARR_BASE_PATH", "routarr/"),
        ]);
        let asked = std::cell::RefCell::new(std::collections::BTreeSet::new());
        let lookup = |key: &str| {
            asked.borrow_mut().insert(key.to_string());
            set.get(key).map(|value| value.to_string())
        };
        let config = Config::from_lookup(&lookup).unwrap();
        let asked = asked.into_inner();
        let untested: Vec<&String> =
            asked.iter().filter(|name| !set.contains_key(name.as_str())).collect();
        let unread: Vec<&&str> = set.keys().filter(|name| !asked.contains(**name)).collect();
        assert!(untested.is_empty(), "read and missing from this table: {untested:?}");
        assert!(unread.is_empty(), "in this table and no longer read: {unread:?}");

        assert_eq!(config.db_path, PathBuf::from("/srv/routarr/library.db"));
        assert_eq!(config.data_dir, PathBuf::from("/srv/routarr"));
        assert_eq!(config.auth_mode, AuthMode::Oidc);
        assert_eq!(config.metadata_concurrency, 7);
        assert_eq!((config.log_level.as_str(), config.log_format.as_str()), ("debug", "json"));
        assert_eq!(config.log_filter.as_deref(), Some("routarr=trace"));
        let proxies = ["10.0.0.1", "172.18.0.0/16"].map(|entry| Network::parse(entry).unwrap());
        assert_eq!(config.trusted_proxies, proxies);
        let said = |start: &str| config.startup_notes.iter().any(|note| note.starts_with(start));
        assert!(said("RUST_LOG") && said("ROUTARR_TRUSTED_PROXIES"), "{:?}", config.startup_notes);
        let zones_known = Path::new("/usr/share/zoneinfo").is_dir();
        assert_eq!(said("TZ is 'Mars/Olympus'"), zones_known, "{:?}", config.startup_notes);
        assert!(said("PUID and PGID are set"), "{:?}", config.startup_notes);
        assert_eq!((config.host.as_str(), config.port), ("10.1.2.3", 9123));
        assert_eq!(config.backup_copy_dir, Some(PathBuf::from("/mnt/copies/routarr")));
        assert_eq!(config.frontend_dir, PathBuf::from("/opt/routarr/ui"));
        assert_eq!(config.tmdb_api_key.as_deref(), Some("tmdb-k1"));
        assert_eq!(config.api_key.as_deref(), Some("api-k2"));
        assert_eq!(config.oidc_issuer.as_deref(), Some("https://auth.example"));
        assert_eq!(config.oidc_client_id.as_deref(), Some("routarr"));
        assert_eq!(config.oidc_client_secret.as_deref(), Some("oidc-k3"));
        assert_eq!(config.oidc_redirect_url.as_deref(), Some("https://routarr.example/callback"));
        assert_eq!(config.oidc_allowed_subjects, ["alice", "bob"]);
        assert_eq!(config.oidc_allowed_groups, ["admins"]);
        assert_eq!(config.oidc_read_subjects, ["carol"]);
        assert_eq!(config.oidc_read_groups, ["family", "guests"]);
        assert_eq!(config.oidc_groups_claim, "roles");
        assert!(config.oidc_allow_anyone);
        assert_eq!(config.cors_origins, ["https://pages.example"]);
        assert_eq!(config.allowed_hosts, ["nas.lan"]);
        assert_eq!(config.http_timeout, Duration::from_secs(31));
        assert_eq!(config.library_timeout, Duration::from_secs(93));
        assert_eq!(config.max_library_bytes, 3 * 1024 * 1024);
        assert_eq!(config.secret_key.as_deref(), Some("master-k4"));
        assert_eq!(config.previous_secret_key.as_deref(), Some("former-k5"));
        assert!(config.allow_new_master_key);
        assert_eq!(config.tmdb_base_url, "http://127.0.0.1:1/tmdb");
        assert_eq!(config.omdb_api_key.as_deref(), Some("omdb-k6"));
        assert_eq!(config.omdb_base_url, "http://127.0.0.1:1/omdb");
        assert_eq!(config.tvdb_api_key.as_deref(), Some("tvdb-k7"));
        assert_eq!(config.tvdb_pin.as_deref(), Some("pin-8"));
        assert_eq!(config.tvdb_base_url, "http://127.0.0.1:1/tvdb");
        assert_eq!(config.anilist_base_url, "http://127.0.0.1:1/anilist");
        assert_eq!(config.tenrai_base_url, "http://127.0.0.1:1/tenrai");
        assert_eq!(config.mal_client_id.as_deref(), Some("mal-k9"));
        assert_eq!(config.myanimelist_base_url, "http://127.0.0.1:1/mal");
        assert_eq!(config.anime_ids_url, "http://127.0.0.1:1/ids.json");
        assert_eq!(config.base_path, "/routarr");

        let refused = |key: &str| {
            (key == "ROUTARR_TRUSTED_PROXIES").then(|| "10.0.0.1, the-proxy".to_string())
        };
        let err = Config::from_lookup(&refused).unwrap_err().to_string();
        assert!(err.contains("'the-proxy' in ROUTARR_TRUSTED_PROXIES"), "{err}");
    }

    #[test]
    fn an_in_memory_database_never_derives_a_relative_data_directory() {
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
        assert_eq!(data_dir_for(Path::new("routarr.db")), PathBuf::from("."));
    }

    /// Left behind, the master key would be generated anew at the next start, opening none of the
    /// stored credentials.
    #[test]
    fn moving_the_database_takes_the_keys_and_the_backups_with_it() {
        let mut config = Config::for_tests();
        config.set_db_path(PathBuf::from("/srv/routarr/data/routarr.db"));

        assert_eq!(config.data_dir, PathBuf::from("/srv/routarr/data"));
        assert_eq!(config.secret_key_path(), PathBuf::from("/srv/routarr/data/routarr.key"));
        assert_eq!(config.api_key_path(), PathBuf::from("/srv/routarr/data/routarr.api_key"));
    }

    #[test]
    fn a_wildcard_origin_is_refused_by_name() {
        let err = validate_origin("*").expect_err("a wildcard has to be refused");
        let message = err.to_string();
        assert!(message.contains("ROUTARR_CORS_ORIGINS"), "the message names the variable");
        assert!(message.contains("wildcard"), "and says what is wrong: {message}");
    }

    #[test]
    fn an_origin_that_could_never_match_is_refused() {
        for bad in ["example.com", "//example.com", "ftp://example.com", "https://", "not a url"] {
            assert!(validate_origin(bad).is_err(), "'{bad}' was accepted");
        }
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
            "mal-client-9",
        ];
        config.tmdb_api_key = Some(secrets[0].into());
        config.api_key = Some(secrets[1].into());
        config.oidc_client_secret = Some(secrets[2].into());
        config.secret_key = Some(secrets[3].into());
        config.previous_secret_key = Some(secrets[4].into());
        config.omdb_api_key = Some(secrets[5].into());
        config.tvdb_api_key = Some(secrets[6].into());
        config.tvdb_pin = Some(secrets[7].into());
        config.mal_client_id = Some(secrets[8].into());

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

    #[test]
    fn a_backup_copy_folder_is_whole_and_apart_from_the_data() {
        let mut config = Config::for_tests();
        config.set_db_path(PathBuf::from("/config/routarr.db"));
        for refused in ["copies", "/config/backups-copy", "/config/../config/x", "/", "/config"] {
            config.backup_copy_dir = Some(PathBuf::from(refused));
            let err = config.validate().expect_err(refused).to_string();
            assert!(err.contains("ROUTARR_BACKUP_COPY_DIR"), "{refused}: {err}");
        }
        for served in ["/mnt/nas/routarr", "/config-copy", "/config/../mnt/nas"] {
            config.backup_copy_dir = Some(PathBuf::from(served));
            assert!(config.validate().is_ok(), "{served}");
        }
    }

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

    /// A range trusts every peer inside it, each container of a shared Docker network among
    /// them, to say which address a sign-in comes from: said at start, as an open mode is.
    #[test]
    fn a_trusted_range_is_said_at_start_and_one_address_is_not() {
        let notes = |entries: &[&str]| {
            proxy_range_notes(
                &entries.iter().map(|e| Network::parse(e).unwrap()).collect::<Vec<_>>(),
            )
        };
        let said = notes(&["172.18.0.2", "172.18.0.0/16"]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("172.18.0.0/16") && said[0].contains("ROUTARR_TRUSTED_PROXIES"));
        assert!(notes(&["172.18.0.2", "fd00::2/128"]).is_empty());
    }

    /// A switch is read as the stored settings read one, and a refusal names what it takes.
    #[test]
    fn a_switch_is_read_in_any_case_and_refused_by_name_otherwise() {
        const KEY: &str = "ROUTARR_ALLOW_NEW_MASTER_KEY";
        for (written, read) in [("TRUE", true), (" true ", true), ("False", false), ("", true)] {
            assert_eq!(parse_switch(KEY, Some(written.into()), true).unwrap(), read, "{written}");
        }
        assert!(!parse_switch(KEY, None, false).unwrap());
        for refused in ["maybe", "1", "yes"] {
            let err = parse_switch(KEY, Some(refused.into()), false).unwrap_err().to_string();
            assert!(err.contains(KEY) && err.contains("true") && err.contains("false"), "{err}");
        }
    }

    /// A `.env` is configuration like the environment: read from the working directory alone,
    /// and a line it cannot read stops the start naming the file and the variable, never the
    /// value, which may be a key.
    #[test]
    fn a_dotenv_line_that_cannot_be_read_stops_the_start_by_name() {
        let dir = crate::tests::TempDir::new("dotenv-line");
        let file = dir.join(".env");
        std::fs::write(&file, "A_PORT=19874\nA_KEY=a key pasted with spaces\n").unwrap();
        let err = read_dotenv(&dir).unwrap_err().to_string();
        assert!(err.contains(".env") && err.contains("A_KEY"), "{err}");
        assert!(!err.contains("pasted"), "the value was quoted: {err}");

        std::fs::write(&file, "A_PORT=19874\nA_KEY='a key with spaces'\n").unwrap();
        assert_eq!(read_dotenv(&dir).unwrap(), Some(file), "the control");
    }

    #[test]
    fn a_dotenv_in_a_parent_folder_is_not_read() {
        let dir = crate::tests::TempDir::new("dotenv-parent");
        std::fs::write(dir.join(".env"), "A_PORT=19874\n").unwrap();
        let below = dir.join("below");
        std::fs::create_dir(&below).unwrap();
        assert_eq!(read_dotenv(&below).unwrap(), None);
        assert_eq!(read_dotenv(&dir).unwrap(), Some(dir.join(".env")), "the control");
    }

    /// chrono reads a `TZ` naming no zone as UTC, unsaid, and every log line is off by its offset.
    #[test]
    fn a_time_zone_no_system_knows_is_said() {
        let known = |zone: &str| zone == "Europe/Paris";
        let note = time_zone_note(Some("Europe/Pari"), known).expect("a note");
        assert!(
            note.contains("TZ") && note.contains("Europe/Pari") && note.contains("UTC"),
            "{note}"
        );
        for quiet in [None, Some(""), Some("Europe/Paris"), Some(":Europe/Paris"), Some("UTC")] {
            assert_eq!(time_zone_note(quiet, known), None, "{quiet:?}");
        }
        assert_eq!(time_zone_note(Some("CET-1CEST"), known), None, "a POSIX rule names no file");
    }

    /// A URL in `ROUTARR_ALLOWED_HOSTS` names its host, which an operator means, and a path no
    /// host has is refused by name.
    #[test]
    fn an_allowed_host_written_as_a_url_is_read_or_refused() {
        let read =
            allowed_hosts_from(Some("http://NAS.lan:9876/, media.example, [::1]:9876".into()));
        assert_eq!(read.unwrap(), ["nas.lan", "media.example", "::1"]);
        let err = allowed_hosts_from(Some("nas.lan/routarr".into())).unwrap_err().to_string();
        assert!(err.contains("ROUTARR_ALLOWED_HOSTS") && err.contains("nas.lan/routarr"), "{err}");
        assert!(allowed_hosts_from(None).unwrap().is_empty());
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
        // Any free port, which no client and no health check can find.
        let err = refused(|config| config.port = 0);
        assert!(err.contains("ROUTARR_PORT"), "{err}");
    }

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

    /// Refused at start: readers with nobody to run Routarr, readers anyone's full access would
    /// swallow, and a name in both lists, which full access would win.
    #[test]
    fn oidc_read_lists_that_cannot_hold_refuse_to_start() {
        let mut config = oidc_config();
        config.oidc_read_groups = vec!["family".into()];
        assert!(config.validate().is_ok());

        config.oidc_allowed_subjects.clear();
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ROUTARR_OIDC_ALLOWED_SUBJECTS"), "{err}");
        assert!(!err.contains("every account"), "{err}");

        config.oidc_allow_anyone = true;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ROUTARR_OIDC_ALLOW_ANYONE"), "{err}");

        let mut config = oidc_config();
        config.oidc_read_subjects = vec!["u-42".into()];
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("'u-42'"), "{err}");
        let mut config = oidc_config();
        config.oidc_allowed_groups = vec!["media".into()];
        config.oidc_read_groups = vec!["media".into()];
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ROUTARR_OIDC_READ_GROUPS") && err.contains("'media'"), "{err}");
    }

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
