//! Encryption of secrets at rest: the Arr and metadata source keys, the
//! notification address and the signing secrets.
//!
//! A value is stored as `enc:v2:<base64(nonce || ciphertext)>`, sealed with
//! the master key. A passphrase given as the master key is stretched with
//! Argon2id and the installation's salt, which the database holds and every
//! backup carries. `enc:v1:` is the same seal under a passphrase stretched
//! with SHA-256, still opened. A value without either prefix is plaintext,
//! returned as it is. `maintenance::reseal_secrets` seals both again as
//! `enc:v2:` at the next start.

use aes_gcm::aead::{Aead, AeadCore, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};

/// Operating-system randomness, or a refusal rather than a panic.
///
/// `getrandom` directly rather than `rand`'s `OsRng.fill_bytes`, which panics
/// when the system cannot supply entropy. For a key generator that is the wrong
/// shape: the one case where it matters is the one where the process should
/// stop and say so. It is also what `rand` calls underneath, so this costs a
/// dependency rather than a change of source.
fn random_bytes(buffer: &mut [u8]) -> AppResult<()> {
    getrandom::fill(buffer).map_err(|e| {
        AppError::Internal(format!("the operating system refused to supply randomness: {e}"))
    })
}

/// The nonce type, taken from the cipher rather than restated.
///
/// `Nonce` is generic over its size. Naming the size here would be a second
/// place to keep in step with `NONCE_LEN` below, and asking the cipher removes
/// the question.
type GcmNonce = Nonce<<Aes256Gcm as AeadCore>::NonceSize>;

const PREFIX: &str = "enc:v2:";
/// A seal under a passphrase stretched with SHA-256 rather than Argon2id.
const PREFIX_V1: &str = "enc:v1:";
const NONCE_LEN: usize = 12;

/// Holds the master key used to seal and open secrets.
///
/// A second, previous key can be supplied during a rotation: values are opened
/// with either key but always re-sealed with the current one, so an operator can
/// change `ROUTARR_SECRET_KEY` without locking themselves out of the database.
#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
    previous: Option<Aes256Gcm>,
    /// What else opens an `enc:v1:` value: the current and the previous
    /// passphrase stretched with SHA-256. A key given whole is the same key
    /// under both, and opens it as `cipher`.
    v1: Vec<Aes256Gcm>,
    /// What a new seal is written as: `enc:v1:` only for a passphrase with no
    /// salt to stretch it with.
    prefix: &'static str,
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBox(<redacted>)")
    }
}

impl SecretBox {
    /// Build from a user-supplied key, or from a key file generated on first run.
    ///
    /// The key file is written with `0600` so the secrets are no more readable
    /// than the database file itself.
    ///
    /// `previous` is a superseded key kept readable during a rotation. Values
    /// are opened with either key but always re-sealed with the current one.
    ///
    /// `salt` is the installation's ([`installation_salt`]), which a
    /// passphrase is stretched with. Without one a passphrase is stretched as
    /// `enc:v1:` was, for a database that holds no salt yet.
    pub fn load(
        configured: Option<&str>,
        previous: Option<&str>,
        key_path: &Path,
        salt: Option<&[u8]>,
    ) -> AppResult<Self> {
        let current = match configured {
            Some(k) => k.trim().to_string(),
            None => key_from_file(key_path)?,
        };
        let cipher = |input: &str| -> AppResult<Aes256Gcm> {
            Ok(Aes256Gcm::new(&Key::<Aes256Gcm>::from(derive_key(input, salt)?)))
        };
        let v1 = match salt {
            Some(_) => [Some(current.as_str()), previous]
                .into_iter()
                .flatten()
                .filter(|input| whole_key(input).is_none())
                .map(|input| Aes256Gcm::new(&Key::<Aes256Gcm>::from(stretched_v1(input))))
                .collect(),
            None => Vec::new(),
        };
        let prefix =
            if salt.is_some() || whole_key(&current).is_some() { PREFIX } else { PREFIX_V1 };

        Ok(Self {
            cipher: cipher(&current)?,
            previous: previous.map(cipher).transpose()?,
            v1,
            prefix,
        })
    }

    /// Encrypt a secret for storage. Already-encrypted values pass through.
    pub fn seal(&self, plaintext: &str) -> AppResult<String> {
        if Self::is_sealed(plaintext) {
            return Ok(plaintext.to_string());
        }
        let mut nonce_bytes = [0u8; NONCE_LEN];
        random_bytes(&mut nonce_bytes)?;
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| AppError::Internal("failed to encrypt secret".into()))?;

        let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        Ok(format!("{}{}", self.prefix, B64.encode(payload)))
    }

    /// Decrypt a stored secret. A plaintext value is returned as it is.
    pub fn open(&self, stored: &str) -> AppResult<String> {
        let (encoded, v1) = match (stored.strip_prefix(PREFIX), stored.strip_prefix(PREFIX_V1)) {
            (Some(encoded), _) => (encoded, &[][..]),
            (None, Some(encoded)) => (encoded, &self.v1[..]),
            (None, None) => return Ok(stored.to_string()),
        };

        let payload = B64
            .decode(encoded)
            .map_err(|_| AppError::Internal("stored secret is not valid base64".into()))?;

        if payload.len() <= NONCE_LEN {
            return Err(AppError::Internal("stored secret is truncated".into()));
        }

        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        // `split_at(NONCE_LEN)` after the length check above, so this cannot
        // fail, and if it ever could, it answers an error rather than a panic.
        let nonce = <&GcmNonce>::try_from(nonce_bytes)
            .map_err(|_| AppError::Internal("stored secret has a malformed nonce".into()))?;

        // Mid-rotation a value is still sealed with the superseded key.
        let plaintext = std::iter::once(&self.cipher)
            .chain(&self.previous)
            .chain(v1)
            .find_map(|key| key.decrypt(nonce, ciphertext).ok())
            .ok_or(())
            .map_err(|()| {
                AppError::Config(
                    "cannot decrypt a stored secret: ROUTARR_SECRET_KEY (or routarr.key) does not match this database. Set ROUTARR_PREVIOUS_SECRET_KEY to the old value to migrate.".into(),
                )
            })?;

        String::from_utf8(plaintext)
            .map_err(|_| AppError::Internal("decrypted secret is not valid UTF-8".into()))
    }

    /// True when the value should be rewritten under the current key:
    /// plaintext, a passphrase's `enc:v1:` seal once a salt stretches it, and a
    /// value still sealed with a superseded key.
    pub fn needs_reseal(&self, stored: &str) -> bool {
        if stored.starts_with(PREFIX_V1) && self.prefix == PREFIX {
            return true;
        }
        let Some(encoded) =
            stored.strip_prefix(self.prefix).or_else(|| stored.strip_prefix(PREFIX_V1))
        else {
            return true;
        };
        let Ok(payload) = B64.decode(encoded) else {
            return false;
        };
        if payload.len() <= NONCE_LEN {
            return false;
        }
        let (nonce, ciphertext) = payload.split_at(NONCE_LEN);
        let Ok(nonce) = <&GcmNonce>::try_from(nonce) else {
            return false;
        };
        self.cipher.decrypt(nonce, ciphertext).is_err()
    }

    /// True when the value is stored encrypted.
    pub fn is_sealed(stored: &str) -> bool {
        stored.starts_with(PREFIX) || stored.starts_with(PREFIX_V1)
    }
}

/// The master key the file beside the database holds, or a new one written
/// there when there is no file, or an empty one.
///
/// A file that cannot be read, or holds bytes that are not text, is somebody's
/// key: written over, it would take every credential sealed under it. An empty
/// file is what a power cut leaves of a key written moments before, and a start
/// only gets here with one once `main` has made sure no value in the database
/// was sealed with the key it held.
fn key_from_file(key_path: &Path) -> AppResult<String> {
    match std::fs::read_to_string(key_path) {
        Ok(stored) if !stored.trim().is_empty() => return Ok(stored.trim().to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(AppError::Config(format!(
                "cannot read the master key at {}: {e}. Fix the file or move it aside, since a \
                 new key would leave every stored credential unreadable",
                key_path.display()
            )));
        }
    }
    let mut bytes = [0u8; 32];
    random_bytes(&mut bytes)?;
    let encoded = B64.encode(bytes);
    write_private(key_path, encoded.as_bytes()).map_err(|e| {
        AppError::Config(format!("cannot write master key to {}: {e}", key_path.display()))
    })?;
    info!(
        "Generated a new master key at {}. Back it up alongside the database",
        key_path.display()
    );
    Ok(encoded)
}

/// The salt a passphrase given as the master key is stretched with, one per
/// installation, made by a migration and kept in the database.
pub async fn installation_salt(pool: &sqlx::SqlitePool) -> AppResult<String> {
    if let Some(salt) =
        sqlx::query_scalar("SELECT salt FROM secret_salt LIMIT 1").fetch_optional(pool).await?
    {
        return Ok(salt);
    }
    let salt = generate_secret()?[..32].to_string();
    sqlx::query("INSERT INTO secret_salt (salt) VALUES (?)").bind(&salt).execute(pool).await?;
    Ok(salt)
}

/// 32 bytes of OS randomness, hex-encoded.
///
/// URL-safe, shell-safe, and easy to copy out of a log without the ambiguity
/// base64 padding invites. Shared by the API key, the session ids and the
/// generated password, which want the same properties for the same reasons.
pub fn generate_secret() -> AppResult<String> {
    let mut bytes = [0u8; 32];
    random_bytes(&mut bytes)?;
    Ok(bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    }))
}

/// A secret the outgoing notification is signed with, as Standard Webhooks
/// writes one: `whsec_` and 32 random bytes in base64.
pub fn generate_signing_secret() -> AppResult<String> {
    let mut bytes = [0u8; 32];
    random_bytes(&mut bytes)?;
    Ok(format!("whsec_{}", B64.encode(bytes)))
}

/// The key a signing secret stands for, `None` for anything not shaped like
/// one.
pub fn signing_key(secret: &str) -> Option<Vec<u8>> {
    let key = B64.decode(secret.trim().strip_prefix("whsec_")?).ok()?;
    (key.len() >= 24).then_some(key)
}

/// Load the API key from `path`, generating one on first run.
///
/// An API that can move files on disk must not be open by omission, and the
/// Arrs set the same default. Generating one keeps the zero-configuration first
/// run: the key appears in the startup log and in a file beside the database.
///
/// Returns the key and whether it had to be created, so the caller can make the
/// first run loud and later ones quiet.
pub fn load_or_generate_api_key(path: &Path) -> AppResult<(String, bool)> {
    if let Some(existing) = read_api_key(path) {
        return Ok((existing, false));
    }

    let key = generate_secret()?;
    write_api_key(path, &key)?;
    Ok((key, true))
}

/// The stored key, if there is one worth reading.
///
/// An empty or whitespace-only file is not a key: it is what a truncated write
/// or a hand-emptied file leaves behind, and honouring it would mean accepting
/// an empty credential.
pub fn read_api_key(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Write the key with the permissions it needs, creating the directory if the
/// data directory has not been made yet.
pub fn write_api_key(path: &Path, key: &str) -> AppResult<()> {
    write_private(path, key.as_bytes()).map_err(|e| {
        AppError::Config(format!("cannot write the API key to {}: {e}", path.display()))
    })
}

/// Write a file that holds a secret, private from the moment it exists, and
/// whole or not at all.
///
/// `std::fs::write` then `chmod` opens it under the umask first (0644 under
/// the usual 022), and the chmod that follows only warns when the filesystem
/// refuses it, which a bind mount from SMB or NFS does. Created with the mode
/// instead, there is no window and nothing to warn about. Where modes are not
/// honoured the file is exactly as private as it can be there.
///
/// Written beside, flushed to the disk, then renamed over the old one: written
/// in place, a power cut between the truncation and the write leaves an empty
/// key, and the previous one gone.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let beside = path.with_file_name(format!("{name}.tmp"));
    let mut file = create_private(&beside, false)?;
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| std::fs::rename(&beside, path)) {
        std::fs::remove_file(&beside).ok();
        return Err(e);
    }
    sync_parent(path);
    Ok(())
}

/// Make a rename durable: it reaches the disk with the directory that holds
/// it. Best effort, since a filesystem that cannot open a directory to sync it
/// still renames.
pub fn sync_parent(path: &Path) {
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir).and_then(|dir| dir.sync_all()).ok();
    }
}

/// Open a file for writing with mode 0600, created if absent, and refused if
/// present when `exclusive` is set, so an archive is never written over.
pub fn create_private(path: &Path, exclusive: bool) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    // A file that already exists keeps its mode until it is set here.
    restrict_permissions(path);
    Ok(file)
}

/// Remove the stored key. A file that was never there is not an error: the
/// caller asked for there to be no key, and there is none.
pub fn remove_api_key(path: &Path) -> AppResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            Err(AppError::Config(format!("cannot remove the API key at {}: {e}", path.display())))
        }
    }
}

/// A base64-encoded 32-byte key is used as it is, and anything else is a
/// passphrase: stretched with Argon2id and the installation's salt, since a
/// copy of the database or of a backup lets anyone test phrases offline, and
/// the tag of any sealed value confirms a right guess. Without a salt, as an
/// `enc:v1:` value was sealed.
fn derive_key(input: &str, salt: Option<&[u8]>) -> AppResult<[u8; 32]> {
    if let Some(whole) = whole_key(input) {
        return Ok(whole);
    }
    let Some(salt) = salt else {
        return Ok(stretched_v1(input));
    };
    let mut out = [0u8; 32];
    argon2::Argon2::default().hash_password_into(input.trim().as_bytes(), salt, &mut out).map_err(
        |e| AppError::Config(format!("the master key passphrase cannot be stretched: {e}")),
    )?;
    Ok(out)
}

/// The key `input` is when it is written whole, as base64 of 32 bytes.
fn whole_key(input: &str) -> Option<[u8; 32]> {
    B64.decode(input.trim()).ok()?.try_into().ok()
}

/// A passphrase as `enc:v1:` stretched it: SHA-256, repeated, unsalted.
fn stretched_v1(input: &str) -> [u8; 32] {
    let trimmed = input.trim();
    let mut hasher = Sha256::new();
    hasher.update(b"routarr:secret-key:v1");
    hasher.update(trimmed.as_bytes());
    let mut digest = hasher.finalize();
    // Iterate to make trivial passphrases marginally more expensive to grind.
    for _ in 0..10_000 {
        let mut h = Sha256::new();
        h.update(b"routarr:secret-key:v1");
        h.update(digest);
        digest = h.finalize();
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Lock down a database file *and* its write-ahead sidecars.
///
/// `routarr.db-wal` holds every commit not yet checkpointed and `-shm` its
/// index: between checkpoints they contain exactly what the database contains,
/// including the sealed Arr credentials. Restricting only the `.db` would
/// leave the most recent data readable to anyone on the host.
pub fn restrict_database_permissions(path: &Path) {
    restrict_permissions(path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = std::path::PathBuf::from(sidecar);
        // Absent until the first write in WAL mode, which is not an error.
        if sidecar.exists() {
            restrict_permissions(&sidecar);
        }
    }
}

/// Best-effort `chmod 600`, ignored on platforms without Unix permissions.
pub fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            warn!("Could not restrict permissions on {}: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    /// Radarr and Sonarr generate their key at first start, and this API can
    /// move files: starting open with a warning is not the posture to ship.
    #[test]
    fn an_api_key_is_generated_on_first_run_and_reused_afterwards() {
        let dir = crate::tests::TempDir::new("key");
        let path = dir.join("routarr.api_key");

        let (first, generated) = load_or_generate_api_key(&path).unwrap();
        assert!(generated, "the first run must create a key");
        assert_eq!(first.len(), 64, "32 bytes, hex-encoded");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "must be safe to paste anywhere");

        // Every later start reuses it: a key that changed on restart would lock
        // out every client that had already stored it.
        let (second, generated_again) = load_or_generate_api_key(&path).unwrap();
        assert_eq!(second, first);
        assert!(!generated_again);
    }

    /// Two installations must not share a key.
    #[test]
    fn each_generated_key_is_unique() {
        let dir = crate::tests::TempDir::new("key-uniq");

        let a = dir.join("a.key");
        let b = dir.join("b.key");

        let (first, _) = load_or_generate_api_key(&a).unwrap();
        let (second, _) = load_or_generate_api_key(&b).unwrap();
        assert_ne!(first, second);
    }

    /// A secret file is born 0600 rather than locked down after: with the
    /// usual umask, `write` then `chmod` would leave it readable for the
    /// instant in between.
    #[cfg(unix)]
    #[test]
    fn a_private_file_is_created_with_its_mode_and_truncated_on_rewrite() {
        use std::os::unix::fs::PermissionsExt;

        let dir = crate::tests::TempDir::new("private");
        let path = dir.join("secret");

        write_private(&path, b"a much longer first value").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "created readable to others");

        write_private(&path, b"short").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"short", "the old value was left behind");

        assert!(create_private(&path, true).is_err(), "an existing file must not be reopened");
    }

    /// A key is written beside its file and renamed over it: a write that
    /// fails leaves the previous key whole, and nothing behind.
    #[test]
    fn a_failed_key_write_leaves_the_previous_key_whole() {
        let dir = crate::tests::TempDir::new("key-durable");
        let path = dir.join("routarr.key");
        write_private(&path, b"the key of before").unwrap();
        assert!(!dir.join("routarr.key.tmp").exists(), "the file written beside was left");

        std::fs::create_dir(dir.join("routarr.key.tmp")).unwrap();
        assert!(write_private(&path, b"the key of after").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"the key of before");
    }

    /// The key file is as readable as the database, and no more.
    #[cfg(unix)]
    #[test]
    fn the_generated_key_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = crate::tests::TempDir::new("key-perm");
        let path = dir.join("routarr.api_key");

        load_or_generate_api_key(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the API key was left readable to others");
    }

    use super::*;

    /// The key generated at first start is the key every later start reads. A
    /// key written in one form and read back in another opens none of the
    /// sealed Arr and metadata keys after a restart, and nothing else warns.
    #[test]
    fn a_generated_master_key_opens_what_it_sealed_after_a_restart() {
        let dir = crate::tests::TempDir::new("master-key");
        let path = dir.join("routarr.key");

        let sealed = SecretBox::load(None, None, &path, None).unwrap().seal("the-arr-key").unwrap();
        assert!(path.exists(), "the first start writes the key it generated");

        let restarted = SecretBox::load(None, None, &path, None).unwrap();
        assert_eq!(restarted.open(&sealed).unwrap(), "the-arr-key");
    }

    fn boxed() -> SecretBox {
        SecretBox::load(
            Some("dGVzdC1rZXktMzItYnl0ZXMtZm9yLXVuaXQtdGVzdCE="),
            None,
            Path::new("/nonexistent"),
            None,
        )
        .unwrap()
    }

    #[test]
    fn roundtrip() {
        let sb = boxed();
        let sealed = sb.seal("super-secret-api-key").unwrap();
        assert!(SecretBox::is_sealed(&sealed), "{sealed}");
        assert_eq!(sb.open(&sealed).unwrap(), "super-secret-api-key");
    }

    #[test]
    fn plaintext_passes_through() {
        let sb = boxed();
        assert_eq!(sb.open("legacy-plaintext").unwrap(), "legacy-plaintext");
    }

    #[test]
    fn sealing_is_idempotent() {
        let sb = boxed();
        let once = sb.seal("k").unwrap();
        assert_eq!(sb.seal(&once).unwrap(), once);
    }

    #[test]
    fn nonce_is_random() {
        let sb = boxed();
        assert_ne!(sb.seal("same").unwrap(), sb.seal("same").unwrap());
    }

    #[test]
    fn a_previous_key_still_opens_its_values() {
        let old =
            SecretBox::load(Some("old-passphrase"), None, Path::new("/nonexistent"), None).unwrap();
        let sealed = old.seal("arr-key").unwrap();

        let rotated = SecretBox::load(
            Some("new-passphrase"),
            Some("old-passphrase"),
            Path::new("/nonexistent"),
            None,
        )
        .unwrap();

        assert_eq!(rotated.open(&sealed).unwrap(), "arr-key");
        assert!(rotated.needs_reseal(&sealed), "it must be rewritten under the new key");

        let resealed = rotated.seal(&rotated.open(&sealed).unwrap()).unwrap();
        assert!(!rotated.needs_reseal(&resealed));
    }

    #[test]
    fn plaintext_always_needs_resealing() {
        let sb = boxed();
        assert!(sb.needs_reseal("legacy-plaintext"));
        assert!(!sb.needs_reseal(&sb.seal("x").unwrap()));
    }

    #[test]
    fn wrong_key_fails_closed() {
        let sealed = boxed().seal("secret").unwrap();
        let other = SecretBox::load(
            Some("a-completely-different-passphrase"),
            None,
            Path::new("/nonexistent"),
            None,
        )
        .unwrap();
        assert!(other.open(&sealed).is_err());
    }
}
