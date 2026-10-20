import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { captureDownloads } from '../test/downloads';
import { answerConfirmation } from '../test/confirm';
import { answerProof } from '../test/proof';
import { ApiError, api } from '../api/client';
import type { BackupList, RestoreResult } from '../api/types';
import { createOutcome } from '../lib/outcome.svelte';
import BackupCard from './BackupCard.svelte';

/**
 * The archives on disk, and the one thing about them the operator cannot see
 * for themselves: whether the file carries the master key it needs.
 */

const FILE = {
  name: 'routarr-backup-20260904-101500.zip',
  size_bytes: 1024,
  created_at: '2026-09-04 10:15:00',
  encrypted: false,
  copied: false,
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
  vi.spyOn(api, 'listBackups').mockResolvedValue({
    backups: [FILE],
    retention_count: 7,
    copy: null,
  });
  const outcome = createOutcome();
  renderWithI18n(BackupCard, { props: { outcome } });
  return outcome;
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

/** Fetched with the key, since the archive holds the master key, and saved under its own name. */
it('downloads the archive of its row under its name', async () => {
  vi.spyOn(api, 'listBackups').mockResolvedValue({
    backups: [FILE],
    retention_count: 7,
    copy: null,
  });
  const archive = new Blob(['zip']);
  const fetched = vi.spyOn(api, 'downloadBackup').mockResolvedValue(archive);
  const saved = captureDownloads();
  renderWithI18n(BackupCard, { props: { outcome: createOutcome() } });

  await userEvent.click(await screen.findByRole('button', { name: `Download – ${FILE.name}` }));

  await waitFor(() => expect(saved).toHaveLength(1));
  expect(fetched).toHaveBeenCalledWith(FILE.name);
  expect(saved[0]).toEqual({ name: FILE.name, blob: archive });
});

/**
 * "No backup yet" tells the operator there is nothing to restore. Said of a
 * list that never arrived, it is false at the moment they look for one.
 */
describe('listing the backups', () => {
  it('reports a list that failed to load, rather than showing it empty', async () => {
    vi.spyOn(api, 'listBackups')
      .mockRejectedValueOnce(new ApiError('The backup folder cannot be read', 500, 'internal'))
      .mockResolvedValueOnce({ backups: [FILE], retention_count: 7, copy: null });
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() } });

    expect(await screen.findByRole('alert')).toHaveTextContent('The backup folder cannot be read');
    expect(screen.queryByText('No backup yet.')).toBeNull();

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

    await waitFor(() =>
      expect(outcome.notice).toBe('Restore staged. Restart Routarr to complete it.'),
    );
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
        'Restore staged. It completes at the next start. This archive carries no master key, so the stored Radarr and Sonarr credentials will have to be entered again.',
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

  /**
   * A sealed archive this installation holds no passphrase for: asked, sent,
   * and asked again with the server's reason when it opened nothing.
   */
  it('asks for the passphrase of a sealed archive until one opens it', async () => {
    const sealed = (message: string) => new ApiError(message, 400, 'passphrase_required');
    const restore = vi
      .spyOn(api, 'restoreBackup')
      .mockRejectedValueOnce(sealed('This archive is encrypted.'))
      .mockRejectedValueOnce(sealed('That passphrase does not open this archive.'))
      .mockResolvedValue(result(true));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore – routarr-backup/ }));
    await answerConfirmation();
    expect(await answerProof('the wrong one')).toEqual({ asked: 'passphrase', note: undefined });
    expect(await answerProof('the right one')).toEqual({
      asked: 'passphrase',
      note: 'That passphrase does not open this archive.',
    });

    await waitFor(() =>
      expect(outcome.notice).toBe('Restore staged. Restart Routarr to complete it.'),
    );
    expect(restore.mock.calls.map(([, passphrase]) => passphrase)).toEqual([
      undefined,
      'the wrong one',
      'the right one',
    ]);
  });

  it('stages nothing when the passphrase is not given', async () => {
    const restore = vi
      .spyOn(api, 'restoreBackup')
      .mockRejectedValue(new ApiError('This archive is encrypted.', 400, 'passphrase_required'));
    const outcome = mount();

    await userEvent.click(await screen.findByRole('button', { name: /Restore – routarr-backup/ }));
    await answerConfirmation();
    await answerProof(null);

    await waitFor(() => expect(restore).toHaveBeenCalledTimes(1));
    expect(outcome.error).toBeNull();
    expect(outcome.notice).toBeNull();
  });
});

it('marks an archive sealed with the backup passphrase', async () => {
  vi.spyOn(api, 'listBackups').mockResolvedValue({
    backups: [FILE, { ...FILE, name: 'routarr-backup-20260905-101500.zip.enc', encrypted: true }],
    retention_count: 7,
    copy: null,
  });
  renderWithI18n(BackupCard, { props: { outcome: createOutcome() } });

  const sealed = (await screen.findByText('routarr-backup-20260905-101500.zip.enc')).parentElement!;
  expect(within(sealed).getByText('Encrypted')).toBeTruthy();
  const plain = screen.getByText(FILE.name).parentElement!;
  expect(within(plain).queryByText('Encrypted')).toBeNull();
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

  function mountTwo(after: Promise<BackupList>) {
    const list = vi
      .spyOn(api, 'listBackups')
      .mockResolvedValueOnce({ backups: [FILE, OLDER], retention_count: 7, copy: null })
      .mockReturnValueOnce(after);
    vi.spyOn(api, 'deleteBackup').mockResolvedValue(undefined as never);
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() } });
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
    mountTwo(Promise.resolve({ backups: [OLDER], retention_count: 7, copy: null }));

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
      .mockResolvedValueOnce({ backups: [FILE], retention_count: 7, copy: null })
      .mockResolvedValueOnce({ backups: [], retention_count: 7, copy: null });
    vi.spyOn(api, 'deleteBackup').mockResolvedValue(undefined as never);
    renderWithI18n(BackupCard, { props: { outcome: createOutcome() } });

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    await answerConfirmation();

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Back up now' })),
    );
  });
});

/** The copy folder, read from its tasks: where the copies go, how the last one went. */
describe('the copy folder', () => {
  const COPY = {
    folder: '/mnt/nas/routarr',
    state: 'copied' as const,
    detail: 'Backups copied to /mnt/nas/routarr. Archives copied: 1',
    copied_at: '2026-09-04 10:16:00',
  };

  function mountWith(copy: BackupList['copy'], backups = [{ ...FILE, copied: true }]) {
    vi.spyOn(api, 'listBackups').mockResolvedValue({ backups, retention_count: 7, copy });
    const outcome = createOutcome();
    renderWithI18n(BackupCard, { props: { outcome } });
    return outcome;
  }

  it('names the folder and the last copy, and marks each archive copied', async () => {
    mountWith(COPY);

    expect(await screen.findByText('/mnt/nas/routarr')).toBeTruthy();
    expect(screen.getByText(/^Last copy: /)).toBeTruthy();
    const row = screen.getByText(FILE.name).parentElement!;
    expect(within(row).getByText('Copied')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Start the copy here' })).toBeNull();
  });

  it('says why the last copy failed, and offers nothing a share asleep needs', async () => {
    const failed = 'The copy to /mnt/nas/routarr did not end in time.';
    mountWith({ ...COPY, state: 'failed', detail: failed });

    expect(await screen.findByText(failed)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Start the copy here' })).toBeNull();
  });

  /** Only on purpose: into a share not mounted, a copy fills the local disk. */
  it('starts the copy in a folder without its marker once asked', async () => {
    const adopted = vi.spyOn(api, 'adoptCopyFolder').mockResolvedValue(undefined);
    const outcome = mountWith({ ...COPY, state: 'unmounted', detail: 'No marker.' });

    await userEvent.click(await screen.findByRole('button', { name: 'Start the copy here' }));
    const asked = await answerConfirmation();

    expect(asked).toContain('/mnt/nas/routarr');
    await waitFor(() => expect(adopted).toHaveBeenCalledTimes(1));
    expect(outcome.notice).toBe('The folder is taken as it is. The copy runs now.');
  });

  it('starts nothing when the question is declined', async () => {
    const adopted = vi.spyOn(api, 'adoptCopyFolder');
    mountWith({ ...COPY, state: 'unmounted', detail: 'No marker.' });

    await userEvent.click(await screen.findByRole('button', { name: 'Start the copy here' }));
    await answerConfirmation(null);

    expect(adopted).not.toHaveBeenCalled();
  });

  it('says a copy is running', async () => {
    mountWith({ ...COPY, state: 'running', copied_at: null, detail: null });
    expect(await screen.findByText('Copying now.')).toBeTruthy();
  });

  it('says no copy was made yet', async () => {
    mountWith({ ...COPY, state: 'never', copied_at: null, detail: null }, [FILE]);
    expect(await screen.findByText('No copy yet.')).toBeTruthy();
  });

  /** Deleting an archive deletes its copy: a question saying it is the only copy would lie. */
  it('names the copy a deletion takes with the archive', async () => {
    vi.spyOn(api, 'deleteBackup').mockResolvedValue(undefined as never);
    mountWith(COPY);

    await userEvent.click(await screen.findByRole('button', { name: `Delete – ${FILE.name}` }));
    const asked = await answerConfirmation(null);

    expect(asked).toBe(
      `Delete the backup ${FILE.name} and its copy in /mnt/nas/routarr? Nothing else holds this snapshot.`,
    );
  });
});
