use serde::{Deserialize, Serialize};

/// An exception: a title pinned to a category by hand, which outranks every
/// rule.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OverrideEntry {
    pub id: String,
    pub media_id: String,
    pub target_category: String,
    pub reason: Option<String>,
    pub created_at: String,
    /// Who set it: an application's name, or the person a sign-in mode names.
    pub subject: Option<String>,
}

/// A title to pin, and the category to pin it to.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateOverrideRequest {
    /// The title's id in Routarr, as `/media` lists it.
    pub media_id: String,
    /// The name of an existing category.
    pub target_category: String,
    pub reason: Option<String>,
}

/// An exception, with the title it pins.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OverrideWithMedia {
    #[serde(flatten)]
    pub override_entry: OverrideEntry,
    pub media_title: String,
    pub media_type: String,
    pub instance_name: String,
}
