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
    withoutCredentials,
  } from '../api/format';
  import type { Instance } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { takeQueryFlag } from '../api/onboarding';
  import { i18n, t } from '../lib/i18n.svelte';
  import { handFocus } from '../lib/focus';
  import ActionMenu from '../components/ActionMenu.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import GuideStepBanner from '../components/GuideStepBanner.svelte';
  import { askConfirmation } from '../lib/confirm.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import { invalidateStatus } from '../lib/status.svelte';
  import { poll } from '../lib/poll.svelte';

  interface FormState {
    name: string;
    instance_type: 'radarr' | 'sonarr';
    base_url: string;
    api_key: string;
    enabled: boolean;
    sync_interval_minutes: number;
  }

  // Empty rather than a guess: a guessed address left in the field is saved by
  // whoever does not notice it, and `localhost` is Routarr's own container in
  // Docker. The example shows the shape instead, as a placeholder.
  const EXAMPLE_URL = { radarr: 'http://radarr:7878', sonarr: 'http://sonarr:8989' } as const;
  const SERVICE = { radarr: 'Radarr', sonarr: 'Sonarr' } as const;

  // The backend clamps an interval outside these bounds, which would store a
  // figure nobody typed: Save waits for one inside them instead.
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
  });

  const list = createAsync((signal) => api.getInstances(signal));
  const outcome = createOutcome();
  let busyId = $state<string | null>(null);
  let editing = $state<{ form: FormState; id?: string; webhookUrl?: string | null } | null>(null);

  const instances = $derived(list.data ?? []);

  // Generic over what the call returns, so the message reads the typed payload
  // the client already declares: a cast here is a field rename nobody sees.
  async function act<T>(id: string, fn: () => Promise<T>, describe: (result: T) => string) {
    const pressed = document.activeElement as HTMLElement | null;
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
      void handFocus(pressed);
    }
  }

  // Shown inside the dialog: the page banner sits under a modal that makes
  // the page inert, dimmed, with a Dismiss nobody can press, and a 400 on the
  // URL would look like a Save button that did nothing.
  let formError = $state<string | null>(null);

  // What the typed values answered, a success or a failure, kept with the
  // values it answered for: once one of them changes, the result would vouch
  // for values nobody tried.
  let probe = $state<{ tried: string; ok: boolean; text: string } | null>(null);
  let probing = $state(false);
  const typed = $derived(
    editing
      ? [editing.form.instance_type, editing.form.base_url, editing.form.api_key].join('\n')
      : '',
  );
  const probed = $derived(probe && probe.tried === typed ? probe : null);
  // One refusal at a time, the try's or the save's, whichever came last.
  const refused = $derived(formError ?? (probed && !probed.ok ? probed.text : null));

  // A try waits a full connect timeout on an address that does not answer,
  // and outlives nothing: the dialog it was for may be gone by then.
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

  // A second press while the first is in flight would add the same Arr twice,
  // and the server accepts both: names are not unique.
  let saving = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!editing || saving) return;
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

  // Left to the scheduler, an instance never synced waits up to a whole pass
  // with no library and no root folders, and the guide's first step waits with
  // it. Synced on save, a wrong address or key also shows while the form is
  // fresh in mind. Saved and not synced is a partial result, not a failure.
  async function syncFirstTime(saved: Instance) {
    busyId = saved.id;
    try {
      const report = await api.syncInstance(saved.id);
      outcome.succeed(
        t('SyncResult', {
          name: saved.name,
          media: report.media,
          folders: report.root_folders,
        }),
      );
    } catch (err) {
      // The server finishes a sync the browser stopped waiting for, and a
      // large library outlasts a request. The banner saying the library is
      // being read stays true, and the list is followed until it is.
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

  // The first sync the browser stopped waiting for, until its attempt is
  // recorded on the instance, which happens once it ends.
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
    const pressed = document.activeElement as HTMLElement | null;
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
      void handFocus(pressed);
    }
  }

  const webhookAddress = (path: string) => `${window.location.origin}${path}`;

  // Said once the clipboard took it. The clipboard exists on secure origins
  // only, and a homelab serves plain http more often than not, so a refusal
  // hands over the URL itself.
  async function copyWebhookUrl(instance: Instance) {
    const url = webhookAddress(instance.webhook_url ?? '');
    try {
      await navigator.clipboard.writeText(url);
      outcome.succeed(t('WebhookUrlCopied'));
    } catch {
      outcome.fail(t('WebhookUrlCopyFailed', { url }));
    }
  }

  // A dialog opens on its own form, never on the refusal or the try of the one
  // before.
  function startAdd() {
    stopTrying();
    formError = null;
    probe = null;
    editing = { form: blankForm() };
  }
  function close() {
    stopTrying();
    editing = null;
    formError = null;
  }
  // The guide's "Add an instance" lands here with the dialog open.
  if (takeQueryFlag('add')) startAdd();

  function startEdit(instance: Instance) {
    stopTrying();
    formError = null;
    probe = null;
    editing = {
      id: instance.id,
      webhookUrl: instance.webhook_url,
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
      <h1 class="page-title">{t('Instances')}</h1>
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
      <button id="instances-add" class="btn btn-primary" onclick={startAdd}>
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
  <GuideStepBanner step="instance" />

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
            <th class="w-230"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if list.loading && instances.length === 0}
            <TableSkeleton columns={6} />
          {:else if instances.length === 0 && !list.error}
            <tr><td colspan="6"><EmptyState>{t('NoInstanceConfigured')}</EmptyState></td></tr>
          {:else}
            {#each instances as instance (instance.id)}
              {@const address = withoutCredentials(instance.base_url)}
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
                <td class="mono text-sm">
                  {#if opensInBrowser(address)}
                    <!-- The Arr's own interface, one click from its row. -->
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
                  <!-- Encrypted is a property of the stored value, not an
                       outcome, so it is not green: green means "this went
                       well". -->
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
                       The date is the last *success*: a failed attempt stamping
                       it would have the cell read "2 minutes ago" beside an
                       error badge, describing data two days old. -->
                  <span
                    class="text-muted"
                    title={formatTimestamp(instance.last_sync_at, i18n.language, '')}
                  >
                    {formatRelative(instance.last_sync_at, i18n.language, t('Never'))}
                  </span>
                  {#if instance.last_sync_attempt_at && instance.last_sync_attempt_at !== instance.last_sync_at}
                    <!-- Only when they differ, which is exactly when a later
                         attempt failed. The useful thing to say then is when
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
                      title={instance.last_sync_status === 'success'
                        ? undefined
                        : failureDetail(instance.last_sync_status)}
                    >
                      {t(
                        instance.last_sync_status === 'success' ? 'StatusSuccess' : 'StatusFailed',
                      )}
                    </span>
                  {/if}
                </td>
                <td>
                  <div class="row-actions">
                    <!-- Two visible, the rest behind the menu: every action as
                         a button, across two columns, pushes the row off a
                         desktop screen. -->
                    <button
                      class="btn btn-secondary btn-sm"
                      disabled={busyId !== null}
                      title={t('SyncNow')}
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
    {@const intervalOk = intervalFits(form.sync_interval_minutes)}
    <Modal
      label={t(isEdit ? 'EditInstance' : 'AddInstance')}
      onClose={close}
      initialFocus="instances-name"
      returnFocus="instances-add"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t(isEdit ? 'EditInstance' : 'AddInstance')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={close}
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
      <!-- `novalidate`: the browser's own bubble renders in the *browser's*
       language whatever `ui_language` says, and fires before the submit
       handler. Nothing is traded away for it: Save is held until the required
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
            aria-describedby="instances-base-url-help"
            bind:value={form.base_url}
            onblur={() => completeScheme(form)}
            autocapitalize="off"
            spellcheck="false"
            required
          />
          <p id="instances-base-url-help" class="text-muted text-sm mt-1">
            {t('InstanceUrlHelp', { service: SERVICE[form.instance_type] })}
          </p>
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
        {#if editing.webhookUrl}
          <!-- Here and not only behind Copy: on a plain http origin the
               clipboard is closed, and a failed copy would be the one place
               the URL shows. -->
          <div class="form-group">
            <label class="form-label" for="instances-webhook-url">{t('WebhookUrl')}</label>
            <input
              id="instances-webhook-url"
              class="form-input mono"
              value={webhookAddress(editing.webhookUrl)}
              readonly
              aria-describedby="instances-webhook-url-help"
            />
            <p id="instances-webhook-url-help" class="text-muted text-sm mt-1">
              {t('WebhookUrlHelp', { service: SERVICE[form.instance_type] })}
            </p>
          </div>
        {/if}
        <label class="flex items-center gap-2 text-base">
          <input type="checkbox" bind:checked={form.enabled} />
          {t('EnabledSyncedRouted')}
        </label>
        <!-- Mounted before the answer, so filling it is a change a screen
             reader reports. -->
        <div role="status">
          {#if probed?.ok}
            <p class="probe-ok">
              <CheckCircle2 size={16} aria-hidden="true" />
              {probed.text}
            </p>
          {/if}
        </div>
        <div class="dialog-actions">
          <button type="button" class="btn btn-secondary" onclick={close}>
            {t('Cancel')}
          </button>
          <div class="flex flex-wrap gap-2">
            <!-- Never held while a try runs: a focused button that turns
                 disabled drops the focus to the page, and a second press
                 starts the try over. -->
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
