//! Shared application state handed to every route handler and background job.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::sync::Arc;
use std::time::Instant;

/// Where routing lands when nothing matched and no setting says otherwise.
pub const DEFAULT_CATEGORY: &str = "standard";

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::error::{AppError, AppResult};
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::anilist::AniListClient;
use crate::integrations::jikan::JikanClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tmdb::TmdbClient;
use crate::integrations::tvdb::TvdbClient;
use crate::jobs::JobRegistry;
use crate::localization::{DEFAULT_LANGUAGE, Localizer};
use crate::models::Instance;
use crate::services::metadata::{self, FetchingSource, ProviderInfo};

/// The same resolution `main` performs: the environment, else what is stored.
///
/// Written once and used by both, so a harness cannot drift into testing a
/// precedence the application does not have.
#[cfg(test)]
fn resolve_api_key(config: &Config) -> Option<String> {
    config.api_key.clone().or_else(|| crate::crypto::read_api_key(&config.api_key_path()))
}

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// Shared outbound HTTP client (connection pool + timeouts).
    pub http: reqwest::Client,
    pub secrets: SecretBox,
    pub jobs: JobRegistry,
    /// TheTVDB's bearer token, shared across every client this state builds.
    ///
    /// It is valid for about a month, but the clients are rebuilt on every call
    /// that needs a source — a health page, an enrichment pass — so holding the
    /// token inside the client meant logging in again each time. TheTVDB counts
    /// logins; this is the only source that has any.
    pub tvdb_token: Arc<tokio::sync::Mutex<Option<String>>>,
    /// The API key as it stands right now.
    ///
    /// Held here rather than on [`Config`] because it can change while the
    /// process runs: a rotation has to take effect on the next request, not on
    /// the next restart, or a leaked key stays valid until somebody stops the
    /// service. Read on every protected request, so it is a lock and not a
    /// query — the value is resolved once at startup and written only by a
    /// rotation.
    pub api_key: Arc<std::sync::RwLock<Option<String>>>,
    /// What `/auth/login` may spend at once. See
    /// [`crate::services::accounts::SignInThrottle`]: a permit rather than a
    /// lockout, so the cost is bounded without the service being deniable.
    pub sign_in: Arc<crate::services::accounts::SignInThrottle>,
    /// The provider's own description of itself, and when it was read.
    ///
    /// Here for the reason `tvdb_token` is: `/auth/oidc/start` is public and
    /// unauthenticated, and fetching the discovery document on every call turns
    /// one cheap inbound request into one outbound request against the
    /// operator's identity provider — which rate-limits by address and would
    /// lock them out of their own login. It is a static document; providers
    /// expect it to be cached.
    pub oidc_provider: Arc<tokio::sync::RwLock<Option<(crate::services::oidc::Provider, Instant)>>>,
}

/// The settings table as it stood when it was read — see [`AppState::settings`].
///
/// Parsed the way the single-key readers parse, so an answer is the same
/// whichever of the two a caller asked.
pub struct Settings(std::collections::HashMap<String, String>);

impl Settings {
    /// The stored text, or `None` when the key has no row.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// `default` when absent or unparseable, as `AppState::setting` answers.
    pub fn get<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        self.raw(key).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
    }

    /// `true` or `1`, as `AppState::bool_setting` answers.
    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.raw(key).map_or(default, |v| v.eq_ignore_ascii_case("true") || v == "1")
    }
}

impl AppState {
    /// Point this state at a config, keeping the live key in step with it.
    ///
    /// A test that swaps the config to change the auth mode expects the key it
    /// wrote there to be the one the middleware checks. Setting the two apart
    /// is how a suite ends up asserting against a credential nothing reads.
    #[cfg(test)]
    pub fn with_config(mut self, config: Config) -> Self {
        self.api_key = Arc::new(std::sync::RwLock::new(resolve_api_key(&config)));
        self.config = Arc::new(config);
        self
    }

    /// The key a request must present, or `None` when there is none to present.
    ///
    /// A poisoned lock is recovered rather than propagated, the same choice
    /// `JobRegistry::try_lock` makes and for a stronger reason: this is read on
    /// every protected request, so panicking here would turn one panic under
    /// the guard into a server that answers nothing at all. The guarded value
    /// is one `Option<String>` with no invariant a panic could leave half
    /// written.
    pub fn api_key(&self) -> Option<String> {
        self.api_key.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Replace the key, in memory and on disk, and return what to show once.
    ///
    /// The file is rewritten first: a key live in memory but absent from disk
    /// would work until the next restart and then lock everybody out, which is
    /// the one failure worse than not rotating at all.
    pub fn rotate_api_key(&self) -> AppResult<String> {
        let key = crate::crypto::generate_secret()?;
        crate::crypto::write_api_key(&self.config.api_key_path(), &key)?;
        *self.api_key.write().unwrap_or_else(|p| p.into_inner()) = Some(key.clone());
        Ok(key)
    }

    /// Withdraw the key entirely, leaving the session as the only way in.
    pub fn clear_api_key(&self) -> AppResult<()> {
        crate::crypto::remove_api_key(&self.config.api_key_path())?;
        *self.api_key.write().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    /// Build an Arr client for a stored instance, decrypting its API key.
    pub fn adapter(&self, instance: &Instance) -> AppResult<ArrAdapter> {
        let api_key = self.secrets.open(&instance.api_key)?;
        ArrAdapter::for_instance(self.http.clone(), instance, &api_key)
    }

    /// Every setting as stored, read in one statement.
    ///
    /// A screen that asks about several — `/status` asks about the language,
    /// the dry-run switch, the metadata order, three credentials and the
    /// retention counts — reads the table once and answers from this rather
    /// than once per key. An error reads as an empty table, the posture every
    /// single-key reader takes.
    pub async fn settings(&self) -> Settings {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();
        Settings(rows.into_iter().collect())
    }

    /// The credential for a metadata source: what the interface saved, else the
    /// environment variable.
    ///
    /// The saved value is sealed with the master key, exactly as an Arr's is —
    /// an Arr key writes to the library while a metadata key only reads, so the
    /// more dangerous of the two already goes through this machinery.
    ///
    /// The environment stays the fallback rather than being replaced: an
    /// existing deployment must keep working untouched, and a key handed in by
    /// an orchestrator is a legitimate way to run.
    #[cfg(test)]
    pub async fn provider_key(&self, provider: &str) -> Option<String> {
        self.provider_key_from(&self.settings().await, provider)
    }

    /// `provider_key`, answered from a snapshot already read.
    pub fn provider_key_from(&self, settings: &Settings, provider: &str) -> Option<String> {
        let stored = settings.raw(&format!("{provider}_api_key"));
        if let Some(raw) = stored.filter(|v| !v.trim().is_empty()) {
            // A value that opens with neither key is left alone and reported,
            // never treated as absent: silently falling back would route on
            // metadata the operator believes they configured.
            return match self.secrets.open(raw) {
                Ok(key) => Some(key),
                Err(e) => {
                    tracing::warn!("The stored {provider} key could not be opened: {e}");
                    None
                }
            };
        }
        match provider {
            metadata::TMDB => self.config.tmdb_api_key.clone(),
            metadata::OMDB => self.config.omdb_api_key.clone(),
            metadata::TVDB => self.config.tvdb_api_key.clone(),
            _ => None,
        }
    }

    /// `provider_keys`, answered from a snapshot already read.
    pub fn provider_keys_from(
        &self,
        settings: &Settings,
    ) -> std::collections::HashMap<&'static str, String> {
        let mut keys = std::collections::HashMap::new();
        for id in [metadata::TMDB, metadata::OMDB, metadata::TVDB] {
            if let Some(key) = self.provider_key_from(settings, id) {
                keys.insert(id, key);
            }
        }
        keys
    }

    /// `tmdb`, built from a snapshot already read.
    pub fn tmdb_from(&self, settings: &Settings) -> Option<TmdbClient> {
        let key = self.provider_key_from(settings, metadata::TMDB)?;
        let regions = Self::certification_regions_from(settings);
        Some(TmdbClient::new(self.http.clone(), &key, &self.config.tmdb_base_url, &regions))
    }

    /// Metadata sources as the user ordered them, highest priority first.
    ///
    /// An absent setting is the shipped default; a setting set to the empty
    /// string means the user disabled every source, which is a legitimate
    /// choice for a library routed on paths and titles alone.
    pub async fn metadata_order(&self) -> Vec<&'static ProviderInfo> {
        Self::metadata_order_from(&self.settings().await)
    }

    /// `metadata_order`, answered from a snapshot already read.
    pub fn metadata_order_from(settings: &Settings) -> Vec<&'static ProviderInfo> {
        match settings.raw("metadata_providers") {
            Some(value) => metadata::parse_order(value),
            None => metadata::parse_order(&metadata::DEFAULT_ORDER.join(",")),
        }
    }

    /// The same list minus the sources that cannot answer today — one that
    /// needs a key and has none. What a rule can actually rely on.
    pub async fn metadata_providers(&self) -> Vec<&'static ProviderInfo> {
        self.metadata_providers_from(&self.settings().await)
    }

    /// `metadata_providers`, answered from a snapshot already read: the order
    /// and the keys it is filtered by come from the same read, so a save that
    /// lands between two reads cannot enable a source and hide its key.
    pub fn metadata_providers_from(&self, settings: &Settings) -> Vec<&'static ProviderInfo> {
        let keys = self.provider_keys_from(settings);
        Self::metadata_order_from(settings)
            .into_iter()
            .filter(|p| metadata::is_usable(p, &keys))
            .collect()
    }

    /// The sources enrichment has to go and ask, in the same order.
    ///
    /// Building the client is also the last check that a source is usable: a
    /// key that disappeared between the setting and here simply yields no
    /// client, and the sources below it answer instead.
    pub async fn metadata_sources(&self) -> Vec<FetchingSource> {
        // One read for the order, the keys and the regions: five reads had
        // five chances to see a save land between them.
        let settings = self.settings().await;
        let regions = Self::certification_regions_from(&settings);
        let mut sources = Vec::new();

        for provider in self.metadata_providers_from(&settings) {
            if !provider.fetched {
                continue;
            }

            let source = match provider.id {
                metadata::TMDB => self.tmdb_from(&settings).map(FetchingSource::Tmdb),
                metadata::ANILIST => Some(FetchingSource::AniList(AniListClient::new(
                    self.http.clone(),
                    &self.config.anilist_base_url,
                ))),
                metadata::JIKAN => Some(FetchingSource::Jikan(JikanClient::new(
                    self.http.clone(),
                    &self.config.jikan_base_url,
                ))),
                metadata::OMDB => self.provider_key_from(&settings, metadata::OMDB).map(|key| {
                    FetchingSource::Omdb(OmdbClient::new(
                        self.http.clone(),
                        &key,
                        &self.config.omdb_base_url,
                    ))
                }),
                metadata::TVDB => self.provider_key_from(&settings, metadata::TVDB).map(|key| {
                    FetchingSource::Tvdb(TvdbClient::new(
                        self.http.clone(),
                        &key,
                        self.config.tvdb_pin.as_deref(),
                        &self.config.tvdb_base_url,
                        &regions,
                        Arc::clone(&self.tvdb_token),
                    ))
                }),
                other => {
                    tracing::warn!("No client for metadata source '{other}'");
                    None
                }
            };

            sources.extend(source);
        }

        sources
    }

    /// The configured UI language, or English when unset.
    pub async fn language(&self) -> String {
        Self::language_from(&self.settings().await)
    }

    /// `language`, answered from a snapshot already read.
    pub fn language_from(settings: &Settings) -> String {
        settings
            .raw("ui_language")
            .filter(|code| crate::localization::is_supported(code))
            .map_or_else(|| DEFAULT_LANGUAGE.to_string(), str::to_string)
    }

    /// Translator for the configured language.
    ///
    /// The justifications Routarr writes are user-facing prose, so they follow
    /// the same language setting as the interface.
    pub async fn localizer(&self) -> Localizer {
        Localizer::new(&self.language().await)
    }

    /// `certification_regions`, answered from a snapshot already read.
    pub fn certification_regions_from(settings: &Settings) -> Vec<String> {
        let raw = settings.raw("certification_regions");

        raw.map(|v| {
            v.split(',')
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| vec!["US".to_string()])
    }

    /// Read a setting, falling back to `default` when absent or unparseable.
    pub async fn setting<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }

    /// The category the engine falls back to when no rule matched.
    ///
    /// Takes a pool rather than `&self` because `routing.rs` holds one and not
    /// an `AppState`. Spelled at its call sites instead, it acquires a fallback
    /// per site — `"standard"` in the engine, the empty string in the category
    /// screen — and with no setting row the delete guard then compares a name
    /// against `""`, never fires, and leaves the category routing actually
    /// lands in deletable.
    ///
    /// One spelling, one fallback. The default lives in `DEFAULT_CATEGORY`.
    pub async fn default_category(pool: &sqlx::SqlitePool) -> String {
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'default_category'")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_CATEGORY.to_string())
    }

    /// Read a boolean setting stored as `"true"`/`"false"`.
    pub async fn bool_setting(&self, key: &str, default: bool) -> bool {
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(default)
    }

    /// Load an instance by id, or 404.
    pub async fn instance(&self, id: &str) -> AppResult<Instance> {
        sqlx::query_as::<_, Instance>(AssertSqlSafe(format!(
            "SELECT {} FROM instances WHERE id = ?",
            crate::models::INSTANCE_COLUMNS
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Instance {id} not found")))
    }

    /// Load every instance, optionally only the enabled ones.
    pub async fn instances(&self, only_enabled: bool) -> AppResult<Vec<Instance>> {
        let sql = format!(
            "SELECT {} FROM instances {} ORDER BY name",
            crate::models::INSTANCE_COLUMNS,
            if only_enabled { "WHERE enabled = 1" } else { "" }
        );
        Ok(sqlx::query_as::<_, Instance>(AssertSqlSafe(sql.as_str())).fetch_all(&self.pool).await?)
    }

    /// Test-only state backed by an in-memory database.
    #[cfg(test)]
    pub async fn for_tests() -> Self {
        let pool = crate::db::test_pool().await;
        let config = Config::for_tests();
        Self {
            http: crate::http::build_client(&config).expect("test http client"),
            secrets: SecretBox::load(
                config.secret_key.as_deref(),
                None,
                std::path::Path::new("/nonexistent"),
            )
            .unwrap(),
            tvdb_token: Arc::new(tokio::sync::Mutex::new(None)),
            jobs: JobRegistry::new(pool.clone()),
            api_key: Arc::new(std::sync::RwLock::new(resolve_api_key(&config))),
            sign_in: Arc::new(Default::default()),
            oidc_provider: Arc::new(tokio::sync::RwLock::new(None)),
            config: Arc::new(config),
            pool,
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::Settings;

    fn stored(pairs: &[(&str, &str)]) -> Settings {
        Settings(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    /// The snapshot answers exactly as the single-key readers do: trimmed,
    /// the default on anything unparseable or absent, `true` or `1` for a
    /// switch.
    #[test]
    fn a_snapshot_parses_the_way_the_single_key_readers_do() {
        let settings = stored(&[
            ("batch_limit", " 12 "),
            ("confirmation_threshold", "many"),
            ("global_dry_run", "TRUE"),
            ("auto_apply_enabled", "1"),
            ("refresh_after_move", "yes"),
        ]);
        assert_eq!(settings.get("batch_limit", 50i64), 12);
        assert_eq!(settings.get("confirmation_threshold", 10i64), 10, "unparseable is the default");
        assert_eq!(settings.get("absent", 7i64), 7);
        assert!(settings.bool("global_dry_run", false));
        assert!(settings.bool("auto_apply_enabled", false));
        assert!(!settings.bool("refresh_after_move", true), "`yes` is not a switch");
        assert!(settings.bool("absent", true));
        assert_eq!(settings.raw("batch_limit"), Some(" 12 "), "raw is as stored");
    }

    /// The usable sources are the snapshot's order filtered by the snapshot's
    /// keys — one read, so a source cannot be enabled by one read and keyless
    /// by another.
    #[tokio::test]
    async fn usable_providers_come_from_one_snapshot() {
        let state = crate::state::AppState::for_tests().await;
        let sealed = state.secrets.seal("tmdb-key").unwrap();
        let settings =
            stored(&[("metadata_providers", "arr,tmdb,omdb"), ("tmdb_api_key", &sealed)]);

        let usable: Vec<&str> =
            state.metadata_providers_from(&settings).iter().map(|p| p.id).collect();
        assert_eq!(usable, ["arr", "tmdb"], "omdb needs a key the snapshot does not hold");
        assert!(state.tmdb_from(&settings).is_some(), "the sealed key opens");
    }
}
