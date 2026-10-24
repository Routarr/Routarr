//! Serves the translation dictionary to the frontend, merged over English so an untranslated
//! key still reads.

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
    /// The placeholders that hold a count, grouped as the language groups
    /// digits.
    pub counts: &'static [&'static str],
    /// `ui_theme`, so the shell paints in it from its first frame, the sign-in screen and a
    /// reader's screens included, who cannot read the settings.
    pub theme: String,
}

/// The dictionary for the configured language, naming the language it holds: English when
/// the stored one is not shipped.
pub async fn dictionary(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> AppResult<axum::response::Response> {
    let json = serde_json::to_string(&answer(&state).await)?;
    Ok(super::revalidated(&headers, json))
}

/// What `/localization` answers, which the page also carries from the start.
pub async fn answer(state: &AppState) -> LocalizationResponse {
    let language = state.localizer().await.language().to_string();
    LocalizationResponse {
        strings: localization::dictionary(&language),
        // With the strings, so the direction flips in the same paint as the language.
        direction: localization::direction(&language),
        counts: localization::COUNTS,
        theme: state.setting::<String>("ui_theme").await,
        language,
    }
}

/// Languages this build ships translations for.
pub async fn languages() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "default": localization::DEFAULT_LANGUAGE,
        "languages": localization::languages(),
    }))
}
