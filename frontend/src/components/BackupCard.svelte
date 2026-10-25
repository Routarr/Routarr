<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import {
    AlertTriangle,
    Archive,
    Check,
    Database,
    Download,
    HardDrive,
    Info,
    Lock,
    Trash2,
  } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import type { BackupCopy, BackupFile, RestoreResult } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { formatBytes, formatTimestamp } from '../api/format';
  import Loading from './Loading.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { askPassphrase, withProof } from '../lib/proof.svelte';
  import { downloadBlob } from '../lib/download';
  import { handFocus } from '../lib/focus';
  import { invalidateStatus } from '../lib/status.svelte';

  /**
   * The archives on disk and what can be done with them, apart from the settings form, whose
   * Save does not apply to them.
   */
  let { outcome }: { outcome: Outcome } = $props();

  const backups = createAsync((signal) => api.listBackups(signal));
  let busy = $state(false);

  /** Stops following, not the backup, which runs on whatever the card does. */
  const leaving = new AbortController();
  $effect(() => () => leaving.abort());

  async function run(action: () => Promise<void>) {
    const pressed = document.activeElement as HTMLElement | null;
    busy = true;
    try {
      await action();
      await backups.reload();
    } catch (err) {
      if (!leaving.signal.aborted) outcome.fail(err);
    } finally {
      busy = false;
      void handFocus(pressed);
    }
  }

  const deleteId = (index: number) => `backup-delete-${index}`;

  /** How the last copy went, when it went well or has not ended. */
  function copyLine(copy: BackupCopy): string {
    if (copy.state === 'running') return t('BackupCopyRunning');
    if (copy.copied_at === null) return t('BackupCopyNever');
    return t('BackupCopyLast', { date: formatTimestamp(copy.copied_at, i18n.language) });
  }

  /** Deleting an archive deletes its copy, which the question says, or it would read as kept. */
  function deleteQuestion(file: BackupFile): string {
    const copy = backups.data?.copy;
    if (copy && file.copied)
      return t('ConfirmDeleteBackupWithCopy', { name: file.name, folder: copy.folder });
    return t('ConfirmDeleteBackup', { name: file.name });
  }

  async function download(name: string) {
    const archive = await withProof((proof) => api.downloadBackup(name, proof), 'archive');
    if (archive) downloadBlob(archive, name);
  }

  /** Taking a folder as it is, asked in the words of what the folder holds. */
  function adoptQuestion(copy: BackupCopy): string {
    if (copy.state === 'foreign') return t('ConfirmBackupCopyForeign', { folder: copy.folder });
    return t('ConfirmBackupCopyStart', { folder: copy.folder });
  }

  /**
   * Stage `name`, asking for its passphrase while the server says it is sealed
   * with one it does not hold. `null` when the question is declined.
   */
  async function restore(name: string): Promise<RestoreResult | null> {
    let passphrase: string | undefined;
    for (;;) {
      try {
        return await api.restoreBackup(name, passphrase);
      } catch (cause) {
        if (!(cause instanceof ApiError && cause.kind === 'passphrase_required')) throw cause;
        const typed = await askPassphrase(passphrase === undefined ? undefined : cause.message);
        if (typed === null) return null;
        passphrase = typed;
      }
    }
  }
</script>

<div class="card">
  <div class="card-header">
    <div>
      <CardTitle card="Backups" class="flex items-center gap-2">
        <Archive size={18} aria-hidden="true" />
        {t('Backups')}
      </CardTitle>
      <p class="card-note">{t('BackupsHelp')}</p>
    </div>
  </div>

  {#if backups.data?.pending_restore}
    {@const pending = backups.data.pending_restore}
    <div class="banner banner-warning items-start">
      <AlertTriangle size={16} aria-hidden="true" />
      <div class="flex-col gap-2">
        <span>
          {t('RestorePending', {
            name: pending.name,
            date: formatTimestamp(pending.staged_at, i18n.language),
          })}
        </span>
        <div>
          <button
            id="restore-withdraw"
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            onclick={() =>
              run(async () => {
                await api.withdrawRestore();
                invalidateStatus();
                outcome.succeed(t('RestoreWithdrawn'));
              })}
          >
            {t('RestoreWithdraw')}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <button
    id="backup-now"
    type="button"
    class="btn btn-secondary"
    disabled={busy}
    onclick={() =>
      run(async () => {
        await api.createBackup({ signal: leaving.signal });
        outcome.clear();
      })}
  >
    <Archive size={16} />
    {t('BackupNow')}
  </button>

  {#if backups.data?.copy}
    {@const copy = backups.data.copy}
    <div class="backup-copy mt-4 flex-col gap-2">
      <div class="flex items-center gap-2">
        <HardDrive size={16} aria-hidden="true" />
        <span class="text-muted">{t('BackupCopyFolder')}</span>
        <span class="mono">{copy.folder}</span>
      </div>
      {#if copy.state === 'failed' || copy.state === 'unmounted' || copy.state === 'foreign'}
        <div class="banner banner-warning items-start">
          <AlertTriangle size={16} aria-hidden="true" />
          <div class="flex-col gap-2">
            <span>{copy.detail}</span>
            {#if copy.copied_at}
              <span class="text-muted text-sm">
                {t('BackupCopyLast', { date: formatTimestamp(copy.copied_at, i18n.language) })}
              </span>
            {/if}
            {#if copy.state === 'unmounted' || copy.state === 'foreign'}
              <div>
                <button
                  id="backup-copy-start"
                  type="button"
                  class="btn btn-secondary btn-sm"
                  disabled={busy}
                  onclick={() =>
                    run(async () => {
                      if (!(await askConfirmation(adoptQuestion(copy), 'BackupCopyStart'))) return;
                      await api.adoptCopyFolder();
                      outcome.succeed(t('BackupCopyStarted'));
                    })}
                >
                  {t('BackupCopyStart')}
                </button>
              </div>
            {/if}
          </div>
        </div>
      {:else}
        <p class="text-muted text-sm">{copyLine(copy)}</p>
      {/if}
      {#if copy.same_disk}
        <div class="banner items-start">
          <Info size={16} aria-hidden="true" />
          <span>{t('BackupCopySameDisk')}</span>
        </div>
      {/if}
    </div>
  {/if}

  {#if backups.error}
    <div class="mt-4">
      <ErrorBanner message={backups.error} onRetry={() => void backups.reload()} />
    </div>
  {/if}
  <!-- The first load only: a reload that swapped the rows for a spinner would
       take the focus with them. -->
  {#if backups.loading && backups.data === null}
    <Loading />
  {:else if backups.data?.backups.length === 0}
    <p class="text-muted mt-4">{t('NoBackupsYet')}</p>
  {:else if backups.data}
    <!-- A gap on the list, not a margin on each row: a margin also lands
           below the last one, inside the card. -->
    <div class="mt-4 flex-col gap-2">
      {#each backups.data.backups as file, index (file.name)}
        <div class="flex gap-2 items-center">
          <div class="flex-1 backup-file">
            <span class="mono text-md">{file.name}</span>
            {#if file.encrypted}
              <span class="badge badge-plain">
                <Lock size={12} aria-hidden="true" />
                {t('BackupEncrypted')}
              </span>
            {/if}
            {#if file.copied}
              <span class="badge badge-plain">
                <Check size={12} aria-hidden="true" />
                {t('BackupCopied')}
              </span>
            {/if}
            <div class="text-muted text-sm">
              {formatTimestamp(file.created_at, i18n.language)} · {formatBytes(
                file.size_bytes,
                i18n.language,
              )}
            </div>
          </div>
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            aria-label="{t('DownloadBackup')} – {file.name}"
            onclick={() =>
              run(async () => {
                await download(file.name);
                outcome.clear();
              })}
          >
            <Download size={14} />
          </button>
          <button
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            aria-label="{t('RestoreBackup')} – {file.name}"
            onclick={() =>
              run(async () => {
                if (
                  !(await askConfirmation(
                    t('ConfirmRestore', { name: file.name }),
                    'RestoreBackup',
                  ))
                )
                  return;
                const result = await restore(file.name);
                if (!result) return;
                invalidateStatus();
                // Only the manifest says whether the archive holds the master key: one taken while
                // ROUTARR_SECRET_KEY held it has none, and every sealed Arr credential is lost.
                if (result.manifest.includes_master_key) outcome.succeed(t('RestoreStaged'));
                else outcome.warn(t('RestoreStagedWithoutKey'));
              })}
          >
            {t('RestoreBackup')}
          </button>
          <button
            id={deleteId(index)}
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            aria-label="{t('Delete')} – {file.name}"
            onclick={async () => {
              // Asked: deleting destroys the only copy of a snapshot, and nothing undoes it.
              if (!(await askConfirmation(deleteQuestion(file), 'Delete'))) return;
              await run(async () => {
                await api.deleteBackup(file.name);
                outcome.clear();
              });
              // The archive takes its row and the pressed Delete with it: the
              // one now in its place takes the focus, else the one before.
              void handFocus(deleteId(index), deleteId(index - 1), 'backup-now');
            }}
          >
            <Trash2 size={14} />
          </button>
        </div>
      {/each}
    </div>
  {/if}

  {#if backups.data?.replaced_database}
    {@const replaced = backups.data.replaced_database}
    <div class="mt-4 flex gap-2 items-center">
      <div class="flex-1 backup-file">
        <span class="flex items-center gap-2">
          <Database size={14} aria-hidden="true" />
          {t('ReplacedDatabase')}
        </span>
        <div class="text-muted text-sm">
          {formatTimestamp(replaced.replaced_at, i18n.language)} · {formatBytes(
            replaced.size_bytes,
            i18n.language,
          )}
        </div>
        <p class="text-muted text-sm">{t('ReplacedDatabaseHelp')}</p>
      </div>
      <button
        id="replaced-database-delete"
        type="button"
        class="btn btn-secondary btn-sm"
        disabled={busy}
        aria-label="{t('Delete')} – {t('ReplacedDatabase')}"
        onclick={async () => {
          if (!(await askConfirmation(t('ConfirmDeleteReplacedDatabase'), 'Delete'))) return;
          await run(async () => {
            await api.deleteReplacedDatabase();
            invalidateStatus();
            outcome.clear();
          });
          void handFocus('backup-now');
        }}
      >
        <Trash2 size={14} />
      </button>
    </div>
  {/if}
</div>
