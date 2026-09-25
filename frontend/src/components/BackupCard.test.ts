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

describe('restoring a backup', () => {
  it('says nothing extra when the archive carries the master key', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(true));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
    await answerConfirmation();

    await waitFor(() => expect(outcome.notice).toBe('Restore staged.'));
  });

  /// An installation whose master key lives in ROUTARR_SECRET_KEY has no key
  /// file, so its archives carry none, and restoring one leaves every sealed
  /// Arr credential unreadable. The manifest is the only thing that knows.
  it('warns when it does not, since every stored Arr credential is lost', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(false));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
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

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
    await answerConfirmation(null);
    await new Promise((resolve) => setTimeout(resolve, 0));

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
