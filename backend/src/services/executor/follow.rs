//! Whether a move the Arr took was made: both Arrs carry files in a command
//! they queue after answering.

use std::time::Duration;
use tracing::warn;

use super::record::{count_failure, record_confirmed, record_failure, record_held};
use super::{ApplyReport, MoveDirection, PendingMove};
use crate::error::AppResult;
use crate::integrations::adapter::{ArrAdapter, ArrMedia};
use crate::integrations::arr_moves::{CommandState, MoveCommand};
use crate::jobs::Attribution;
use crate::state::AppState;

/// A move with files the Arr took, carried in a command of its own.
pub(super) struct Sent {
    pub(super) mv: PendingMove,
    pub(super) path: String,
}

/// The moves with files one instance took, followed together.
pub(super) struct Following {
    pub(super) adapter: ArrAdapter,
    /// The instance's name, for a reason that names it.
    pub(super) arr: String,
    pub(super) sent: Vec<Sent>,
}

/// Follow the moves of files the Arrs took until each command ends, then read
/// each title back: in its target the move is applied, anywhere else the Arr
/// undid it. One still running at the deadline stays `requested`.
pub(super) async fn follow(
    state: &AppState,
    mut following: Vec<Following>,
    direction: MoveDirection,
    by: &Attribution,
    report: &mut ApplyReport,
) {
    if following.is_empty() {
        return;
    }
    let wait = state.config.move_wait;
    let deadline = tokio::time::Instant::now() + wait;
    let pause = (wait / 20).clamp(Duration::from_millis(50), Duration::from_secs(2));
    let localizer = state.localizer().await;
    loop {
        for instance in &mut following {
            if instance.sent.is_empty() {
                continue;
            }
            let commands = match instance.adapter.move_commands().await {
                Ok(commands) => commands,
                Err(e) => {
                    warn!(arr = %instance.arr, "The Arr's commands could not be read: {e}");
                    continue;
                }
            };
            let mut running = Vec::new();
            for sent in std::mem::take(&mut instance.sent) {
                let command = latest_move(&commands, sent.mv.arr_id, Some(&sent.path));
                let held = match command.map(|c| &c.state) {
                    Some(CommandState::Running) => {
                        running.push(sent);
                        continue;
                    }
                    Some(CommandState::Ended(_)) => None,
                    _ => match instance.adapter.get_media_one(sent.mv.arr_id).await {
                        Ok(held) => Some(held),
                        // Read again at the next round, or by the next sync.
                        Err(_) => {
                            running.push(sent);
                            continue;
                        }
                    },
                };
                let target = &sent.mv.to;
                match undone(&localizer, &instance.arr, command, held.as_ref(), target, UNDONE) {
                    None => {
                        record_confirmed(&state.pool, &sent.mv, direction).await;
                        report.applied += 1;
                    }
                    Some(reason) => {
                        record_failure(state, &sent.mv, &reason, direction, by).await;
                        if let Some(Some(held)) = &held {
                            record_held(&state.pool, &sent.mv, held).await;
                        }
                        count_failure(report, &sent.mv, reason);
                    }
                }
            }
            instance.sent = running;
        }
        let left: usize = following.iter().map(|instance| instance.sent.len()).sum();
        if left == 0 {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            report.moving += left;
            return;
        }
        tokio::time::sleep(pause).await;
    }
}

/// The latest command moving title `item`, to `path` when it names one: a
/// title moved and moved back within minutes has both commands listed.
pub(super) fn latest_move<'a>(
    commands: &'a [MoveCommand],
    item: i64,
    path: Option<&str>,
) -> Option<&'a MoveCommand> {
    commands
        .iter()
        .filter(|command| command.item == item)
        .filter(|command| match (&command.destination, path) {
            (Some(destination), Some(path)) => crate::paths::same(destination, path),
            _ => true,
        })
        .max_by_key(|command| command.id)
}

/// Why a move the Arr took did not happen, `None` when it did: its command
/// failed, or the title is elsewhere than `target`, which `elsewhere` words,
/// or gone. `held` is the title as the Arr holds it now, `Some(None)` once it
/// holds it no more.
pub(super) fn undone(
    localizer: &crate::localization::Localizer,
    arr: &str,
    command: Option<&MoveCommand>,
    held: Option<&Option<ArrMedia>>,
    target: &str,
    elsewhere: &str,
) -> Option<String> {
    if let Some(MoveCommand { state: CommandState::Ended(status), message, .. }) = command {
        return Some(localizer.translate(
            "ErrorMoveCommandEnded",
            &[("arr", arr), ("status", status), ("message", message.as_deref().unwrap_or("-"))],
        ));
    }
    match held {
        Some(None) => Some(localizer.translate("ErrorMoveTitleGone", &[("arr", arr)])),
        Some(Some(media)) => {
            let path = media.path.as_deref().unwrap_or_default();
            (!crate::paths::within(path, target))
                .then(|| localizer.translate(elsewhere, &[("arr", arr), ("path", path)]))
        }
        None => None,
    }
}

/// How a move the Arr took and then undid is worded: its files could not
/// follow. A sync finding a requested move undone cannot tell whether it was
/// ever made, and says so.
pub(super) const UNDONE: &str = "ErrorMoveUndone";

pub(super) const NOT_MADE: &str = "ErrorMoveNotMade";

/// A move an apply left `requested`, as a sync settles it.
#[derive(sqlx::FromRow)]
pub(super) struct RequestedRow {
    decision_id: String,
    media_id: String,
    media_title: String,
    current_root_folder: Option<String>,
    target_root_folder: Option<String>,
    reverting: bool,
    /// `None` once the sync has removed the title the Arr no longer holds.
    arr_id: Option<i64>,
}

/// Settle the moves an apply left `requested` on `instance` once a sync has
/// run: a title the Arr holds in its target with no move running is applied,
/// one elsewhere or gone failed. Each title is read from the Arr rather than
/// from its row, which keeps a move made within the second of the sync's read.
pub async fn settle_requested(
    state: &AppState,
    instance: &crate::models::Instance,
    by: &Attribution,
) -> AppResult<()> {
    let rows: Vec<RequestedRow> = sqlx::query_as(
        "SELECT d.id AS decision_id, d.media_id, d.media_title, d.current_root_folder,
                d.target_root_folder, d.reverted_at IS NOT NULL AS reverting, m.arr_id
           FROM decisions d
           LEFT JOIN media m ON m.id = d.media_id
          WHERE d.instance_id = ? AND d.status = 'requested'",
    )
    .bind(&instance.id)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    // An apply running follows the moves it requested itself.
    let Some(_turn) = state.jobs.try_lock("apply") else {
        return Ok(());
    };
    let adapter = state.adapter(instance)?;
    let commands = adapter.move_commands().await?;
    let localizer = state.localizer().await;

    for row in rows {
        let (direction, from, to) = if row.reverting {
            (MoveDirection::Revert, row.target_root_folder, row.current_root_folder)
        } else {
            (MoveDirection::Forward, row.current_root_folder, row.target_root_folder)
        };
        let Some(to) = to else { continue };
        let mv = PendingMove {
            decision_id: row.decision_id,
            media_id: row.media_id,
            media_title: row.media_title,
            instance_id: instance.id.clone(),
            arr_id: row.arr_id.unwrap_or_default(),
            from,
            to,
        };
        let command = row.arr_id.and_then(|item| latest_move(&commands, item, None));
        let held = match (row.arr_id, command.map(|c| &c.state)) {
            (_, Some(CommandState::Running)) => continue,
            (_, Some(CommandState::Ended(_))) => None,
            (None, _) => Some(None),
            (Some(item), _) => match adapter.get_media_one(item).await {
                Ok(held) => Some(held),
                Err(e) => {
                    warn!(instance = %instance.name, "A requested move could not be read: {e}");
                    continue;
                }
            },
        };
        match undone(&localizer, &instance.name, command, held.as_ref(), &mv.to, NOT_MADE) {
            None => record_confirmed(&state.pool, &mv, direction).await,
            Some(reason) => {
                record_failure(state, &mv, &reason, direction, by).await;
                if let Some(Some(held)) = &held {
                    record_held(&state.pool, &mv, held).await;
                }
            }
        }
    }
    Ok(())
}
