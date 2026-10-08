//! A move as Radarr and Sonarr both take it: the title's record pointed at
//! another root folder, then, with its files, a command the Arr queues to
//! carry them there.

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use super::{send_json, send_ok};
use crate::error::{AppError, AppResult};

/// One Arr's API, as a move reaches it.
pub(crate) struct Api<'a> {
    pub service: &'static str,
    pub client: &'a Client,
    pub base_url: &'a str,
    pub api_key: &'a str,
}

impl Api<'_> {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.base_url)).header("X-Api-Key", self.api_key)
    }

    /// Point the title `resource/id` (`movie/10`, `series/20`) at `root`,
    /// keeping its folder name, and answer the path it now has.
    ///
    /// The whole record is read, patched and sent back, since a PUT drops
    /// the fields it leaves out. The path goes in full: both editors name the
    /// folder from the Arr's naming format, which renames it on disk under
    /// every tool reading the library by path, and Radarr's refuses a root
    /// folder Radarr does not list, which a declared destination never is.
    pub async fn repoint(
        &self,
        resource: &str,
        id: i64,
        root: &str,
        move_files: bool,
    ) -> AppResult<String> {
        let mut record: Value =
            send_json(self.service, self.get(&format!("/api/v3/{resource}/{id}"))).await?;
        // Assigning a key of a `Value` that is not an object panics. A proxy
        // answering 200 with a cached `[]`, or a base URL pointing at another
        // service on the same host, is enough to get one.
        if !record.is_object() {
            return Err(AppError::ExternalApi {
                service: self.service.to_string(),
                status: 0,
                message: format!(
                    "{resource} {id} came back as {}, not an object",
                    kind_of(&record)
                ),
                retry_after: None,
            });
        }
        let path = join_path(root, &folder_name(&record));
        record["rootFolderPath"] = Value::String(root.to_string());
        record["path"] = Value::String(path.clone());

        send_ok(
            self.service,
            self.client
                .put(format!("{}/api/v3/{resource}/{id}?moveFiles={move_files}", self.base_url))
                .header("X-Api-Key", self.api_key)
                .json(&record),
        )
        .await?;
        Ok(path)
    }

    /// The moves among the commands the Arr lists: queued, running, and
    /// those ended in the last few minutes, which it keeps that long.
    pub async fn move_commands(&self) -> AppResult<Vec<MoveCommand>> {
        let listed: Vec<CommandDto> = send_json(self.service, self.get("/api/v3/command")).await?;
        Ok(listed.into_iter().flat_map(CommandDto::moves).collect())
    }
}

/// One title an Arr's move command carries.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveCommand {
    /// The command's id, higher for a later command.
    pub id: i64,
    /// The title's id in the Arr.
    pub item: i64,
    /// The folder the files go to. A bulk command names only the root folder,
    /// and leaves the folder to the naming format.
    pub destination: Option<String>,
    pub state: CommandState,
    /// What the Arr last said about it.
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandState {
    /// Queued or started.
    Running,
    Completed,
    /// Failed, aborted, cancelled or orphaned, in the Arr's word.
    Ended(String),
}

#[derive(Deserialize)]
struct CommandDto {
    id: i64,
    name: String,
    status: String,
    message: Option<String>,
    #[serde(default)]
    body: Value,
}

impl CommandDto {
    fn moves(self) -> Vec<MoveCommand> {
        let state = match self.status.to_ascii_lowercase().as_str() {
            "completed" => CommandState::Completed,
            "failed" | "aborted" | "cancelled" | "orphaned" => {
                CommandState::Ended(self.status.to_ascii_lowercase())
            }
            // Queued, started, or a state a later release adds: the title is
            // still the Arr's to settle.
            _ => CommandState::Running,
        };
        let one = |key: &str| {
            self.body[key].as_i64().map(|item| (item, self.body["destinationPath"].as_str()))
        };
        let bulk = |list: &str, key: &str| -> Vec<(i64, Option<&str>)> {
            let titles = self.body[list].as_array().map(Vec::as_slice).unwrap_or_default();
            titles.iter().filter_map(|title| title[key].as_i64()).map(|item| (item, None)).collect()
        };
        let carried: Vec<(i64, Option<&str>)> = match self.name.as_str() {
            "MoveMovie" => one("movieId").into_iter().collect(),
            "MoveSeries" => one("seriesId").into_iter().collect(),
            "BulkMoveMovie" => bulk("movies", "movieId"),
            "BulkMoveSeries" => bulk("series", "seriesId"),
            _ => Vec::new(),
        };
        carried
            .into_iter()
            .map(|(item, destination)| MoveCommand {
                id: self.id,
                item,
                destination: destination.map(str::to_string),
                state: state.clone(),
                message: self.message.clone(),
            })
            .collect()
    }
}

/// A payload's shape, for a message that says what came back instead.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The last segment of the title's folder, the name the user or the Arr's
/// naming format chose. A title never scanned has no folder yet, and its slug
/// or title names it.
fn folder_name(record: &Value) -> String {
    let current = record["path"].as_str().map(|path| path.trim_end_matches(['/', '\\']));
    current
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty())
        .or_else(|| record["titleSlug"].as_str())
        .or_else(|| record["title"].as_str())
        .unwrap_or("unknown")
        .to_string()
}

fn join_path(root: &str, name: &str) -> String {
    let separator = if root.contains('\\') && !root.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{name}", root.trim_end_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_folder_keeps_its_name() {
        let record =
            json!({ "path": "/tv/standard/Cowboy Bebop (1998)", "titleSlug": "cowboy-bebop" });
        assert_eq!(folder_name(&record), "Cowboy Bebop (1998)");
        assert_eq!(folder_name(&json!({ "path": "/tv/standard/Dark/" })), "Dark");
        assert_eq!(folder_name(&json!({ "path": "D:\\Media\\Akira" })), "Akira");
    }

    #[test]
    fn a_title_never_scanned_is_named_by_its_slug() {
        assert_eq!(folder_name(&json!({ "titleSlug": "cowboy-bebop" })), "cowboy-bebop");
        assert_eq!(folder_name(&json!({ "path": "", "title": "Dark" })), "Dark");
    }

    #[test]
    fn the_root_takes_the_separator_it_is_written_with() {
        assert_eq!(join_path("/tv/anime/", "Dark"), "/tv/anime/Dark");
        assert_eq!(join_path("/tv/anime", "Dark"), "/tv/anime/Dark");
        assert_eq!(join_path("D:\\tv\\anime", "Dark"), "D:\\tv\\anime\\Dark");
    }

    /// Each title a command carries, single or bulk, with the command's
    /// state. Commands that move nothing are left out. The first two are
    /// `/api/v3/command` entries in full, as Radarr and Sonarr list them.
    #[test]
    fn every_title_a_move_command_carries_is_read() {
        let listed: Vec<CommandDto> = serde_json::from_value(json!([
            { "name": "MoveMovie", "commandName": "Move Movie",
              "message": "Moving My Neighbor Totoro (1988)",
              "body": { "movieId": 10, "sourcePath": "/movies/standard/Totoro",
                "destinationPath": "/movies/anime/Totoro", "sendUpdatesToClient": true,
                "updateScheduledTask": true, "completionMessage": "Completed",
                "requiresDiskAccess": true, "isExclusive": false, "isTypeExclusive": false,
                "isLongRunning": false, "name": "MoveMovie", "trigger": "unspecified",
                "suppressMessages": false },
              "priority": "normal", "status": "started", "queued": "2026-10-07T19:02:11Z",
              "started": "2026-10-07T19:02:11Z", "trigger": "unspecified",
              "stateChangeTime": "2026-10-07T19:02:11Z", "sendUpdatesToClient": true,
              "updateScheduledTask": true, "id": 7 },
            { "name": "BulkMoveSeries", "commandName": "Bulk Move Series", "message": "disk full",
              "body": { "series": [{ "seriesId": 20, "sourcePath": "/tv/standard/Dark" },
                  { "seriesId": 21, "sourcePath": "/tv/standard/Mushi-Shi" }],
                "destinationRootFolder": "/tv/anime", "sendUpdatesToClient": true,
                "requiresDiskAccess": true, "name": "BulkMoveSeries", "trigger": "manual" },
              "priority": "normal", "status": "failed", "result": "unsuccessful",
              "queued": "2026-10-07T19:03:00Z", "started": "2026-10-07T19:03:00Z",
              "ended": "2026-10-07T19:03:02Z", "duration": "00:00:01.9934120",
              "exception": "System.IO.IOException: No space left on device",
              "trigger": "manual", "stateChangeTime": "2026-10-07T19:03:00Z",
              "lastExecutionTime": "2026-10-07T19:03:02Z", "id": 8 },
            { "id": 9, "name": "RefreshMovie", "status": "completed", "body": { "movieIds": [10] } },
            { "id": 10, "name": "MoveSeries", "status": "Completed", "body": { "seriesId": 22 } },
        ]))
        .unwrap();
        let moves: Vec<MoveCommand> = listed.into_iter().flat_map(CommandDto::moves).collect();

        let summary: Vec<(i64, i64, Option<&str>, &CommandState)> =
            moves.iter().map(|m| (m.id, m.item, m.destination.as_deref(), &m.state)).collect();
        let failed = CommandState::Ended("failed".into());
        assert_eq!(
            summary,
            [
                (7, 10, Some("/movies/anime/Totoro"), &CommandState::Running),
                (8, 20, None, &failed),
                (8, 21, None, &failed),
                (10, 22, None, &CommandState::Completed),
            ]
        );
        assert_eq!(moves[1].message.as_deref(), Some("disk full"));
    }
}
