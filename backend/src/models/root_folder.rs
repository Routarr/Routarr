use serde::{Deserialize, Serialize};

/// A root folder synchronized from an Arr instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootFolder {
    pub id: String,
    pub instance_id: String,
    /// `None` for a folder Routarr declares: it has no id in the Arr, and
    /// typed as `i64` sqlx decoded the NULL to `0` — a fabricated id on the
    /// wire, harmless only until something believes it.
    pub arr_id: Option<i64>,
    pub path: String,
    pub free_space: Option<i64>,
    pub accessible: bool,
    pub category: Option<String>,
    pub last_synced_at: Option<String>,
    /// When the folder last answered, as opposed to when it was last seen in a
    /// pass. `None` for one that has never answered.
    pub last_accessible_at: Option<String>,
    /// `arr` for a folder the instance reports, `declared` for one typed into
    /// Routarr. Only the second may be deleted here.
    pub origin: String,
}

/// Request to declare a destination the Arr does not report.
#[derive(Debug, Deserialize)]
pub struct DeclareRootFolder {
    pub instance_id: String,
    pub path: String,
}

/// Request to update a root folder's category mapping.
#[derive(Debug, Deserialize)]
pub struct UpdateRootFolderCategory {
    pub category: Option<String>,
}

/// Root folder with instance info for display.
#[derive(Debug, Serialize)]
pub struct RootFolderWithInstance {
    #[serde(flatten)]
    pub root_folder: RootFolder,
    pub instance_name: String,
    pub instance_type: String,
}
