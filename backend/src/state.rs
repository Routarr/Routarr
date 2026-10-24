//! Shared application state handed to every route handler and background job.

use sqlx::{AssertSqlSafe, SqlitePool};
use std::sync::Arc;
use std::time::Instant;

/// Where routing lands when nothing matched and no setting says otherwise.
pub const DEFAULT_CATEGORY: &str = "standard";

use crate::config::Config;
use crate::crypto::SecretBox;
use crate::error::{AppError, AppResult};
use crate::jobs::JobRegistry;
use crate::localization::{DEFAULT_LANGUAGE, Localizer};
use crate::models::Instance;
use crate::services::backup::Credential;

// Every client is built here, from the shared `http` and a key `secrets` opens, so none opens
// a key twice or misses the redirect policy that keeps it from leaking.
use crate::integrations::adapter::ArrAdapter;
use crate::integrations::anilist::AniListClient;
use crate::integrations::myanimelist::MyAnimeListClient;
use crate::integrations::omdb::OmdbClient;
use crate::integrations::tenrai::TenraiClient;
use crate::integrations::tmdb::TmdbClient;
use crate::integrations::tvdb::TvdbClient;
use crate::services::metadata::{self, FetchingSource, ProviderInfo};

/// The API key: the environment's, else the stored one. Shared by `main` and the test harness,
/// so no test pins a precedence the application lacks. `main` generates a key first when
/// `apikey` mode finds neither.
pub(crate) fn resolve_api_key(config: &Config) -> Option<String> {
    config.api_key.clone().or_else(|| crate::crypto::read_api_key(&config.api_key_path()))
}

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<Config>,
    /// The outbound client every call clones: it carries the connection pool, the
    /// timeout and the same-origin redirect policy `http::build_client` sets.
    pub http: reqwest::Client,
    pub secrets: SecretBox,
    pub jobs: JobRegistry,
    /// TheTVDB's bearer token, shared by every client this state builds. The clients are rebuilt
    /// on every call that needs a source, so a token held in one would log in each time, and
    /// TheTVDB counts logins.
    pub tvdb_token: crate::integrations::tvdb::TokenCache,
    pub paces: crate::services::metadata::Paces,
    pub library_passes: crate::services::routing::LibraryPasses,
    /// The live API key, held here so a rotation takes effect on the next request. A lock rather
    /// than a query, since every protected request reads it.
    pub api_key: Arc<std::sync::RwLock<Option<String>>>,
    /// What `/auth/login` may spend at once ([`crate::services::accounts::SignInThrottle`]).
    pub sign_in: Arc<crate::services::accounts::SignInThrottle>,
    /// The provider's discovery document, and until when it is kept. `/auth/oidc/start` is public:
    /// fetched on every call, the document turns each anonymous request into one against the
    /// identity provider, which rate-limits by address and would lock the operator out.
    pub oidc_provider: Arc<tokio::sync::RwLock<Option<(crate::services::oidc::Provider, Instant)>>>,
    /// The work that follows a full sync (enrichment, simulation, auto-apply),
    /// whoever synced: one at a time, and waited for at shutdown
    /// (`jobs::scheduler::follow_sync`).
    pub post_sync: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Per instance id, the moves the last unattended pass held back for being more than one
    /// run may make.
    pub auto_apply_held: Arc<
        std::sync::Mutex<std::collections::BTreeMap<String, crate::services::auto_apply::OverCap>>,
    >,
    /// The notifications waiting to be sent, in order (`services::notify`).
    pub notifications: Arc<crate::services::notify::Queue>,
    /// The security log, which sums the refusals a caller can send at will.
    pub audit: Arc<crate::services::audit::Log>,
    /// The calls to deprecated paths, counted off the request.
    pub deprecated_calls: Arc<crate::api::deprecated::Calls>,
    /// What each application key has left of its rate.
    pub key_rates: Arc<crate::services::applications::Rates>,
    /// The titles an Arr did not know when a placement asked, for a while.
    pub route_misses: Arc<crate::services::placement::Misses>,
    /// The last probe `/health` made, which a second caller waits for and a
    /// caller soon after reads again.
    pub last_probe: Arc<tokio::sync::Mutex<Option<Arc<crate::api::health::Probe>>>>,
    /// The bytes the copies into the backup copy folder have written, which tell a share that
    /// answers slowly from one that hangs, to the copy and to the one waiting behind it.
    pub backup_copy_written: Arc<std::sync::atomic::AtomicU64>,
}

/// The settings table as [`AppState::settings`] read it, parsed as the single-key readers parse,
/// so an answer is the same whichever of the two a caller asks.
pub struct Settings(std::collections::HashMap<String, String>);

/// Over every row in key order: a `HashMap` iterates in no stable one.
impl std::hash::Hash for Settings {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort();
        rows.hash(state);
    }
}

impl Settings {
    /// The stored text, or `None` when the key has no row.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// The setting's default when absent or unparseable, as `AppState::setting` answers.
    pub fn get<T: std::str::FromStr + Default>(&self, key: &str) -> T {
        self.raw(key).and_then(|v| v.trim().parse().ok()).unwrap_or_else(|| default_of(key))
    }

    /// `true` or `1`, as `AppState::bool_setting` answers.
    pub fn bool(&self, key: &str) -> bool {
        is_true(self.raw(key).unwrap_or(crate::services::settings::default_of(key)))
    }
}

/// The default `services::settings::KNOWN` gives `key`, read as `T`: the one
/// answer every reader of a setting falls back to.
fn default_of<T: std::str::FromStr + Default>(key: &str) -> T {
    crate::services::settings::default_of(key).parse().unwrap_or_default()
}

/// How a boolean setting is stored: `true` in any case, or `1`.
fn is_true(value: &str) -> bool {
    value.eq_ignore_ascii_case("true") || value == "1"
}

impl AppState {
    /// Point this state at a config, keeping the live key in step with it: a test that swaps the
    /// config expects the key it wrote there to be the one the middleware checks.
    #[cfg(test)]
    pub fn with_config(mut self, config: Config) -> Self {
        self.api_key = Arc::new(std::sync::RwLock::new(resolve_api_key(&config)));
        self.config = Arc::new(config);
        self
    }

    /// The key a request must present, if any. A poisoned lock is recovered: read on every
    /// protected request, a panic here would leave a server that answers nothing, and one
    /// `Option<String>` has no invariant a panic could leave half written.
    pub fn api_key(&self) -> Option<String> {
        self.api_key.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Replace the key, on disk first, then in memory, and return what to show once: a key live
    /// only in memory would lock everybody out at the next restart.
    pub async fn rotate_api_key(&self) -> AppResult<String> {
        let key = crate::crypto::generate_secret()?;
        crate::crypto::write_api_key(&self.config.api_key_path(), &key)?;
        *self.api_key.write().unwrap_or_else(|p| p.into_inner()) = Some(key.clone());
        let replaced = Credential::MasterApiKey;
        crate::services::backup::set_withdrawn(&self.pool, replaced, false).await?;
        Ok(key)
    }

    /// Withdraw the key entirely, leaving the session as the only way in. The
    /// withdrawal is recorded first: a restore reads it, or brings the
    /// archive's key back as it would to a host that never held one.
    pub async fn clear_api_key(&self) -> AppResult<()> {
        crate::services::backup::set_withdrawn(&self.pool, Credential::MasterApiKey, true).await?;
        crate::crypto::remove_api_key(&self.config.api_key_path())?;
        *self.api_key.write().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    /// Build an Arr client for a stored instance, decrypting its API key.
    pub fn adapter(&self, instance: &Instance) -> AppResult<ArrAdapter> {
        let api_key = self.secrets.open(&instance.api_key)?;
        Ok(ArrAdapter::for_instance(self.http.clone(), instance, &api_key)?
            .with_library_limits(self.config.library_timeout, self.config.max_library_bytes))
    }

    /// Every setting as stored, in one statement, for a reader asking about several as `/status`
    /// does. A failed read is an empty table, as [`AppState::setting`] takes one.
    pub async fn settings(&self) -> Settings {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();
        Settings(rows.into_iter().collect())
    }

    /// `provider_key_from` over a fresh read of the settings.
    #[cfg(test)]
    pub async fn provider_key(&self, provider: &str) -> Option<String> {
        self.provider_key_from(&self.settings().await, provider)
    }

    /// A metadata source's credential in a snapshot: what the interface saved, sealed as an
    /// Arr's key is, else the environment's, kept for a key an orchestrator hands in.
    pub fn provider_key_from(&self, settings: &Settings, provider: &str) -> Option<String> {
        let setting = format!("{provider}_api_key");
        self.credential_from(settings, &setting, self.environment_key(provider))
    }

    /// TheTVDB's subscriber PIN, resolved as its key is: what the interface
    /// saved, else `TVDB_SUBSCRIBER_PIN`.
    pub fn tvdb_pin_from(&self, settings: &Settings) -> Option<String> {
        self.credential_from(settings, "tvdb_subscriber_pin", self.config.tvdb_pin.as_ref())
    }

    /// A sealed setting opened, else the environment's value.
    fn credential_from(
        &self,
        settings: &Settings,
        setting: &str,
        environment: Option<&String>,
    ) -> Option<String> {
        let stored = settings.raw(setting);
        if let Some(raw) = stored.filter(|v| !v.trim().is_empty()) {
            // A value no key opens is reported and yields nothing, never the environment's: the
            // operator would believe the key they saved answers.
            return match self.secrets.open(raw) {
                Ok(value) => Some(value),
                Err(e) => {
                    tracing::warn!("The stored {setting} could not be opened: {e}");
                    None
                }
            };
        }
        environment.cloned()
    }

    /// The key a source takes from the environment, as its `key_env` in the catalogue names it.
    pub fn environment_key(&self, provider: &str) -> Option<&String> {
        match provider {
            metadata::TMDB => self.config.tmdb_api_key.as_ref(),
            metadata::OMDB => self.config.omdb_api_key.as_ref(),
            metadata::TVDB => self.config.tvdb_api_key.as_ref(),
            metadata::MYANIMELIST => self.config.mal_client_id.as_ref(),
            _ => None,
        }
    }

    /// Every metadata source's key, stored or from the environment, read from a
    /// settings snapshot.
    pub fn provider_keys_from(
        &self,
        settings: &Settings,
    ) -> std::collections::HashMap<&'static str, String> {
        let mut keys = std::collections::HashMap::new();
        for id in [metadata::TMDB, metadata::OMDB, metadata::TVDB, metadata::MYANIMELIST] {
            if let Some(key) = self.provider_key_from(settings, id) {
                keys.insert(id, key);
            }
        }
        keys
    }

    /// A TMDB client, when a key is stored or set, read from a settings snapshot.
    pub fn tmdb_from(&self, settings: &Settings) -> Option<TmdbClient> {
        let key = self.provider_key_from(settings, metadata::TMDB)?;
        Some(TmdbClient::new(self.http.clone(), &key, &self.config.tmdb_base_url))
    }

    /// Metadata sources as the user ordered them (`metadata::configured_order`).
    pub async fn metadata_order(&self) -> Vec<&'static ProviderInfo> {
        Self::metadata_order_from(&self.settings().await)
    }

    /// `metadata_order`, answered from a snapshot already read.
    pub fn metadata_order_from(settings: &Settings) -> Vec<&'static ProviderInfo> {
        metadata::configured_order(settings.raw("metadata_providers"))
    }

    /// The same list minus the sources that need a key and have none, those able to answer today.
    pub async fn metadata_providers(&self) -> Vec<&'static ProviderInfo> {
        self.metadata_providers_from(&self.settings().await)
    }

    /// `metadata_providers`, answered from a snapshot already read: the order and the keys come
    /// from one read, so a save between two reads cannot enable a source and hide its key.
    pub fn metadata_providers_from(&self, settings: &Settings) -> Vec<&'static ProviderInfo> {
        let keys = self.provider_keys_from(settings);
        Self::metadata_order_from(settings)
            .into_iter()
            .filter(|p| metadata::is_usable(p, &keys))
            .collect()
    }

    /// The sources enrichment asks, in the same order.
    pub async fn metadata_sources(&self) -> Vec<FetchingSource> {
        let settings = self.settings().await;
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
                metadata::TENRAI => Some(FetchingSource::Tenrai(TenraiClient::new(
                    self.http.clone(),
                    &self.config.tenrai_base_url,
                ))),
                metadata::MYANIMELIST => {
                    self.provider_key_from(&settings, metadata::MYANIMELIST).map(|key| {
                        FetchingSource::MyAnimeList(MyAnimeListClient::new(
                            self.http.clone(),
                            &key,
                            &self.config.myanimelist_base_url,
                        ))
                    })
                }
                metadata::OMDB => self.provider_key_from(&settings, metadata::OMDB).map(|key| {
                    FetchingSource::Omdb(OmdbClient::new(
                        self.http.clone(),
                        &key,
                        &self.config.omdb_base_url,
                        settings.get("omdb_daily_requests"),
                    ))
                }),
                metadata::TVDB => self.provider_key_from(&settings, metadata::TVDB).map(|key| {
                    FetchingSource::Tvdb(TvdbClient::new(
                        self.http.clone(),
                        &key,
                        self.tvdb_pin_from(&settings).as_deref(),
                        &self.config.tvdb_base_url,
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

    /// Translator for the configured UI language, which the texts Routarr writes follow too.
    pub async fn localizer(&self) -> Localizer {
        Localizer::new(&self.language().await)
    }

    /// The regions whose certifications count, upper-case, `US` when none is
    /// set, read from a settings snapshot.
    pub fn certification_regions_from(settings: &Settings) -> Vec<String> {
        Self::certification_regions_of(settings.raw("certification_regions"))
    }

    /// The same from the stored text alone, for a reader holding no snapshot.
    pub fn certification_regions_of(raw: Option<&str>) -> Vec<String> {
        raw.map(|v| {
            v.split(',')
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| {
            vec![crate::services::settings::default_of("certification_regions").into()]
        })
    }

    /// Read a setting, falling back to its default when absent or unparseable, and when the
    /// database fails to answer: for what only shows or paces.
    pub async fn setting<T: std::str::FromStr + Default>(&self, key: &str) -> T {
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| default_of(key))
    }

    /// Read a setting that bounds what is deleted or written, its default only when absent or
    /// unparseable. A failed read is an error: taken for the default, a retention would delete
    /// what a longer one keeps, and a batch limit let through more than the operator allows.
    pub async fn bounding_setting<T: std::str::FromStr + Default>(
        &self,
        key: &str,
    ) -> AppResult<T> {
        let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(stored.and_then(|v| v.trim().parse().ok()).unwrap_or_else(|| default_of(key)))
    }

    /// The category routing lands in when no rule matched, spelled here alone so every caller
    /// and the delete guard agree. It takes no `&self`: `routing.rs` holds no `AppState`.
    /// A failed read is an error, since `DEFAULT_CATEGORY` stands for a missing row only.
    pub async fn default_category<'e, E>(executor: E) -> crate::error::AppResult<String>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_category'")
                .fetch_optional(executor)
                .await?;
        Ok(stored.unwrap_or_else(|| DEFAULT_CATEGORY.to_string()))
    }

    /// Read a boolean setting: `true` in any case, or `1`, is on.
    pub async fn bool_setting(&self, key: &str) -> bool {
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .map_or_else(|| is_true(crate::services::settings::default_of(key)), |v| is_true(&v))
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
        Self::for_tests_on(crate::db::test_pool().await)
    }

    /// Test-only state around a database the test prepared itself.
    #[cfg(test)]
    pub fn for_tests_on(pool: SqlitePool) -> Self {
        let config = Config::for_tests();
        Self {
            http: crate::http::build_client(&config).expect("test http client"),
            // Salted as a start salts it, so what tests seal is what an installation stores.
            secrets: SecretBox::load(
                config.secret_key.as_deref(),
                None,
                std::path::Path::new("/nonexistent"),
                Some(b"routarr-test-salt"),
            )
            .unwrap(),
            tvdb_token: Arc::new(tokio::sync::Mutex::new(None)),
            paces: Default::default(),
            library_passes: Default::default(),
            jobs: JobRegistry::new(pool.clone()),
            api_key: Arc::new(std::sync::RwLock::new(resolve_api_key(&config))),
            sign_in: Arc::new(Default::default()),
            oidc_provider: Arc::new(tokio::sync::RwLock::new(None)),
            post_sync: Arc::new(tokio::sync::Mutex::new(None)),
            auto_apply_held: Arc::default(),
            notifications: Arc::default(),
            audit: Arc::new(crate::services::audit::Log::storing(pool.clone())),
            deprecated_calls: Arc::new(crate::api::deprecated::Calls::counting(pool.clone())),
            key_rates: Arc::default(),
            route_misses: Arc::default(),
            last_probe: Arc::default(),
            backup_copy_written: Arc::default(),
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

    /// The rules `AppState::setting` and `bool_setting` apply to one key.
    #[test]
    fn a_snapshot_trims_defaults_and_reads_switches() {
        let settings = stored(&[
            ("batch_limit", " 12 "),
            ("confirmation_threshold", "many"),
            ("global_dry_run", "TRUE"),
            ("auto_apply_enabled", "1"),
            ("notify_sync_failed", "yes"),
        ]);
        assert_eq!(settings.get::<i64>("batch_limit"), 12);
        assert_eq!(settings.get::<i64>("confirmation_threshold"), 10, "unparseable is the default");
        assert_eq!(settings.get::<i64>("backup_retention_count"), 7, "absent is the default");
        assert!(settings.bool("global_dry_run"));
        assert!(settings.bool("auto_apply_enabled"));
        assert!(!settings.bool("notify_sync_failed"), "`yes` is not a switch");
        assert!(settings.bool("backup_enabled"), "absent is the default");
        assert_eq!(settings.raw("batch_limit"), Some(" 12 "), "raw is as stored");
    }

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
