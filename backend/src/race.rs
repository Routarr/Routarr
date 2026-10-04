//! A point between a check and the write it guards.
//!
//! Nothing in a build that is not a test. A test holds one request there and
//! sends another, which proves that no write slips between the two: the check
//! and its write share one transaction taken for writing from its start
//! (`db::write_transaction`).

/// Reached once the check about `subject` (a category name) has passed and
/// before the write it guards. `point` names the check.
#[cfg(not(test))]
pub async fn checked(_point: &'static str, _subject: &str) {}

/// See the version built outside tests: here a test may hold the request.
#[cfg(test)]
pub async fn checked(point: &'static str, subject: &str) {
    crate::tests::races::hold(point, subject).await;
}
