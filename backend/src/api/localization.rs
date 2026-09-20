//! Serves the translation dictionary to the frontend.
//!
//! The frontend never ships its own copy: it asks for the dictionary of the
//! configured language and gets it already merged over English, so a key that
//! is not translated yet still renders as readable text.

use super::Json;
use axum::extract::State;
use serde::Serialize;

use crate::error::AppResult;
use crate::localization;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct LocalizationResponse {
    pub language: String,
    /// `ltr` or `rtl` for `language`.
    pub direction: &'static str,
    pub strings: std::collections::HashMap<String, String>,
}

/// The dictionary for the configured language.
///
/// The language is read back from the resolved `Localizer`, so the frontend is
/// told which language it actually got — English, if the stored value turned out
/// to be one this build does not ship.
pub async fn dictionary(State(state): State<AppState>) -> AppResult<Json<LocalizationResponse>> {
    let language = state.localizer().await.language().to_string();
    Ok(Json(LocalizationResponse {
        strings: localization::dictionary(&language),
        // Sent with the strings so the shell turns around in the same paint it
        // switches language, rather than a frame later.
        direction: localization::direction(&language),
        language,
    }))
}

/// Languages this build ships translations for.
pub async fn languages() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "default": localization::DEFAULT_LANGUAGE,
        "languages": localization::languages(),
    }))
}
