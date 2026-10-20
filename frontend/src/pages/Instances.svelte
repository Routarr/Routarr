<script lang="ts">
  import {
    CheckCircle2,
    ExternalLink,
    KeyRound,
    Link2,
    Plus,
    RefreshCw,
    Trash2,
    Wifi,
  } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import {
    failureDetail,
    formatRelative,
    formatTimestamp,
    inClearPastHome,
    withoutCredentials,
  } from '../api/format';
  import type { Instance, WebhookCheck } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { takeQueryFlag } from '../api/onboarding';
  import { i18n, t } from '../lib/i18n.svelte';
  import { handFocus, type FocusTarget } from '../lib/focus';
  import ActionMenu from '../components/ActionMenu.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { holdUnsaved } from '../lib/unsaved.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { poll } from '../lib/poll.svelte';
  import { access } from '../lib/access.svelte';

  interface FormState {
    name: string;
    instance_type: 'radarr' | 'sonarr';
    base_url: string;
    api_key: string;
    enabled: boolean;
    sync_interval_minutes: number;
    auto_apply: boolean;
  }

  // A placeholder, never a value: a guessed address gets saved unnoticed, and `localhost` is
  // Routarr's own container in Docker.
  const EXAMPLE_URL = { radarr: 'http://radarr:7878', sonarr: 'http://sonarr:8989' } as const;
  const SERVICE = { radarr: 'Radarr', sonarr: 'Sonarr' } as const;

  // Outside these bounds the backend clamps to a figure nobody typed, so Save waits instead.
  const MAX_INTERVAL_MINUTES = 1440;
  const intervalFits = (minutes: number) =>
    Number.isInteger(minutes) && minutes >= 1 && minutes <= MAX_INTERVAL_MINUTES;

  const blankForm = (): FormState => ({
    name: '',
    instance_type: 'radarr',
    base_url: '',
    api_key: '',
    enabled: true,
    sync_interval_minutes: 15,
    auto_apply: true,
  });

  const list = createAsync((signal) => api.getInstances(signal));
  const outcome = createOutcome();
  let busyId = $state<string | null>(null);
  // Closing the screen stops waiting for a sync, which goes on and is read on return.
  const leaving = new AbortController();
  $effect(() => () => {
    leaving.abort();
    stopTrying();
  });
  // `opened` is the form as it opened, as text: whether closing drops any work.
  let editing = $state<{
    form: FormState;
    opened: string;
    id?: string;
    webhook?: Instance;
  } | null>(null);
  const formChanged = $derived(editing !== null && JSON.stringify(editing.form) !== editing.opened);
  holdUnsaved(
    () => formChanged,
    () => t('ConfirmDiscardInstance'),
  );

  const instances = $derived(list.data ?? []);

  // A deleted instance takes its row and its menu with it: the menu of the row
  // now in its place takes the focus, else the one before, else the table.
  const menuId = (index: number) => `instances-menu-${index}`;

  // Generic so `describe` reads the payload the client types: a cast hides a field rename.
  async function act<T>(
    id: string,
    fn: () => Promise<T>,
    describe: (result: T) => string,
    focus: FocusTarget[] = [],
  ) {
    const pressed = document.activeElement as HTMLElement | null;
    busyId = id;
    try {
      const result = await fn();
      outcome.succeed(describe(result));
      // The shell counts the instances and hears of a change no other way.
      invalidateStatus();
      await list.reload();
    } catch (err) {
      outcome.fail(err);
    } finally {
      busyId = null;
      void handFocus(pressed, ...focus);
    }
  }

  // Shown inside the dialog: the page banner sits under the modal, inert, and a refused URL
  // would look like a Save button that did nothing.
  let formError = $state<string | null>(null);

  // The try's answer, kept with the values it answered for: once one changes, it would vouch
  // for values nobody tried.
  let probe = $state<{
    tried: string;
    ok: boolean;
    text: string;
    warning?: string | null;
    webhook?: WebhookCheck | null;
  } | null>(null);
  let probing = $state(false);
  const typed = $derived(
    editing
      ? [editing.form.instance_type, editing.form.base_url, editing.form.api_key].join('\n')
      : '',
  );
  const probed = $derived(probe && probe.tried === typed ? probe : null);
  // One refusal at a time, the try's or the save's, whichever came last.
  const refused = $derived(formError ?? (probed && !probed.ok ? probed.text : null));

  // A try can wait a full connect timeout, and the dialog it was for may be gone by then.
  let trying: AbortController | null = null;
  function stopTrying() {
    trying?.abort();
    trying = null;
    probing = false;
  }

  async function tryConnection() {
    if (!editing) return;
    const { form, id } = editing;
    completeScheme(form);
    const tried = typed;
    stopTrying();
    const attempt = new AbortController();
    trying = attempt;
    probing = true;
    probe = null;
    formError = null;
    try {
      const answer = await api.probeInstance(
        { instance_type: form.instance_type, base_url: form.base_url, api_key: form.api_key, id },
        attempt.signal,
      );
      if (attempt.signal.aborted) return;
      probe = {
        tried,
        ok: true,
        text: t('ConnectionOk', {
          name: answer.app_name ?? SERVICE[form.instance_type],
          version: answer.version,
          folders: answer.root_folders,
        }),
        warning: answer.warning,
        webhook: answer.webhook,
      };
    } catch (err) {
      if (attempt.signal.aborted) return;
      probe = { tried, ok: false, text: describeError(err) };
    } finally {
      if (trying === attempt) {
        trying = null;
        probing = false;
      }
    }
  }

  // Copied from a browser bar or typed from memory, an address often lacks its
  // scheme, which the backend refuses, and a phone keyboard capitalises it.
  function completeScheme(form: FormState) {
    const address = form.base_url.trim();
    const scheme = /^(https?):\/\//i.exec(address);
    if (!address) form.base_url = address;
    else if (scheme)
      form.base_url = `${scheme[1]!.toLowerCase()}://${address.slice(scheme[0].length)}`;
    else form.base_url = `http://${address}`;
  }

  /**
   * Only an address the reader's browser can open is a link. A single-label
   * host, as the container name the form recommends in Docker, resolves
   * inside Routarr's network alone, and nothing but http runs from a click.
   */
  const opensInBrowser = (address: string) => {
    const host = /^https?:\/\/(\[[^\]]+\]|[^/:?#]+)/i.exec(address)?.[1] ?? '';
    return host.startsWith('[') || host.includes('.');
  };

  // A second press in flight would add the same Arr twice: names are not unique.
  let saving = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!editing || saving) return;
    const pressed = document.activeElement as HTMLElement | null;
    completeScheme(editing.form);
    formError = null;
    let saved: Instance;
    const wasEdit = Boolean(editing.id);
    saving = true;
    try {
      saved = editing.id
        ? await api.updateInstance(editing.id, editing.form)
        : await api.createInstance(editing.form);
    } catch (err) {
      probe = null;
      formError = describeError(err);
      void handFocus(pressed);
      return;
    } finally {
      saving = false;
    }
    editing = null;
    const firstSync = saved.enabled && !saved.last_sync_at;
    outcome.succeed(
      firstSync
        ? t('InstanceFirstSync', { name: saved.name })
        : t(wasEdit ? 'InstanceUpdated' : 'InstanceAdded'),
    );
    invalidateStatus();
    await list.reload();
    if (firstSync) await syncFirstTime(saved);
  }

  // Left to the scheduler, a new instance waits up to a whole pass with no library, and the
  // guide with it. Synced on save, a wrong address or key shows while the form is fresh in
  // mind. Saved and not synced is a partial result, not a failure.
  async function syncFirstTime(saved: Instance) {
    busyId = saved.id;
    try {
      const report = await api.syncInstance(saved.id, { signal: leaving.signal });
      outcome.succeed(
        t('SyncResult', {
          name: saved.name,
          media: report.media,
          folders: report.root_folders,
        }),
      );
    } catch (err) {
      // A large library outlasts the request, and the server finishes the sync: the banner
      // stays true, and the list is followed until the sync ends.
      if (err instanceof ApiError && err.kind === 'timeout') {
        following = saved;
        return;
      }
      outcome.warn(t('InstanceFirstSyncFailed', { name: saved.name }), [describeError(err)]);
    } finally {
      busyId = null;
    }
    invalidateStatus();
    await list.reload();
  }

  // The first sync the browser stopped waiting for, until the instance records its attempt.
  let following = $state<Instance | null>(null);
  async function followFirstSync() {
    const awaited = following;
    if (!awaited) return;
    await list.reload();
    const now = list.data?.find((each) => each.id === awaited.id);
    if (!now?.last_sync_attempt_at || following !== awaited) return;
    following = null;
    if (now.last_sync_status === 'success') {
      outcome.succeed(t('InstanceFirstSyncFinished', { name: awaited.name }));
    } else {
      outcome.warn(t('InstanceFirstSyncFailed', { name: awaited.name }), [
        now.last_sync_status ?? '',
      ]);
    }
    invalidateStatus();
  }
  poll(
    () => void followFirstSync(),
    () => 3000,
    () => following !== null,
  );

  const syncNow = (instance: Instance) =>
    act(
      instance.id,
      () => api.syncInstance(instance.id, { signal: leaving.signal }),
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
    const pressed = document.activeElement as HTMLElement | null;
    busyId = 'all';
    try {
      const all = await api.syncAll({ signal: leaving.signal });
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
      void handFocus(pressed);
    }
  }

  const webhookAddress = (path: string) => `${window.location.origin}${path}`;

  // The clipboard exists on secure origins only, and a homelab often serves plain http, so a
  // refusal hands over the URL itself.
  async function copyWebhookUrl(instance: Instance) {
    const url = webhookAddress(instance.webhook_url ?? '');
    try {
      await navigator.clipboard.writeText(url);
      outcome.succeed(t('WebhookUrlCopied'));
    } catch {
      outcome.fail(t('WebhookUrlCopyFailed', { url }));
    }
  }

  // A dialog opens on its own form, never on the refusal or the try of the one before.
  function startAdd() {
    stopTrying();
    formError = null;
    probe = null;
    const form = blankForm();
    editing = { form, opened: JSON.stringify(form) };
  }
  /** Escape, the close button and Cancel: a changed form is asked about first. */
  async function close() {
    if (formChanged && !(await askConfirmation(t('ConfirmDiscardInstance'), 'DiscardChanges'))) {
      return;
    }
    stopTrying();
    editing = null;
    formError = null;
  }
  // The guide's "Add an instance" lands here with the dialog open.
  if (takeQueryFlag('add') && !access.readOnly) startAdd();

  function startEdit(instance: Instance) {
    stopTrying();
    formError = null;
    probe = null;
    const form: FormState = {
      name: instance.name,
      instance_type: instance.instance_type,
      base_url: instance.base_url,
      // Blank means "keep the stored key" on the backend.
      api_key: '',
      enabled: instance.enabled,
      sync_interval_minutes: instance.sync_interval_minutes,
      auto_apply: instance.auto_apply,
    };
    editing = { id: instance.id, webhook: instance, form, opened: JSON.stringify(form) };
  }
</script>

{#snippet wifiIcon()}<Wifi size={14} aria-hidden="true" />{/snippet}
{#snippet linkIcon()}<Link2 size={14} aria-hidden="true" />{/snippet}
{#snippet keyIcon()}<KeyRound size={14} aria-hidden="true" />{/snippet}
{#snippet trashIcon()}<Trash2 size={14} aria-hidden="true" />{/snippet}

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Instances')}</h1>
      <p class="page-subtitle">{t('InstancesSubtitle')}</p>
    </div>
    {#if !access.readOnly}
      <div class="flex gap-2">
        <button
          class="btn btn-secondary"
          disabled={!instances.some((instance) => instance.enabled) || busyId !== null}
          onclick={() => void syncAll()}
        >
          <RefreshCw size={16} class={busyId === 'all' ? 'spin' : ''} />
          {t('SyncAll')}
        </button>
        <button id="instances-add" class="btn btn-primary" onclick={startAdd}>
          <Plus size={16} />
          {t('AddInstance')}
        </button>
      </div>
    {/if}
  </div>

  <ErrorBanner
    message={list.error}
    onDismiss={() => (list.error = null)}
    onRetry={() => void list.reload()}
  />
  <OutcomeBanner {outcome} />
  <GuideStepBanner step="instance" />

  <div class="card">
    <TableRegion label={t('ArrInstances')} id="instances-table">
      <table>
        <caption class="visually-hidden">{t('ArrInstances')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('Type')}</th>
            <th>{t('BaseUrl')}</th>
            <th>{t('ApiKey')}</th>
            <th>{t('LastSync')}</th>
            <th>{t('LastWebhook')}</th>
            <th class="w-230"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if list.loading && instances.length === 0}
            <TableSkeleton columns={7} />
          {:else if instances.length === 0 && !list.error}
            <tr><td colspan="7"><EmptyState>{t('NoInstanceConfigured')}</EmptyState></td></tr>
          {:else}
            {#each instances as instance, index (instance.id)}
              {@const address = withoutCredentials(instance.base_url)}
              <tr class:row-muted={!instance.enabled}>
                <td>
                  <strong>{instance.name}</strong>
                  {#if !instance.enabled}
                    <span class="badge badge-warning ms-2">
                      {t('InstanceDisabled')}
                    </span>
                  {/if}
                  {#if !instance.auto_apply}
                    <span class="badge badge-info ms-2">{t('InstanceProposalsOnly')}</span>
                  {/if}
                </td>
                <td>
                  <span class="badge badge-kind kind-{instance.instance_type}">
                    {instance.instance_type}
                  </span>
                </td>
                <td class="mono text-sm">
                  {#if opensInBrowser(address)}
                    <a
                      class="instance-link"
                      href={address}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {instance.base_url}
                      <ExternalLink size={12} aria-hidden="true" />
                      <span class="visually-hidden">({t('OpensInNewTab')})</span>
                    </a>
                  {:else}
                    {instance.base_url}
                  {/if}
                </td>
                <td>
                  {#if instance.api_key_set}
                    <!-- Not green: encrypted is a property of the stored value, and green
                         means an outcome that went well. -->
                    <span class="badge badge-value muted" title={t('ApiKeyEncryptedHint')}>
                      {t('ApiKeyEncrypted')}
                    </span>
                  {:else}
                    <span class="badge badge-warning">{t('ApiKeyMissing')}</span>
                    <span class="status-reason">{t('ApiKeyMissingHint')}</span>
                  {/if}
                </td>
                <td class="cell-timestamp" title={instance.last_sync_at ?? undefined}>
                  <!-- The date is the last *success*: a failed attempt stamping it would read
                       "2 minutes ago" beside an error badge, about data two days old. -->
                  <span
                    class="text-muted"
                    title={formatTimestamp(instance.last_sync_at, i18n.language, '')}
                  >
                    {formatRelative(instance.last_sync_at, i18n.language, t('Never'))}
                  </span>
                  {#if instance.last_sync_attempt_at && instance.last_sync_attempt_at !== instance.last_sync_at}
                    <!-- They differ only when a later attempt failed, whose date keeps the badge
                         from looking stale. -->
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
                    >
                      {t(
                        instance.last_sync_status === 'success' ? 'StatusSuccess' : 'StatusFailed',
                      )}
                    </span>
                    {#if instance.last_sync_status !== 'success'}
                      <span class="status-reason">{failureDetail(instance.last_sync_status)}</span>
                    {/if}
                  {/if}
                </td>
                <td class="cell-timestamp">
                  <span
                    class="text-muted"
                    title={formatTimestamp(instance.last_webhook_at, i18n.language, '')}
                  >
                    {formatRelative(instance.last_webhook_at, i18n.language, t('Never'))}
                  </span>
                  {#if instance.last_webhook_event}
                    <span class="text-muted text-xs">{instance.last_webhook_event}</span>
                  {/if}
                  <!-- The badge alone: what to change takes sentences, which the form holds. -->
                  {#if instance.webhook_check?.state === 'misconfigured'}
                    <span class="badge badge-warning">{t('WebhookMisconfigured')}</span>
                  {:else if instance.webhook_check?.state === 'absent' && !instance.last_webhook_at}
                    <span class="badge badge-info">{t('WebhookNotSetUp')}</span>
                  {/if}
                </td>
                <td>
                  {#if !access.readOnly}
                    <div class="row-actions">
                      <!-- Two visible, the rest behind the menu: every action as a button pushes
                           the row off a desktop screen. -->
                      <button
                        class="btn btn-secondary btn-sm"
                        disabled={busyId !== null || !instance.enabled}
                        title={instance.enabled
                          ? t('SyncNow')
                          : t('ErrorInstanceSyncDisabled', { name: instance.name })}
                        aria-label="{t('SyncNow')} – {instance.name}"
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
                        id={menuId(index)}
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
                            // The Arr's webhook stops at once, until the new
                            // address is pasted there: asked, as a deletion is.
                            onSelect: async () => {
                              if (
                                await askConfirmation(
                                  t('ConfirmRotateWebhookToken', { name: instance.name }),
                                  'RotateWebhookToken',
                                )
                              ) {
                                void act(
                                  instance.id,
                                  () => api.rotateWebhookToken(instance.id),
                                  () => t('WebhookTokenRotated'),
                                );
                              }
                            },
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
                                  [menuId(index), menuId(index - 1), 'instances-table'],
                                );
                              }
                            },
                          },
                        ]}
                      />
                    </div>
                  {/if}
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
    {@const intervalOk = intervalFits(form.sync_interval_minutes)}
    <Modal
      label={t(isEdit ? 'EditInstance' : 'AddInstance')}
      onClose={() => void close()}
      initialFocus="instances-name"
      returnFocus="instances-add"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t(isEdit ? 'EditInstance' : 'AddInstance')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => void close()}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
      </div>
      <ErrorBanner
        message={refused}
        onDismiss={() => {
          formError = null;
          probe = null;
        }}
      />
      <!-- `novalidate`: the browser's bubble speaks the browser's language, not `ui_language`.
           Save is held until the required fields are filled instead. -->
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
                form.instance_type = event.currentTarget.value as 'radarr' | 'sonarr';
              }}
            >
              <option value="radarr">Radarr ({t('Movies')})</option>
              <option value="sonarr">Sonarr ({t('Series')})</option>
            </select>
          </div>
          <div class="form-group flex-fit">
            <label class="form-label" for="instances-sync-every-minutes">
              {t('SyncEveryMinutes')}
            </label>
            <input
              id="instances-sync-every-minutes"
              type="number"
              min="1"
              max={MAX_INTERVAL_MINUTES}
              class="form-input"
              aria-invalid={intervalOk ? undefined : 'true'}
              aria-describedby={intervalOk ? undefined : 'instances-sync-every-minutes-range'}
              bind:value={form.sync_interval_minutes}
            />
          </div>
        </div>
        <!-- Under the row rather than in the interval's column: the two fields
             share a bottom edge, which a line under one of them would break. -->
        {#if !intervalOk}
          <p id="instances-sync-every-minutes-range" class="field-error">
            {t('RangeBetween', { min: 1, max: MAX_INTERVAL_MINUTES })}
          </p>
        {/if}
        <div class="form-group">
          <label class="form-label" for="instances-base-url">{t('BaseUrl')}</label>
          <input
            id="instances-base-url"
            class="form-input"
            placeholder={EXAMPLE_URL[form.instance_type]}
            aria-describedby={inClearPastHome(form.base_url)
              ? 'instances-base-url-help instances-base-url-in-clear'
              : 'instances-base-url-help'}
            bind:value={form.base_url}
            onblur={() => completeScheme(form)}
            autocapitalize="off"
            spellcheck="false"
            required
          />
          <p id="instances-base-url-help" class="text-muted text-sm mt-1">
            {t('InstanceUrlHelp', { service: SERVICE[form.instance_type] })}
          </p>
          {#if inClearPastHome(form.base_url)}
            <p id="instances-base-url-in-clear" class="text-warning text-sm mt-1">
              {t('InstanceUrlInClear')}
            </p>
          {/if}
        </div>
        <div class="form-group">
          <label class="form-label" for="instances-api-key">{t('ApiKey')}</label>
          <input
            id="instances-api-key"
            type="password"
            class="form-input"
            placeholder={isEdit ? t('ApiKeyKeepHint') : undefined}
            aria-describedby="instances-api-key-help"
            bind:value={form.api_key}
            required={!isEdit}
          />
          <p id="instances-api-key-help" class="text-muted text-sm mt-1">
            {t('InstanceKeyHelp', { service: SERVICE[form.instance_type] })}
          </p>
        </div>
        {#if editing.webhook?.webhook_path && editing.webhook.webhook_url}
          <!-- The try's reading of the connection is the newer one while it stands. -->
          {@const check = probed?.webhook ?? editing.webhook.webhook_check}
          <div class="webhook-status">
            <p>
              {#if editing.webhook.last_webhook_at}
                {t('WebhookLastDelivery', {
                  event: editing.webhook.last_webhook_event ?? '',
                  when: formatRelative(editing.webhook.last_webhook_at, i18n.language, ''),
                })}
              {:else}
                {t('WebhookNoDeliveryYet', { service: SERVICE[form.instance_type] })}
              {/if}
            </p>
            {#if check}
              {#if check.state === 'ok'}
                <p class="text-success">
                  {t('WebhookCheckOk', { instance: editing.webhook.name })}
                </p>
              {:else if check.state === 'absent'}
                <p>{t('WebhookCheckAbsent', { instance: editing.webhook.name })}</p>
              {/if}
              {#each check.problems as problem (problem)}
                <p class="text-warning">{problem}</p>
              {/each}
              {#each check.notes as note (note)}
                <p class="text-muted">{note}</p>
              {/each}
              <p class="text-muted">
                {t('WebhookCheckedAt', {
                  when: formatRelative(check.checked_at, i18n.language, ''),
                })}
              </p>
            {/if}
          </div>
          <!-- Here and not only behind Copy: on a plain http origin the clipboard is closed. -->
          <div class="form-group">
            <label class="form-label" for="instances-webhook-path">{t('WebhookUrl')}</label>
            <input
              id="instances-webhook-path"
              class="form-input mono"
              value={webhookAddress(editing.webhook.webhook_path)}
              readonly
              aria-describedby="instances-webhook-path-help"
            />
            <p id="instances-webhook-path-help" class="text-muted text-sm mt-1">
              {t('WebhookUrlHelp', { service: SERVICE[form.instance_type] })}
              {t('WebhookTriggers', {
                triggers: editing.webhook.webhook_triggers.join(t('ListSeparator')),
              })}
            </p>
          </div>
          <div class="form-group">
            <label class="form-label" for="instances-webhook-token">{t('WebhookToken')}</label>
            <input
              id="instances-webhook-token"
              class="form-input mono"
              value={editing.webhook.webhook_token ?? ''}
              readonly
            />
          </div>
          <div class="form-group">
            <label class="form-label" for="instances-webhook-url">{t('WebhookUrlWithToken')}</label>
            <input
              id="instances-webhook-url"
              class="form-input mono"
              value={webhookAddress(editing.webhook.webhook_url)}
              readonly
              aria-describedby="instances-webhook-url-help"
            />
            <p id="instances-webhook-url-help" class="text-muted text-sm mt-1">
              {t('WebhookUrlWithTokenHelp')}
            </p>
          </div>
        {/if}
        <!-- Apart, so each box keeps the 24px target WCAG 2.2 asks of a control. -->
        <div class="flex flex-col gap-2">
          <label class="flex items-center gap-2 text-base">
            <input type="checkbox" bind:checked={form.enabled} />
            {t('EnabledSyncedRouted')}
          </label>
          <div>
            <label class="flex items-center gap-2 text-base">
              <input
                type="checkbox"
                bind:checked={form.auto_apply}
                aria-describedby="instances-auto-apply-help"
              />
              {t('InstanceAutoApply')}
            </label>
            <p id="instances-auto-apply-help" class="text-muted text-sm mt-1">
              {t('InstanceAutoApplyHelp')}
            </p>
          </div>
        </div>
        <!-- Mounted before the answer, so filling it is a change a screen reader reports. -->
        <div role="status">
          {#if probed?.ok}
            <p class="probe-ok">
              <CheckCircle2 size={16} aria-hidden="true" />
              {probed.text}
            </p>
            {#if probed.warning}
              <p class="text-warning text-sm">{probed.warning}</p>
            {/if}
          {/if}
        </div>
        <div class="dialog-actions">
          <button type="button" class="btn btn-secondary" onclick={() => void close()}>
            {t('Cancel')}
          </button>
          <div class="flex flex-wrap gap-2">
            <!-- Never held while a try runs: a focused button turning disabled drops the focus,
                 and a second press starts the try over. -->
            <button
              type="button"
              class="btn btn-secondary"
              disabled={!form.base_url.trim() || (!isEdit && !form.api_key.trim())}
              onclick={() => void tryConnection()}
            >
              <Wifi size={16} class={probing ? 'spin' : ''} aria-hidden="true" />
              {t('TestConnection')}
            </button>
            <button
              type="submit"
              class="btn btn-primary"
              disabled={saving ||
                !form.name.trim() ||
                !form.base_url.trim() ||
                !intervalOk ||
                (!isEdit && !form.api_key.trim())}
              >{saving ? t('Saving') : t(isEdit ? 'Save' : 'AddInstance')}</button
            >
          </div>
        </div>
      </form>
    </Modal>
  {/if}
</div>
