<script lang="ts">
  import { Archive, Download, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import { createAsync } from '../lib/async.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import type { Outcome } from '../lib/outcome.svelte';
  import { formatBytes, formatTimestamp } from '../api/format';
  import Loading from './Loading.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { downloadBlob } from '../lib/download';

  /**
   * The archives on disk, and what can be done with them.
   *
   * Separate from the settings form because these are actions on existing
   * files, not values to save — mixing them would make "Save" look as though it
   * applied to the list.
   */
  let { outcome }: { outcome: Outcome } = $props();

  const backups = createAsync((signal) => api.listBackups(signal));
  let busy = $state(false);

  async function run(action: () => Promise<void>) {
    busy = true;
    try {
      await action();
      await backups.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busy = false;
    }
  }

  async function download(name: string) {
    // Fetched with the API key rather than linked: the archive carries the
    // master key, so it is never reachable from an unauthenticated URL.
    downloadBlob(await api.downloadBackup(name), name);
  }
</script>

<div class="card">
  <div class="card-header">
    <h2 class="card-title">
      <Archive size={16} />
      {t('Backups')}
    </h2>
  </div>
  <p class="text-muted text-md mb-3">{t('BackupsHelp')}</p>

  <button
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

  {#if backups.loading}
    <Loading />
  {:else if (backups.data?.backups.length ?? 0) === 0}
    <p class="text-muted mt-4">{t('NoBackupsYet')}</p>
  {:else}
    <!-- A gap on the list, not a margin on each row: a margin also lands
           below the last one, inside the card. -->
    <div class="mt-4 flex-col gap-2">
      <!-- `?? []` rather than a non-null assertion: this branch is only
           reached when there is data, but the `{:else}` cannot tell the
           compiler that, and an assertion is a claim nothing rechecks if the
           condition above it ever changes. -->
      {#each backups.data?.backups ?? [] as file (file.name)}
        <div class="flex gap-2 items-center">
          <div class="flex-1">
            <span class="mono text-md">{file.name}</span>
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
            aria-label="{t('DownloadBackup')} {file.name}"
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
            aria-label="{t('RestoreBackup')} {file.name}"
            onclick={() =>
              run(async () => {
                if (
                  !(await askConfirmation(
                    t('ConfirmRestore', { name: file.name }),
                    'RestoreBackup',
                  ))
                )
                  return;
                const result = await api.restoreBackup(file.name);
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
            type="button"
            class="btn btn-secondary btn-sm"
            disabled={busy}
            aria-label="{t('Delete')} {file.name}"
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
            }}
          >
            <Trash2 size={14} />
          </button>
        </div>
      {/each}
    </div>
  {/if}
</div>
