<script lang="ts">
  import { KeyRound, Link2, Plus, RefreshCw, Trash2, Wifi } from '../lib/icons';
  import { api } from '../api/client';
  import { formatRelative, formatTimestamp } from '../api/format';
  import type { Instance } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import ActionMenu from '../components/ActionMenu.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { invalidateStatus } from '../lib/status.svelte';

  interface FormState {
    name: string;
    instance_type: 'radarr' | 'sonarr';
    base_url: string;
    api_key: string;
    enabled: boolean;
    sync_interval_minutes: number;
  }

  const blankForm = (): FormState => ({
    name: '',
    instance_type: 'radarr',
    base_url: 'http://localhost:7878',
    api_key: '',
    enabled: true,
    sync_interval_minutes: 15,
  });

  const list = createAsync((signal) => api.getInstances(signal));
  const outcome = createOutcome();
  let busyId = $state<string | null>(null);
  let editing = $state<{ form: FormState; id?: string } | null>(null);

  const instances = $derived(list.data ?? []);

  // Generic over what the call returns, so the message reads the typed payload
  // the client already declares: a cast here is a field rename nobody sees.
  async function act<T>(id: string, fn: () => Promise<T>, describe: (result: T) => string) {
    busyId = id;
    try {
      const result = await fn();
      outcome.succeed(describe(result));
      // Enabling, disabling or removing an instance changes what the shell
      // counts, and it has no other way of hearing about it.
      invalidateStatus();
      await list.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busyId = null;
    }
  }

  // Shown inside the dialog: the page banner sits under a modal that makes
  // the page inert, dimmed at 85 % with a Dismiss nobody can press, so a 400
  // on the URL looked like a Save button that did nothing.
  let formError = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!editing) return;
    formError = null;
    try {
      if (editing.id) await api.updateInstance(editing.id, editing.form);
      else await api.createInstance(editing.form);
      const wasEdit = Boolean(editing.id);
      editing = null;
      outcome.succeed(t(wasEdit ? 'InstanceUpdated' : 'InstanceAdded'));
      invalidateStatus();
      await list.reload();
    } catch (err) {
      formError = describeError(err);
    }
  }

  const syncNow = (instance: Instance) =>
    act(
      instance.id,
      () => api.syncInstance(instance.id),
      (r) => t('SyncResult', { name: instance.name, media: r.media, folders: r.root_folders }),
    );

  const testConnection = (instance: Instance) =>
    act(
      instance.id,
      () => api.testInstance(instance.id),
      (r) => {
        return t('ConnectionOk', {
          name: instance.name,
          version: r.version,
          folders: r.root_folders,
        });
      },
    );

  async function syncAll() {
    busyId = 'all';
    try {
      const all = await api.syncAll();
      // One line per failure: an error is free text, commas included.
      const failed = all.filter((r) => r.error).map((r) => `${r.instance_name}: ${r.error}`);
      const message = t('SyncAllResult', { count: all.length - failed.length });
      if (failed.length === 0) outcome.succeed(message);
      else if (failed.length === all.length) outcome.fail(message, failed);
      else outcome.warn(message, failed);
      invalidateStatus();
      await list.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busyId = null;
    }
  }

  // Said once the clipboard took it. The clipboard exists on secure origins
  // only, and a homelab serves plain http more often than not, so a refusal
  // hands over the URL itself.
  async function copyWebhookUrl(instance: Instance) {
    const url = `${window.location.origin}${instance.webhook_url}`;
    try {
      await navigator.clipboard.writeText(url);
      outcome.succeed(t('WebhookUrlCopied'));
    } catch {
      outcome.fail(t('WebhookUrlCopyFailed', { url }));
    }
  }

  // A dialog opens on its own form, never on the refusal of the one before.
  function startAdd() {
    formError = null;
    editing = { form: blankForm() };
  }

  function startEdit(instance: Instance) {
    formError = null;
    editing = {
      id: instance.id,
      form: {
        name: instance.name,
        instance_type: instance.instance_type,
        base_url: instance.base_url,
        // Blank means "keep the stored key" on the backend.
        api_key: '',
        enabled: instance.enabled,
        sync_interval_minutes: instance.sync_interval_minutes,
      },
    };
  }
</script>

{#snippet wifiIcon()}<Wifi size={14} aria-hidden="true" />{/snippet}
{#snippet linkIcon()}<Link2 size={14} aria-hidden="true" />{/snippet}
{#snippet keyIcon()}<KeyRound size={14} aria-hidden="true" />{/snippet}
{#snippet trashIcon()}<Trash2 size={14} aria-hidden="true" />{/snippet}

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('ArrInstances')}</h1>
      <p class="page-subtitle">{t('InstancesSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button
        class="btn btn-secondary"
        disabled={!instances.some((instance) => instance.enabled) || busyId !== null}
        onclick={() => void syncAll()}
      >
        <RefreshCw size={16} class={busyId === 'all' ? 'spin' : ''} />
        {t('SyncAll')}
      </button>
      <button class="btn btn-primary" onclick={startAdd}>
        <Plus size={16} />
        {t('AddInstance')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={list.error}
    onDismiss={() => (list.error = null)}
    onRetry={() => void list.reload()}
  />
  <OutcomeBanner {outcome} />

  <div class="card">
    <TableRegion label={t('ArrInstances')}>
      <table>
        <caption class="visually-hidden">{t('ArrInstances')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('Type')}</th>
            <th>{t('BaseUrl')}</th>
            <th>{t('ApiKey')}</th>
            <th>{t('LastSync')}</th>
            <th class="w-230">{t('Actions')}</th>
          </tr>
        </thead>
        <tbody>
          {#if list.loading && instances.length === 0}
            <tr><td colspan="6"><Loading /></td></tr>
          {:else if instances.length === 0}
            <tr><td colspan="7"><EmptyState>{t('NoInstanceConfigured')}</EmptyState></td></tr>
          {:else}
            {#each instances as instance (instance.id)}
              <tr class:row-muted={!instance.enabled}>
                <td>
                  <strong>{instance.name}</strong>
                  {#if !instance.enabled}
                    <span class="badge badge-warning ms-2">
                      {t('Disabled')}
                    </span>
                  {/if}
                </td>
                <td>
                  <span class="badge badge-kind kind-{instance.instance_type}">
                    {instance.instance_type}
                  </span>
                </td>
                <td class="mono text-sm">{instance.base_url}</td>
                <td>
                  <!-- Encrypted is a property of the stored value, not an
                       outcome — the last green that meant something other than
                       "this went well". -->
                  <span
                    class="badge {instance.api_key_encrypted
                      ? 'badge-value muted'
                      : 'badge-warning'}"
                    title={t(
                      instance.api_key_encrypted ? 'ApiKeyEncryptedHint' : 'ApiKeyPlaintextHint',
                    )}
                  >
                    {instance.api_key_encrypted ? t('ApiKeyEncrypted') : instance.api_key_masked}
                  </span>
                </td>
                <td class="cell-timestamp" title={instance.last_sync_at ?? undefined}>
                  <!-- Date and outcome read as one fact, so they share a line
                       rather than stacking the cell two rows tall.
                       The date is the last *success*. Both branches used to
                       stamp one column, so a failed attempt refreshed it like a
                       successful one and the cell read "2 minutes ago" beside an
                       error badge, describing data that was two days old. -->
                  <span
                    class="text-muted"
                    title={formatTimestamp(instance.last_sync_at, i18n.language, '')}
                  >
                    {formatRelative(instance.last_sync_at, i18n.language, t('Never'))}
                  </span>
                  {#if instance.last_sync_attempt_at && instance.last_sync_attempt_at !== instance.last_sync_at}
                    <!-- Only when they differ, which is exactly when a later
                         attempt failed — and the useful thing to say is when
                         that was, or the badge looks stale. -->
                    <span
                      class="text-muted text-xs"
                      title={formatTimestamp(instance.last_sync_attempt_at, i18n.language, '')}
                    >
                      {t('LastAttempt', {
                        when: formatRelative(instance.last_sync_attempt_at, i18n.language, ''),
                      })}
                    </span>
                  {/if}
                  {#if instance.last_sync_status}
                    <span
                      class="badge {instance.last_sync_status === 'success'
                        ? 'badge-success'
                        : 'badge-danger'}"
                      title={instance.last_sync_status}
                    >
                      {instance.last_sync_status}
                    </span>
                  {/if}
                </td>
                <td>
                  <div class="row-actions">
                    <!-- Two visible, the rest behind the menu: six buttons
                         across two columns push the row off a 1440px screen. -->
                    <button
                      class="btn btn-secondary btn-sm"
                      disabled={busyId !== null}
                      title={t('SyncNow')}
                      aria-label="{t('SyncNow')} {instance.name}"
                      onclick={() => void syncNow(instance)}
                    >
                      <RefreshCw size={14} class={busyId === instance.id ? 'spin' : ''} />
                    </button>
                    <button
                      class="btn btn-secondary btn-sm"
                      aria-label="{t('Edit')} – {instance.name}"
                      onclick={() => startEdit(instance)}
                    >
                      {t('Edit')}
                    </button>
                    <ActionMenu
                      label="{t('Actions')} – {instance.name}"
                      actions={[
                        {
                          label: t('TestConnection'),
                          icon: wifiIcon,
                          disabled: busyId !== null,
                          onSelect: () => void testConnection(instance),
                        },
                        {
                          label: t('CopyUrl'),
                          icon: linkIcon,
                          disabled: !instance.webhook_url,
                          onSelect: () => void copyWebhookUrl(instance),
                        },
                        {
                          label: t('RotateWebhookToken'),
                          icon: keyIcon,
                          disabled: busyId !== null,
                          onSelect: () =>
                            void act(
                              instance.id,
                              () => api.rotateWebhookToken(instance.id),
                              () => t('WebhookTokenRotated'),
                            ),
                        },
                        {
                          label: t('Delete'),
                          icon: trashIcon,
                          danger: true,
                          disabled: busyId !== null,
                          onSelect: async () => {
                            if (
                              await askConfirmation(
                                t('ConfirmDeleteInstance', { name: instance.name }),
                                'Delete',
                              )
                            ) {
                              void act(
                                instance.id,
                                () => api.deleteInstance(instance.id),
                                () => t('InstanceDeleted'),
                              );
                            }
                          },
                        },
                      ]}
                    />
                  </div>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if editing}
    {@const form = editing.form}
    {@const isEdit = Boolean(editing.id)}
    <Modal
      label={t(isEdit ? 'EditInstance' : 'AddInstance')}
      onClose={() => {
        editing = null;
        formError = null;
      }}
    >
      <div class="modal-header">
        <h2 class="modal-title">{t(isEdit ? 'EditInstance' : 'AddInstance')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => (editing = null)}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
      </div>
      <ErrorBanner message={formError} onDismiss={() => (formError = null)} />
      <!-- `novalidate`: the browser's own bubble renders in the *browser's*
       language whatever `ui_language` says, and fires before the submit
       handler. Nothing is traded away for it — Save is held until the required
       fields are filled, so the constraint is enforced before the press rather
       than complained about after it. -->
      <form novalidate onsubmit={submit}>
        <div class="form-group">
          <label class="form-label" for="instances-name">{t('Name')}</label>
          <input id="instances-name" class="form-input" bind:value={form.name} required />
        </div>
        <div class="form-row">
          <div class="form-group flex-fill-200">
            <label class="form-label" for="instances-type">{t('Type')}</label>
            <select
              id="instances-type"
              class="form-select"
              value={form.instance_type}
              onchange={(event) => {
                const kind = event.currentTarget.value as 'radarr' | 'sonarr';
                form.instance_type = kind;
                form.base_url =
                  kind === 'radarr' ? 'http://localhost:7878' : 'http://localhost:8989';
              }}
            >
              <option value="radarr">Radarr ({t('Movies')})</option>
              <option value="sonarr">Sonarr ({t('Series')})</option>
            </select>
          </div>
          <div class="form-group flex-fixed-160">
            <label class="form-label" for="instances-sync-every-minutes">
              {t('SyncEveryMinutes')}
            </label>
            <input
              id="instances-sync-every-minutes"
              type="number"
              min="1"
              max="1440"
              class="form-input"
              bind:value={form.sync_interval_minutes}
            />
          </div>
        </div>
        <div class="form-group">
          <label class="form-label" for="instances-base-url">{t('BaseUrl')}</label>
          <input
            id="instances-base-url"
            class="form-input"
            placeholder="http://radarr:7878"
            bind:value={form.base_url}
            required
          />
        </div>
        <div class="form-group">
          <label class="form-label" for="instances-api-key">{t('ApiKey')}</label>
          <input
            id="instances-api-key"
            type="password"
            class="form-input"
            placeholder={t(isEdit ? 'ApiKeyKeepHint' : 'ApiKeyWhereHint')}
            bind:value={form.api_key}
            required={!isEdit}
          />
        </div>
        <label class="flex items-center gap-2 text-base">
          <input type="checkbox" bind:checked={form.enabled} />
          {t('EnabledSyncedRouted')}
        </label>
        <div class="flex justify-between mt-4">
          <button type="button" class="btn btn-secondary" onclick={() => (editing = null)}>
            {t('Cancel')}
          </button>
          <button
            type="submit"
            class="btn btn-primary"
            disabled={!form.name.trim() ||
              !form.base_url.trim() ||
              !Number.isInteger(form.sync_interval_minutes) ||
              form.sync_interval_minutes < 1 ||
              (!isEdit && !form.api_key.trim())}>{t(isEdit ? 'Save' : 'AddInstance')}</button
          >
        </div>
      </form>
    </Modal>
  {/if}
</div>
