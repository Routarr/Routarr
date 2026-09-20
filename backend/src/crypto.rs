//! Encryption of secrets at rest (Arr API keys).
//!
//! Values are stored as `enc:v1:<base64(nonce || ciphertext)>`. Anything that
//! does not carry that prefix is treated as legacy plaintext and returned as-is,
//! so upgrading an existing database never loses access to the instances; the
//! values are re-encrypted the next time they are written.

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
/// `Nonce` became generic over its size in aes-gcm 0.11. Naming the size here
/// would be a second place to keep in step with `NONCE_LEN` below; asking the
/// cipher removes the question.
type GcmNonce = Nonce<<Aes256Gcm as AeadCore>::NonceSize>;

const PREFIX: &str = "enc:v1:";
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
    /// `previous` is a superseded key kept readable during a rotation; values
    /// are opened with either key but always re-sealed with the current one.
    pub fn load(
        configured: Option<&str>,
        previous: Option<&str>,
        key_path: &Path,
    ) -> AppResult<Self> {
        let raw = match configured {
            Some(k) => derive_key(k),
            None => {
                let stored = std::fs::read_to_string(key_path).ok().map(|s| s.trim().to_string());
                match stored.filter(|s| !s.is_empty()) {
                    Some(k) => derive_key(&k),
                    None => {
                        let mut bytes = [0u8; 32];
                        random_bytes(&mut bytes)?;
                        let encoded = B64.encode(bytes);
                        write_private(key_path, encoded.as_bytes()).map_err(|e| {
                            AppError::Config(format!(
                                "cannot write master key to {}: {e}",
                                key_path.display()
                            ))
                        })?;
                        info!(
                            "Generated a new master key at {} — back it up alongside the database",
                            key_path.display()
                        );
                        bytes
                    }
                }
            }
        };

        let key = Key::<Aes256Gcm>::from(raw);
        let previous = previous.map(|p| Aes256Gcm::new(&Key::<Aes256Gcm>::from(derive_key(p))));

        Ok(Self { cipher: Aes256Gcm::new(&key), previous })
    }

    /// Encrypt a secret for storage. Already-encrypted values pass through.
    pub fn seal(&self, plaintext: &str) -> AppResult<String> {
        if plaintext.starts_with(PREFIX) {
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

        Ok(format!("{PREFIX}{}", B64.encode(payload)))
    }

    /// Decrypt a stored secret. Legacy plaintext values are returned unchanged.
    pub fn open(&self, stored: &str) -> AppResult<String> {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_string());
        };

        let payload = B64
            .decode(encoded)
            .map_err(|_| AppError::Internal("stored secret is not valid base64".into()))?;

        if payload.len() <= NONCE_LEN {
            return Err(AppError::Internal("stored secret is truncated".into()));
        }

        let (nonce_bytes, ciphertext) = payload.split_at(NONCE_LEN);
        // `split_at(NONCE_LEN)` after the length check above, so this cannot
        // fail — but the call it replaces would have panicked if it ever could.
        let nonce = <&GcmNonce>::try_from(nonce_bytes)
            .map_err(|_| AppError::Internal("stored secret has a malformed nonce".into()))?;

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .or_else(|_| {
                // Mid-rotation: the value is still sealed with the superseded key.
                self.previous
                    .as_ref()
                    .ok_or(())
                    .and_then(|previous| previous.decrypt(nonce, ciphertext).map_err(|_| ()))
            })
            .map_err(|_| {
                AppError::Config(
                    "cannot decrypt a stored secret — ROUTARR_SECRET_KEY (or routarr.key) does not match this database. Set ROUTARR_PREVIOUS_SECRET_KEY to the old value to migrate.".into(),
                )
            })?;

        String::from_utf8(plaintext)
            .map_err(|_| AppError::Internal("decrypted secret is not valid UTF-8".into()))
    }

    /// True when the value should be rewritten under the current key.
    ///
    /// Covers both legacy plaintext and values still sealed with a superseded key.
    pub fn needs_reseal(&self, stored: &str) -> bool {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
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
        stored.starts_with(PREFIX)
    }
}

/// Load the API key from `path`, generating one on first run.
///
/// An API that can move files on disk must not be open by omission, and the
/// Arrs set the same default. Generating one keeps the zero-configuration first
/// run: the key appears in the startup log and in a file beside the database.
///
/// Returns the key and whether it had to be created, so the caller can make the
/// first run loud and later ones quiet.
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

/// Write a file that holds a secret, private from the moment it exists.
///
/// `std::fs::write` then `chmod` opens it under the umask first — 0644 under
/// the usual 022 — and the chmod that follows only warns when the filesystem
/// refuses it, which a bind mount from SMB or NFS does. Created with the mode
/// instead, there is no window and nothing to warn about; where modes are not
/// honoured the file is exactly as private as it can be there.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = create_private(path, false)?;
    file.write_all(contents)?;
    file.flush()
}

/// Open a file for writing with mode 0600, created if absent — refused if
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
    // A file that already existed keeps the mode it had; say what it should be.
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

/// Accept either a base64-encoded 32-byte key or an arbitrary passphrase.
///
/// A passphrase is stretched with SHA-256 over a fixed domain-separation label.
/// This is deliberately not a memory-hard KDF: the key never leaves the host and
/// the threat model is "someone copied routarr.db", not offline cracking of a
/// user password. Supply a base64 32-byte key for full strength.
fn derive_key(input: &str) -> [u8; 32] {
    let trimmed = input.trim();
    if let Ok(decoded) = B64.decode(trimmed)
        && decoded.len() == 32
    {
        let mut out = [0u8; 32];
        out.copy_from_slice(&decoded);
        return out;
    }

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

/// Best-effort `chmod 600`; ignored on platforms without Unix permissions.
/// Lock down a database file *and* its write-ahead sidecars.
///
/// `routarr.db-wal` holds every commit not yet checkpointed and `-shm` its
/// index: between checkpoints they contain exactly what the database contains,
/// including the sealed Arr credentials. Restricting only the `.db` left the
/// most recent data readable to anyone on the host.
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
        let dir = std::env::temp_dir().join(format!("routarr-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("routarr.api_key");
        let _ = std::fs::remove_file(&path);

        let (first, generated) = load_or_generate_api_key(&path).unwrap();
        assert!(generated, "the first run must create a key");
        assert_eq!(first.len(), 64, "32 bytes, hex-encoded");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "must be safe to paste anywhere");

        // Every later start reuses it: a key that changed on restart would lock
        // out every client that had already stored it.
        let (second, generated_again) = load_or_generate_api_key(&path).unwrap();
        assert_eq!(second, first);
        assert!(!generated_again);

        std::fs::remove_file(&path).ok();
    }

    /// Two installations must not share a key.
    #[test]
    fn each_generated_key_is_unique() {
        let dir = std::env::temp_dir().join(format!("routarr-key-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let a = dir.join("a.key");
        let b = dir.join("b.key");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);

        let (first, _) = load_or_generate_api_key(&a).unwrap();
        let (second, _) = load_or_generate_api_key(&b).unwrap();
        assert_ne!(first, second);

        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
    }

    /// A secret file is born 0600 rather than locked down after: with the
    /// usual umask, `write` then `chmod` left it readable for the instant in
    /// between, and only warned where the chmod was refused.
    #[cfg(unix)]
    #[test]
    fn a_private_file_is_created_with_its_mode_and_truncated_on_rewrite() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("routarr-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret");
        let _ = std::fs::remove_file(&path);

        write_private(&path, b"a much longer first value").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "created readable to others");

        write_private(&path, b"short").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"short", "the old value was left behind");

        assert!(create_private(&path, true).is_err(), "an existing file must not be reopened");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The key file is as readable as the database, and no more.
    #[cfg(unix)]
    #[test]
    fn the_generated_key_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("routarr-key-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("routarr.api_key");
        let _ = std::fs::remove_file(&path);

        load_or_generate_api_key(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the API key was left readable to others");

        std::fs::remove_file(&path).ok();
    }

    use super::*;

    fn boxed() -> SecretBox {
        SecretBox::load(
            Some("dGVzdC1rZXktMzItYnl0ZXMtZm9yLXVuaXQtdGVzdHMh"),
            None,
            Path::new("/nonexistent"),
        )
        .unwrap()
    }

    #[test]
    fn roundtrip() {
        let sb = boxed();
        let sealed = sb.seal("super-secret-api-key").unwrap();
        assert!(sealed.starts_with(PREFIX));
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
        let old = SecretBox::load(Some("old-passphrase"), None, Path::new("/nonexistent")).unwrap();
        let sealed = old.seal("arr-key").unwrap();

        let rotated = SecretBox::load(
            Some("new-passphrase"),
            Some("old-passphrase"),
            Path::new("/nonexistent"),
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
        )
        .unwrap();
        assert!(other.open(&sealed).is_err());
    }
}
