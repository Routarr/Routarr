import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { ApiError, api } from '../api/client';
import type { RestoreResult } from '../api/types';
import { createOutcome } from '../lib/outcome.svelte';
import BackupCard from './BackupCard.svelte';

/**
 * The archives on disk, and the one thing about them the operator cannot see
 * for themselves: whether the file carries the master key it needs.
 */

const STRINGS = {
  Backups: 'Backups',
  BackupNow: 'Back up now',
  RestoreBackup: 'Restore',
  Delete: 'Delete',
  DownloadBackup: 'Download',
  NoBackupsYet: 'No backup yet',
  BackupsHelp: 'Taken on a schedule',
  ConfirmRestore: 'Restore {name}?',
  RestoreStaged: 'Restore staged.',
  RestoreStagedWithoutKey: 'Restore staged, but the credentials will have to be entered again.',
  Retry: 'Retry',
};

const FILE = {
  name: 'routarr-backup-20260904-101500.zip',
  size_bytes: 1024,
  created_at: '2026-09-04 10:15:00',
};

function result(includes_master_key: boolean): RestoreResult {
  return {
    manifest: {
      version: '0.1.0',
      schema: '015_decision_subject',
      created_at: '2026-09-04 10:15:00',
      includes_master_key,
    },
    restart_required: true,
  };
}

function mount() {
  vi.spyOn(api, 'listBackups').mockResolvedValue({ backups: [FILE], retention_count: 7 });
  const outcome = createOutcome();
  renderWithI18n(BackupCard, { props: { outcome }, strings: STRINGS });
  return outcome;
}

afterEach(() => vi.restoreAllMocks());

/**
 * "No backup yet" tells the operator there is nothing to restore. Said of a
 * list that never arrived, it is false at the moment they look for one.
 */
describe('listing the backups', () => {
  it('reports a list that failed to load, rather than showing it empty', async () => {
    vi.spyOn(api, 'listBackups')
      .mockRejectedValueOnce(new ApiError('The backup folder cannot be read', 500, 'internal'))
      .mockResolvedValueOnce({ backups: [FILE], retention_count: 7 });
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() }, strings: STRINGS });

    expect(await screen.findByRole('alert')).toHaveTextContent('The backup folder cannot be read');
    expect(screen.queryByText('No backup yet')).toBeNull();

    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText(FILE.name)).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
  });
});

describe('restoring a backup', () => {
  it('says nothing extra when the archive carries the master key', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(true));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore – routarr-backup/ }));
    await answerConfirmation();

    await waitFor(() => expect(outcome.notice).toBe('Restore staged.'));
  });

  /**
   * An installation whose master key lives in ROUTARR_SECRET_KEY has no key
   * file, so its archives carry none, and restoring one leaves every sealed
   * Arr credential unreadable. The manifest is the only thing that knows.
   */
  it('warns when it does not, since every stored Arr credential is lost', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(false));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore – routarr-backup/ }));
    await answerConfirmation();

    await waitFor(() =>
      expect(outcome.warning).toBe(
        'Restore staged, but the credentials will have to be entered again.',
      ),
    );
    expect(outcome.notice).toBeNull();
  });

  it('does nothing when the question is declined', async () => {
    const restore = vi.spyOn(api, 'restoreBackup');
    const outcome = mount();
    outcome.succeed('Settings saved.');

    await userEvent.click(await screen.findByRole('button', { name: /Restore – routarr-backup/ }));
    await answerConfirmation(null);

    expect(restore).not.toHaveBeenCalled();
    expect(outcome.notice).toBe('Settings saved.');
  });
});

describe('taking a backup', () => {
  it('takes the error of the attempt before off screen once it succeeds', async () => {
    vi.spyOn(api, 'createBackup').mockResolvedValue(FILE);
    const outcome = mount();
    outcome.fail('No space left on the backup volume');

    await userEvent.click(await screen.findByRole('button', { name: 'Back up now' }));

    await waitFor(() => expect(outcome.error).toBeNull());
  });

  it('reports a backup that failed', async () => {
    vi.spyOn(api, 'createBackup').mockRejectedValue(
      new ApiError('No space left on the backup volume', 409, 'conflict'),
    );
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: 'Back up now' }));

    await waitFor(() => expect(outcome.error).toBe('No space left on the backup volume'));
  });
});

/**
 * A deleted archive takes its row away. Swapped for a spinner while the list
 * reloads, the other rows would take the focus with them, so the list stays,
 * and what takes the deleted row's place takes the focus.
 */
describe('deleting a backup', () => {
  const OLDER = { ...FILE, name: 'routarr-backup-20260903-101500.zip' };

  function mountTwo(after: Promise<{ backups: (typeof FILE)[]; retention_count: number }>) {
    const list = vi
      .spyOn(api, 'listBackups')
      .mockResolvedValueOnce({ backups: [FILE, OLDER], retention_count: 7 })
      .mockReturnValueOnce(after);
    vi.spyOn(api, 'deleteBackup').mockResolvedValue(undefined as never);
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() }, strings: STRINGS });
    return list;
  }

  it('deletes nothing when the deletion is cancelled', async () => {
    mountTwo(new Promise(() => {}));

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    await answerConfirmation(null);

    expect(api.deleteBackup).not.toHaveBeenCalled();
  });

  it('deletes the archive whose row was used', async () => {
    mountTwo(new Promise(() => {}));

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${OLDER.name}` }));
    await answerConfirmation();

    expect(api.deleteBackup).toHaveBeenCalledExactlyOnceWith(OLDER.name);
  });

  it('keeps the list on screen while it reloads', async () => {
    const list = mountTwo(new Promise(() => {}));

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    await answerConfirmation();
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));

    expect(screen.getByRole('button', { name: `Delete – ${OLDER.name}` })).toBeTruthy();
  });

  it('hands the focus to the archive that took the place of the deleted one', async () => {
    mountTwo(Promise.resolve({ backups: [OLDER], retention_count: 7 }));

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: `Delete – ${OLDER.name}` }),
      ),
    );
  });

  it('hands the focus to Back up now once no archive is left', async () => {
    vi.spyOn(api, 'listBackups')
      .mockResolvedValueOnce({ backups: [FILE], retention_count: 7 })
      .mockResolvedValueOnce({ backups: [], retention_count: 7 });
    vi.spyOn(api, 'deleteBackup').mockResolvedValue(undefined as never);
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() }, strings: STRINGS });

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Back up now' })),
    );
  });
});
