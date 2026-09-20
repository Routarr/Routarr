//! The stored format, pinned against the library that produces it.
//!
//! `enc:v1:` values live in the user's database and outlive any dependency
//! upgrade. A round-trip test proves the current build agrees with itself,
//! which is exactly what an upgrade that silently changed the format would also
//! satisfy — so the value below was captured from an older build and is checked
//! rather than recomputed.
//!
//! It carries no secret: the key and the plaintext are both invented here.

use crate::crypto::SecretBox;

/// Sealed by aes-gcm 0.10 with `rand` 0.8, before the move to aes-gcm 0.11 and
/// `getrandom`. Regenerating this defeats the purpose — if it stops opening,
/// the upgrade broke every database in the field, and that is the finding.
const SEALED_BY_AN_OLDER_BUILD: &str =
    "enc:v1:I+41qjR2qs5HaE4sSs5wIenO54BViu+hly5MWxF10qBY48O5ed3/22Whhw==";
const ITS_KEY: &str = "routarr-format-fixture-key";
const ITS_PLAINTEXT: &str = "the-arr-api-key";

fn secrets(key: &str) -> SecretBox {
    SecretBox::load(Some(key), None, std::path::Path::new("/tmp/routarr-format-test.key")).unwrap()
}

#[tokio::test]
async fn a_value_sealed_by_an_older_build_still_opens() {
    let opened = secrets(ITS_KEY).open(SEALED_BY_AN_OLDER_BUILD).unwrap();
    assert_eq!(opened, ITS_PLAINTEXT, "the stored format moved under an upgrade");
}

#[tokio::test]
async fn the_wrong_master_key_does_not_open_it() {
    // Otherwise the test above would pass on a build that had stopped
    // decrypting at all and returned its input.
    assert!(secrets("a-different-key").open(SEALED_BY_AN_OLDER_BUILD).is_err());
}
