use serde::{Deserialize, Serialize};

/// A user-defined category for media classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_default: bool,
    pub display_order: i64,
    pub created_at: String,
}

/// Request body for creating/updating a category.
#[derive(Debug, Deserialize)]
pub struct RenameCategoryRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub display_order: i64,
}

/// A category with the counts that make it safe (or unsafe) to delete.
#[derive(Debug, Clone, Serialize)]
pub struct CategoryWithUsage {
    #[serde(flatten)]
    pub category: Category,
    pub rule_count: i64,
    pub root_folder_count: i64,
}
