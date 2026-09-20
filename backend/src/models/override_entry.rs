use serde::{Deserialize, Serialize};

/// A manual override for a specific media item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverrideEntry {
    pub id: String,
    pub media_id: String,
    pub target_category: String,
    pub reason: Option<String>,
    pub locked: bool,
    pub created_at: String,
}

/// Request body for creating an override.
#[derive(Debug, Deserialize)]
pub struct CreateOverrideRequest {
    pub media_id: String,
    pub target_category: String,
    pub reason: Option<String>,
    #[serde(default)]
    pub locked: bool,
}

/// Override with associated media info for display.
#[derive(Debug, Serialize)]
pub struct OverrideWithMedia {
    #[serde(flatten)]
    pub override_entry: OverrideEntry,
    pub media_title: String,
    pub media_type: String,
    pub instance_name: String,
}
