import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { api } from '../api/client';
import type { RestoreResult } from '../api/types';
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
  const notices: string[] = [];
  renderWithI18n(BackupCard, {
    props: { onError: () => {}, onNotice: (m: string) => notices.push(m) },
    strings: STRINGS,
  });
  return notices;
}

afterEach(() => vi.restoreAllMocks());

describe('restoring a backup', () => {
  it('says nothing extra when the archive carries the master key', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(true));
    const notices = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
    await answerConfirmation();

    await waitFor(() => expect(notices).toContain('Restore staged.'));
  });

  /// An installation whose master key lives in ROUTARR_SECRET_KEY has no key
  /// file, so its archives carry none — and restoring one leaves every sealed
  /// Arr credential unreadable. The manifest is the only thing that knows, and
  /// until now the answer was computed, sent, and dropped here.
  it('warns when it does not, since every stored Arr credential is lost', async () => {
    vi.spyOn(api, 'restoreBackup').mockResolvedValue(result(false));
    const notices = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
    await answerConfirmation();

    await waitFor(() =>
      expect(notices).toContain(
        'Restore staged, but the credentials will have to be entered again.',
      ),
    );
  });

  it('does nothing when the question is declined', async () => {
    const restore = vi.spyOn(api, 'restoreBackup');
    mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore routarr-backup/ }));
    await answerConfirmation(null);

    expect(restore).not.toHaveBeenCalled();
  });
});
