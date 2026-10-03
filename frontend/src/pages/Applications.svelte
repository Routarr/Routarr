<script lang="ts">
  import { Plus, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import type { ApplicationScope, Guardrail, MintedApplication } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { formatTimestamp } from '../api/format';
  import { askConfirmation } from '../lib/confirm.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Modal from '../components/Modal.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import WarningBanner from '../components/WarningBanner.svelte';

  const applications = createAsync((signal) => api.getApplications(signal));
  const outcome = createOutcome();
  const rows = $derived(applications.data ?? []);

  /** What a key may be given beyond read, which every key holds. */
  const SCOPES: { value: ApplicationScope; key: string; help: string }[] = [
    { value: 'operate', key: 'ScopeOperate', help: 'ScopeOperateHelp' },
    { value: 'write', key: 'ScopeWrite', help: 'ScopeWriteHelp' },
  ];

  /** The guardrails a key may answer on its own, the most common first. */
  const GUARDRAILS: { value: Guardrail; key: string }[] = [
    { value: 'batch', key: 'GuardrailBatch' },
    { value: 'threshold', key: 'GuardrailThreshold' },
    { value: 'capacity', key: 'GuardrailCapacity' },
    { value: 'unreachable', key: 'GuardrailUnreachable' },
  ];

  const scopeKey = (scope: ApplicationScope) =>
    SCOPES.find((entry) => entry.value === scope)?.key ?? scope;
  const guardrailKey = (name: Guardrail) =>
    GUARDRAILS.find((entry) => entry.value === name)?.key ?? name;

  /** The key just made, whose token this page shows until it is left. */
  let minted = $state<MintedApplication | null>(null);

  let creating = $state(false);
  let dialogError = $state<string | null>(null);
  let name = $state('');
  let scopes = $state<ApplicationScope[]>([]);
  let mayConfirm = $state<Guardrail[]>([]);
  let mayMoveFiles = $state(false);
  // A guardrail and a file move are only ever asked of a key that operates.
  const operates = $derived(scopes.includes('operate'));

  function openCreate() {
    name = '';
    scopes = [];
    mayConfirm = [];
    mayMoveFiles = false;
    dialogError = null;
    creating = true;
  }

  function toggled<T>(list: T[], value: T, on: boolean): T[] {
    return on ? [...list, value] : list.filter((entry) => entry !== value);
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    dialogError = null;
    try {
      minted = await api.createApplication({
        name,
        scopes,
        may_confirm: operates ? mayConfirm : [],
        may_move_files: operates && mayMoveFiles,
      });
      creating = false;
      outcome.succeed(t('ApplicationCreated', { name: minted.name }));
      await applications.reload();
    } catch (err) {
      dialogError = describeError(err);
    }
  }

  async function revoke(id: string, keyName: string) {
    const question = t('ConfirmRevokeApplication', { name: keyName });
    if (!(await askConfirmation(question, 'RevokeKey'))) return;
    try {
      await api.revokeApplication(id);
      if (minted?.id === id) minted = null;
      outcome.succeed(t('ApplicationRevoked', { name: keyName }));
      await applications.reload();
    } catch (err) {
      outcome.fail(err);
    }
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('Applications')}</h1>
      <p class="page-subtitle">{t('ApplicationsSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button class="btn btn-primary" onclick={openCreate}>
        <Plus size={16} />
        {t('NewApplicationKey')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={applications.error}
    onDismiss={() => (applications.error = null)}
    onRetry={() => void applications.reload()}
  />
  <OutcomeBanner {outcome} />

  {#if minted}
    <!-- The one moment the token is readable. `mono` carries the
         `direction: ltr` a token needs in a right-to-left page. -->
    <div class="card">
      <p><strong>{t('ApplicationTokenFor', { name: minted.name })}</strong></p>
      <WarningBanner message={t('ApiKeyMintedOnce')} />
      <code class="mono secret-once">{minted.token}</code>
    </div>
  {/if}

  <div class="card">
    <TableRegion label={t('Applications')}>
      <table>
        <caption class="visually-hidden">{t('Applications')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('ApplicationScopes')}</th>
            <th>{t('ApplicationMayConfirm')}</th>
            <th>{t('ApplicationMovesFiles')}</th>
            <th>{t('Created')}</th>
            <th>{t('LastUsed')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if applications.loading && rows.length === 0}
            <TableSkeleton columns={7} />
          {:else if rows.length === 0 && !applications.error}
            <tr><td colspan="7"><EmptyState>{t('NoApplications')}</EmptyState></td></tr>
          {:else}
            {#each rows as application (application.id)}
              <tr>
                <td><strong>{application.name}</strong></td>
                <td>
                  <span class="badge badge-value">{t('ScopeRead')}</span>
                  {#each application.scopes as scope (scope)}
                    <span class="badge badge-value">{t(scopeKey(scope))}</span>
                  {/each}
                </td>
                <td class="text-muted">
                  {application.may_confirm.length === 0
                    ? t('None')
                    : application.may_confirm
                        .map((guardrail) => t(guardrailKey(guardrail)))
                        .join(t('ListSeparator'))}
                </td>
                <td>{t(application.may_move_files ? 'Yes' : 'No')}</td>
                <td class="cell-timestamp">
                  <span>{formatTimestamp(application.created_at, i18n.language)}</span>
                  {#if application.created_by}
                    <span class="text-xs text-muted" title={t('PerformedBy')}
                      >{application.created_by}</span
                    >
                  {/if}
                </td>
                <td class="cell-timestamp">
                  {application.last_used_at
                    ? formatTimestamp(application.last_used_at, i18n.language)
                    : t('Never')}
                </td>
                <td>
                  <button
                    class="btn btn-danger btn-sm"
                    aria-label="{t('RevokeKey')} – {application.name}"
                    title={t('RevokeKey')}
                    onclick={() => void revoke(application.id, application.name)}
                  >
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>
  </div>

  {#if creating}
    <Modal
      label={t('NewApplicationKey')}
      onClose={() => (creating = false)}
      maxWidth={560}
      initialFocus="application-name"
    >
      <div class="modal-header">
        <h2 class="modal-title">{t('NewApplicationKey')}</h2>
        <button
          class="btn btn-secondary btn-sm"
          onclick={() => (creating = false)}
          aria-label={t('Dismiss')}
          title={t('Dismiss')}
        >
          ✕
        </button>
      </div>
      <ErrorBanner message={dialogError} onDismiss={() => (dialogError = null)} />

      <form novalidate onsubmit={save}>
        <div class="form-group">
          <label class="form-label" for="application-name">{t('Name')}</label>
          <input
            id="application-name"
            class="form-input"
            placeholder={t('ApplicationNamePlaceholder')}
            bind:value={name}
          />
        </div>

        <fieldset class="form-group">
          <legend>{t('ApplicationScopes')}</legend>
          <label class="check-option">
            <input type="checkbox" checked disabled />
            <span>{t('ScopeRead')}<span class="form-hint">{t('ScopeReadHelp')}</span></span>
          </label>
          {#each SCOPES as scope (scope.value)}
            <label class="check-option">
              <input
                type="checkbox"
                checked={scopes.includes(scope.value)}
                onchange={(event) =>
                  (scopes = toggled(scopes, scope.value, event.currentTarget.checked))}
              />
              <span>{t(scope.key)}<span class="form-hint">{t(scope.help)}</span></span>
            </label>
          {/each}
        </fieldset>

        <fieldset class="form-group">
          <legend>{t('ApplicationMayConfirm')}</legend>
          <p class="form-hint">{t('ApplicationMayConfirmHelp')}</p>
          {#each GUARDRAILS as guardrail (guardrail.value)}
            <label class="check-option">
              <input
                type="checkbox"
                disabled={!operates}
                checked={operates && mayConfirm.includes(guardrail.value)}
                onchange={(event) =>
                  (mayConfirm = toggled(mayConfirm, guardrail.value, event.currentTarget.checked))}
              />
              <span>{t(guardrail.key)}</span>
            </label>
          {/each}
          <label class="check-option">
            <input
              type="checkbox"
              disabled={!operates}
              checked={operates && mayMoveFiles}
              onchange={(event) => (mayMoveFiles = event.currentTarget.checked)}
            />
            <span>{t('ApplicationMayMoveFiles')}</span>
          </label>
        </fieldset>

        <div class="dialog-actions">
          <button type="button" class="btn btn-secondary" onclick={() => (creating = false)}>
            {t('Cancel')}
          </button>
          <button type="submit" class="btn btn-primary">{t('CreateKey')}</button>
        </div>
      </form>
    </Modal>
  {/if}
</div>
