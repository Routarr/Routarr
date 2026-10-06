//! The stored format, pinned against the library that produces it, and the
//! master key a start opens it with.
//!
//! Sealed values live in the user's database and outlive any dependency
//! upgrade. A round-trip test proves the current build agrees with itself,
//! which an upgrade that silently changed the format would also satisfy. The
//! values below therefore come from earlier builds and are checked rather than
//! recomputed.
//!
//! They carry no secret: the key, the salt and the plaintext are all invented
//! here.

use crate::crypto::SecretBox;

/// Sealed by aes-gcm 0.10 with `rand` 0.8, older than the aes-gcm 0.11 and
/// `getrandom` this build uses. Regenerating it defeats the purpose: if it
/// stops opening, an upgrade broke every database in the field, and that is the
/// finding.
const SEALED_BY_AN_OLDER_BUILD: &str =
    "enc:v1:I+41qjR2qs5HaE4sSs5wIenO54BViu+hly5MWxF10qBY48O5ed3/22Whhw==";
const ITS_KEY: &str = "routarr-format-fixture-key";
const ITS_PLAINTEXT: &str = "the-arr-api-key";

/// The same plaintext under the same passphrase, stretched with Argon2id and
/// the salt below. Regenerating it defeats the purpose, as above.
const SEALED_WITH_A_SALT: &str =
    "enc:v2:FbHYYngcRt8NJCKa5YFVWSDvljmcKGYLJmtuDDsv5bHUpafU6YFo3okldw==";
const ITS_SALT: &[u8] = b"routarr-format-fixture-salt";

fn salted(key: &str, salt: &[u8]) -> SecretBox {
    SecretBox::load(Some(key), None, std::path::Path::new("/nonexistent"), Some(salt)).unwrap()
}

fn secrets(key: &str) -> SecretBox {
    SecretBox::load(Some(key), None, std::path::Path::new("/tmp/routarr-format-test.key"), None)
        .unwrap()
}

#[tokio::test]
async fn a_value_sealed_by_an_older_build_still_opens() {
    let opened = secrets(ITS_KEY).open(SEALED_BY_AN_OLDER_BUILD).unwrap();
    assert_eq!(opened, ITS_PLAINTEXT, "the stored format moved under an upgrade");
    let opened = salted(ITS_KEY, ITS_SALT).open(SEALED_WITH_A_SALT).unwrap();
    assert_eq!(opened, ITS_PLAINTEXT, "the stretch of a passphrase moved under an upgrade");
}

/// A copy of the database lets anyone test passphrases offline, and the tag
/// of any sealed value confirms a right guess. Stretched with Argon2id and the
/// installation's own salt, one phrase is another key on another
/// installation, and a guess costs what Argon2id costs.
#[test]
fn a_passphrase_is_stretched_with_the_installations_salt() {
    let here = salted(ITS_KEY, ITS_SALT);
    let sealed = here.seal(ITS_PLAINTEXT).unwrap();
    assert!(sealed.starts_with("enc:v2:"), "{sealed}");
    assert_eq!(salted(ITS_KEY, ITS_SALT).open(&sealed).unwrap(), ITS_PLAINTEXT);
    assert!(salted(ITS_KEY, b"another-installation").open(&sealed).is_err());
    assert!(secrets(ITS_KEY).open(&sealed).is_err(), "the salt changed nothing");
}

/// A value sealed under the passphrase as SHA-256 stretched it still opens
/// once a salt is there, and is sealed again under Argon2id at the next start.
#[test]
fn a_value_sealed_under_the_older_stretch_opens_and_is_sealed_again() {
    let salted = salted(ITS_KEY, ITS_SALT);
    assert_eq!(salted.open(SEALED_BY_AN_OLDER_BUILD).unwrap(), ITS_PLAINTEXT);
    assert!(salted.needs_reseal(SEALED_BY_AN_OLDER_BUILD));

    let resealed = salted.seal(&salted.open(SEALED_BY_AN_OLDER_BUILD).unwrap()).unwrap();
    assert!(resealed.starts_with("enc:v2:"), "{resealed}");
    assert!(!salted.needs_reseal(&resealed));
}

/// A start with the database and without its master key: `routarr.db`
/// copied alone to a new volume, the key file deleted, or emptied by a power
/// cut. A new key opens none of what the database holds, so the start stops
/// and names the file, leaving it as it found it, unless the operator says to
/// make a new key.
#[tokio::test]
async fn a_database_with_sealed_values_refuses_a_fresh_master_key() {
    for emptied in [false, true] {
        let dir = super::TempDir::new("lost-master-key");
        let mut config = crate::config::Config::for_tests();
        config.set_db_path(dir.join("routarr.db"));
        config.secret_key = None;
        let key_path = config.secret_key_path();

        let (_, pool, secrets) = crate::open_storage(&config).await.unwrap();
        sqlx::query(
            "INSERT INTO instances (id, name, instance_type, base_url, api_key)
             VALUES ('inst-1', 'Radarr', 'radarr', 'http://127.0.0.1:1', ?)",
        )
        .bind(secrets.seal("arr-key").unwrap())
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
        if emptied {
            std::fs::write(&key_path, "").unwrap();
        } else {
            std::fs::remove_file(&key_path).unwrap();
        }

        let refused = crate::open_storage(&config).await.err().map(|e| e.to_string());
        let refused =
            refused.unwrap_or_else(|| panic!("started with a new key, emptied: {emptied}"));
        assert!(refused.contains("routarr.key"), "{refused}");
        assert!(refused.contains("ROUTARR_ALLOW_NEW_MASTER_KEY"), "{refused}");
        assert_eq!(key_path.exists(), emptied, "the key file was written");

        config.allow_new_master_key = true;
        let (_, pool, _) = crate::open_storage(&config).await.unwrap();
        pool.close().await;
        assert!(!std::fs::read_to_string(&key_path).unwrap().trim().is_empty());
    }
}

/// A key made at the first start, with nothing sealed yet, is a key nobody
/// depends on: an empty file then is a power cut moments after it was
/// written, and a new key is made in its place.
#[tokio::test]
async fn an_empty_key_file_with_nothing_sealed_is_made_again() {
    let dir = super::TempDir::new("empty-master-key");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.secret_key = None;
    std::fs::write(config.secret_key_path(), "").unwrap();

    let (_, pool, _) = crate::open_storage(&config).await.unwrap();
    pool.close().await;
    assert!(!std::fs::read_to_string(config.secret_key_path()).unwrap().trim().is_empty());
}

/// The key and the password a first start makes are in their files, and the
/// log says where: a log shipper keeps every line it is sent.
#[tokio::test]
async fn the_generated_key_and_password_stay_out_of_the_log() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    let dir = super::TempDir::new("quiet-first-start");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    config.auth_mode = crate::config::AuthMode::ApiKey;

    let capture = super::LogCapture::default();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(capture.clone()));
    let first_start = async {
        let (key, pool, _) = crate::open_storage(&config).await.unwrap();
        crate::services::accounts::ensure_account(&pool, &config.password_path()).await.unwrap();
        pool.close().await;
        key.unwrap()
    };
    let key = first_start.with_subscriber(tracing::Dispatch::new(subscriber)).await;

    let password = std::fs::read_to_string(config.password_path()).unwrap();
    let log = capture.contents();
    assert!(log.contains("Generated an API key"), "{log}");
    assert!(!log.contains(&key), "the API key is in the log:\n{log}");
    assert!(!log.contains(password.trim()), "the password is in the log:\n{log}");
}

/// A key file that cannot be read as text, as `openssl rand 32 > routarr.key`
/// writes it, is refused by name. Replaced by a new key, it would take every
/// credential sealed under it, and the old key with them.
#[test]
fn a_master_key_file_that_cannot_be_read_is_refused_and_left_as_it_is() {
    let dir = super::TempDir::new("unreadable-master-key");
    let path = dir.join("routarr.key");
    let raw = [0xffu8, 0xfe, 0x00, 0x9c, 0x41, 0x12, 0xc3, 0x28];
    std::fs::write(&path, raw).unwrap();

    let refused = SecretBox::load(None, None, &path, None);
    assert!(refused.is_err(), "an unreadable key file was taken for no key at all");
    assert_eq!(std::fs::read(&path).unwrap(), raw, "the key file was overwritten");

    // A missing file is the one case that makes a new key.
    std::fs::remove_file(&path).unwrap();
    SecretBox::load(None, None, &path, None).unwrap();
    assert!(path.exists());
}

/// Every value a passphrase sealed is opened with the installation's salt.
/// A database without it has been edited by hand, and a new salt would leave
/// all of them unreadable: the start stops instead.
#[tokio::test]
async fn a_database_without_its_salt_refuses_to_start() {
    let dir = super::TempDir::new("no-salt");
    let mut config = crate::config::Config::for_tests();
    config.set_db_path(dir.join("routarr.db"));
    let (_, pool, _) = crate::open_storage(&config).await.unwrap();
    sqlx::query("DELETE FROM secret_salt").execute(&pool).await.unwrap();
    pool.close().await;

    let refused = crate::open_storage(&config).await.err().map(|e| e.to_string());
    assert!(refused.as_deref().is_some_and(|e| e.contains("secret_salt")), "{refused:?}");
}
