//! Whether a move the Arr took was made: both Arrs carry files in a command
//! they queue after answering.

use std::time::Duration;
use tracing::warn;

use super::record::{count_failure, record_confirmed, record_failure, record_held};
use super::{ApplyReport, MoveDirection, Moved, PendingMove, Sending, StopReason};
use crate::error::AppResult;
use crate::integrations::adapter::{ArrAdapter, ArrMedia};
use crate::integrations::arr_moves::{CommandState, MoveCommand, under};
use crate::jobs::Attribution;
use crate::localization::Told;
use crate::services::connection::Said;
use crate::state::AppState;

/// A move with files the Arr took, carried in a command of its own.
pub(super) struct Sent {
    pub(super) mv: PendingMove,
    pub(super) path: String,
    /// Where the Arr held the title before the move.
    pub(super) from: Option<String>,
}

impl Sent {
    pub(super) fn moved(&self, instance: &str, status: &'static str) -> Moved {
        let to = (status != "failed").then(|| self.path.clone());
        Moved::of(&self.mv, Some(instance), self.from.clone(), to, status)
    }
}

/// How following one move ended for its run.
pub(super) enum Followed {
    /// Applied or failed, as the Arr's command and the title read back say.
    Settled,
    /// Still in the Arr's hands, the title left `requested` for a later sync to settle.
    Stopped(StopReason),
}

/// An Arr unread for this many times `move_wait` stops the run.
const UNREAD_ROUNDS: u32 = 5;

/// A move still running past this many times `move_wait` stops the run: the Arr's queue is
/// stuck, and a next move would only wait behind it.
const CEILING_ROUNDS: u32 = 360;

/// Follow one move of files until its command ends, then read the title back: in its target the
/// move is applied, anywhere else the Arr undid it. Polled quickly for `move_wait`, then every
/// quarter of it, 15 s at most, since a copy across disks takes minutes per film.
pub(super) async fn follow_one(
    sending: Sending<'_>,
    adapter: &ArrAdapter,
    arr: &str,
    mut sent: Sent,
    report: &mut ApplyReport,
) -> Followed {
    let Sending { state, stop, .. } = sending;
    let wait = state.config.move_wait;
    let quick = (wait / 20).clamp(Duration::from_millis(50), Duration::from_secs(2));
    let slow = (wait / 4).clamp(Duration::from_millis(50), Duration::from_secs(15));
    let started = tokio::time::Instant::now();
    let mut read_at = started;
    loop {
        match adapter.move_commands().await {
            Ok(commands) => {
                read_at = tokio::time::Instant::now();
                match settle(sending, adapter, arr, sent, &commands, report).await {
                    None => return Followed::Settled,
                    Some(running) => sent = running,
                }
            }
            Err(e) => warn!(arr = %arr, "The Arr's commands could not be read: {e}"),
        }
        let stopped = match super::stop_reason(state, stop).await {
            Some(reason) => Some(reason),
            None if read_at.elapsed() >= wait * UNREAD_ROUNDS => Some(StopReason::Unreachable),
            None if started.elapsed() >= wait * CEILING_ROUNDS => Some(StopReason::Stalled),
            None => None,
        };
        if let Some(reason) = stopped {
            report.moving += 1;
            report.note(sent.moved(arr, "moving"));
            return Followed::Stopped(reason);
        }
        tokio::time::sleep(if started.elapsed() < wait { quick } else { slow }).await;
    }
}

/// Record how the move of `sent` ended, as `commands` and the title read back say, or hand it
/// back while it runs or cannot be read.
async fn settle(
    sending: Sending<'_>,
    adapter: &ArrAdapter,
    arr: &str,
    sent: Sent,
    commands: &[MoveCommand],
    report: &mut ApplyReport,
) -> Option<Sent> {
    let Sending { state, direction, by, localizer, .. } = sending;
    let command = latest_move(commands, sent.mv.arr_id, Some(&sent.path));
    let held = match command.map(|c| &c.state) {
        Some(CommandState::Running) => return Some(sent),
        Some(CommandState::Ended(_)) => None,
        _ => match adapter.get_media_one(sent.mv.arr_id).await {
            Ok(held) => Some(held),
            Err(_) => return Some(sent),
        },
    };
    match undone(arr, command, held.as_ref(), &sent.mv.to, UNDONE) {
        None => {
            record_confirmed(&state.pool, &sent.mv, direction).await;
            report.applied += 1;
            report.note(sent.moved(arr, "applied"));
        }
        Some(reason) => {
            let failed = Said::told(&reason, localizer);
            record_failure(state, &sent.mv, &failed, direction, by).await;
            if let Some(Some(held)) = &held {
                record_held(&state.pool, &sent.mv, held).await;
            }
            count_failure(report, &sent.mv, &failed);
            report.note(sent.moved(arr, "failed"));
        }
    }
    None
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

/// Why a move the Arr took did not happen, `None` when it did: its command failed, or the title
/// is gone or elsewhere than `target`, worded by `elsewhere`. `held` is the title as the Arr
/// holds it, `Some(None)` once it holds it no more.
pub(super) fn undone(
    arr: &str,
    command: Option<&MoveCommand>,
    held: Option<&Option<ArrMedia>>,
    target: &str,
    elsewhere: &str,
) -> Option<Told> {
    if let Some(MoveCommand { state: CommandState::Ended(status), message, .. }) = command {
        let ended = Told::new("ErrorMoveCommandEnded").with("arr", arr).with("status", status);
        return Some(match message.as_deref().map(str::trim).filter(|said| !said.is_empty()) {
            Some(said) => ended.with("message", said),
            None => Told { key: "ErrorMoveCommandEndedSilent".into(), ..ended },
        });
    }
    match held {
        Some(None) => Some(Told::new("ErrorMoveTitleGone").with("arr", arr)),
        Some(Some(media)) => {
            let path = media.path.as_deref().unwrap_or_default();
            (!crate::paths::within(path, target))
                .then(|| Told::new(elsewhere).with("arr", arr).with("path", path))
        }
        None => None,
    }
}

/// How a move the Arr took and then undid is worded: its files could not follow.
pub(super) const UNDONE: &str = "ErrorMoveUndone";

/// How a sync words a requested move it finds undone: it cannot tell whether it was ever made.
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
    current_path: Option<String>,
    media_type: Option<String>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
}

/// Settle the moves an apply left `requested` on `instance` after a sync: one in its target with
/// no move running is applied, one elsewhere or gone failed. Each title is read from the Arr
/// rather than from its row, which keeps a move made within the second of the sync's read.
pub async fn settle_requested(
    state: &AppState,
    instance: &crate::models::Instance,
    by: &Attribution,
) -> AppResult<()> {
    let rows: Vec<RequestedRow> = sqlx::query_as(
        "SELECT d.id AS decision_id, d.media_id, d.media_title, d.current_root_folder,
                d.target_root_folder, d.reverted_at IS NOT NULL AS reverting, m.arr_id,
                m.current_path, m.media_type, m.tmdb_id, m.tvdb_id, m.imdb_id
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
    let mut settled = Vec::new();

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
            media_type: row.media_type.unwrap_or_default(),
            tmdb_id: row.tmdb_id,
            tvdb_id: row.tvdb_id,
            imdb_id: row.imdb_id,
        };
        // The row holds the path the move wrote, and the title left its folder under `from`.
        let to_path = row.current_path;
        let from_path =
            mv.from.as_deref().zip(to_path.as_deref()).map(|(root, at)| under(root, at));
        let named = Some(instance.name.as_str());
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
        match undone(&instance.name, command, held.as_ref(), &mv.to, NOT_MADE) {
            None => {
                record_confirmed(&state.pool, &mv, direction).await;
                settled.push(Moved::of(&mv, named, from_path, to_path, "applied"));
            }
            Some(reason) => {
                record_failure(state, &mv, &Said::told(&reason, &localizer), direction, by).await;
                if let Some(Some(held)) = &held {
                    record_held(&state.pool, &mv, held).await;
                }
                settled.push(Moved::of(&mv, named, from_path, None, "failed"));
            }
        }
    }
    if !settled.is_empty() {
        let total = settled.len();
        settled.truncate(super::MOVED_LISTED);
        crate::services::notify::send_later(
            state,
            crate::services::notify::Event::MovesSettled { total, moves: settled },
        );
    }
    Ok(())
}
