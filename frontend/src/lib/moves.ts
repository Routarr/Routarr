import type { ApplyReport, BatchApplyReport, StopReason } from '../api/types';
import { t } from './i18n.svelte';
import type { Outcome } from './outcome.svelte';

const STOPPED: Record<StopReason, string> = {
  dry_run: 'ApplyStoppedByDryRun',
  cancelled: 'ApplyStoppedOnRequest',
  shutdown: 'ApplyStoppedForShutdown',
  unreachable: 'ApplyStoppedArrUnreachable',
  stalled: 'ApplyStoppedArrStalled',
  refused: 'ApplyStoppedSliceRefused',
  failed: 'ApplyStoppedSliceFailed',
};

/** Whether a run in slices was cut short by a failure rather than stopped. */
export function endedOnFailure(report: Pick<BatchApplyReport, 'stopped'>): boolean {
  return report.stopped === 'refused' || report.stopped === 'failed';
}

/**
 * The outcome of an apply or a revert: what it says beside its counts, then the moves it could
 * not make. Left unfinished, it fails with nothing made and warns with some.
 */
export function reportMoves(
  outcome: Outcome,
  message: string,
  report: Pick<ApplyReport, 'applied' | 'moving' | 'superseded' | 'stopped' | 'errors'>,
  unfinished: boolean,
) {
  const said = [
    ...(report.stopped ? [t(STOPPED[report.stopped])] : []),
    ...(report.moving > 0 ? [t('ApplyStillMoving', { count: report.moving })] : []),
    ...(report.superseded > 0 ? [t('ApplyReplaced', { count: report.superseded })] : []),
    ...report.errors.map((failure) =>
      t('FilterTerm', { label: failure.media_title, value: failure.message }),
    ),
  ];
  const made = report.applied + report.moving;
  if (!unfinished && said.length === 0) outcome.succeed(message);
  else if (made === 0) outcome.fail(message, said);
  else outcome.warn(message, said);
}
