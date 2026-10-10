use serde::{Deserialize, Serialize};

/// A user-defined category for media classification.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Category {
    pub id: String,
    /// What rules, folders and exceptions name it by: lower case, letters,
    /// digits, `-` and `_`.
    pub name: String,
    pub description: Option<String>,
    /// Whether it is the fallback, where every title no rule matches goes.
    pub is_default: bool,
    /// Where it sorts among the categories, lowest first.
    pub display_order: i64,
    /// When it was made, in UTC.
    #[serde(serialize_with = "crate::timestamp::rfc3339")]
    #[schema(format = DateTime)]
    pub created_at: String,
}

/// Request body for renaming a category.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameCategoryRequest {
    /// The new name, stored as `Category.name` says.
    pub name: String,
}

/// A category to make.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateCategoryRequest {
    /// Trimmed and lower-cased: letters, digits, `-` and `_`.
    pub name: String,
    /// What it holds, for the person reading the list.
    pub description: Option<String>,
    /// `true` makes it the fallback for every title no rule matches, in place
    /// of the current one. An application key may not.
    #[serde(default)]
    pub is_default: bool,
    /// Where it sorts among the categories, lowest first. Defaults to 0.
    #[serde(default)]
    pub display_order: i64,
}

/// A category with the counts that make it safe (or unsafe) to delete.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CategoryWithUsage {
    #[serde(flatten)]
    pub category: Category,
    pub rule_count: i64,
    pub root_folder_count: i64,
}
