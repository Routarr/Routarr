<script lang="ts">
  import { Archive, Download, Lock, Trash2 } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import type { RestoreResult } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { formatBytes, formatTimestamp } from '../api/format';
  import Loading from './Loading.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { askPassphrase } from '../lib/proof.svelte';
  import { downloadBlob } from '../lib/download';
  import { handFocus } from '../lib/focus';

  /**
   * The archives on disk, and what can be done with them.
   *
   * Separate from the settings form because these are actions on existing
   * files, not values to save: mixing them would make "Save" look as though it
   * applied to the list.
   */
  let { outcome }: { outcome: Outcome } = $props();

  const backups = createAsync((signal) => api.listBackups(signal));
  let busy = $state(false);

  async function run(action: () => Promise<void>) {
    const pressed = document.activeElement as HTMLElement | null;
    busy = true;
    try {
      await action();
      await backups.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = false;
      void handFocus(pressed);
    }
  }

  const deleteId = (index: number) => `backup-delete-${index}`;

  async function download(name: string) {
    downloadBlob(await api.downloadBackup(name), name);
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
      <h2 class="card-title flex items-center gap-2">
        <Archive size={18} aria-hidden="true" />
        {t('Backups')}
      </h2>
      <p class="card-note">{t('BackupsHelp')}</p>
    </div>
  </div>

  <button
    id="backup-now"
    type="button"
    class="btn btn-secondary"
    disabled={busy}
    onclick={() =>
      run(async () => {
        await api.createBackup();
        outcome.clear();
      })}
  >
    <Archive size={16} />
    {t('BackupNow')}
  </button>

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
                // The manifest is the only place that knows. An archive taken
                // while ROUTARR_SECRET_KEY held the master key carries no key
                // file, and restoring it leaves every sealed Arr credential
                // unreadable, visible afterwards only as instances that stopped
                // working.
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
              // Asked, because this one is the irreversible half: restoring is
              // staged and undone by not restarting, while deleting destroys the
              // only copy of a snapshot.
              if (!(await askConfirmation(t('ConfirmDeleteBackup', { name: file.name }), 'Delete')))
                return;
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
</div>
