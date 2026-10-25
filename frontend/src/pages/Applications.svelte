<script lang="ts">
  import { Plus, Trash2 } from '../lib/icons';
  import { api } from '../api/client';
  import { withProof } from '../lib/proof.svelte';
  import type { ApplicationScope, Guardrail, MintedApplication } from '../api/types';
  import { createAsync, describeError } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { expiresWithinAWeek, formatTimestamp, idleFor90Days, toggled } from '../api/format';
  import { askConfirmation } from '../lib/confirm.svelte';
  import { handFocus } from '../lib/focus';
  import NoValue from '../components/NoValue.svelte';
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
    { value: 'configure', key: 'ScopeConfigure', help: 'ScopeConfigureHelp' },
  ];

  /** The guardrails a key may answer on its own, the most common first. */
  const GUARDRAILS: { value: Guardrail; key: string }[] = [
    { value: 'batch', key: 'GuardrailBatch' },
    { value: 'threshold', key: 'GuardrailThreshold' },
    { value: 'capacity', key: 'GuardrailCapacity' },
    { value: 'unreachable', key: 'GuardrailUnreachable' },
  ];

  /** The lives the form offers. A key is never renewed: a new one replaces it. */
  const EXPIRIES: { days: number | null; label: string }[] = [
    { days: null, label: 'Never' },
    { days: 30, label: 'ApplicationExpiryIn30Days' },
    { days: 90, label: 'ApplicationExpiryIn90Days' },
    { days: 365, label: 'ApplicationExpiryInAYear' },
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
  let expiresInDays = $state<number | null>(null);
  // A guardrail and a file move are only ever asked of a key that operates.
  const operates = $derived(scopes.includes('operate'));

  function openCreate() {
    name = '';
    scopes = [];
    mayConfirm = [];
    mayMoveFiles = false;
    expiresInDays = null;
    dialogError = null;
    creating = true;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    dialogError = null;
    try {
      const made = await withProof((proof) =>
        api.createApplication(
          {
            name,
            scopes,
            may_confirm: operates ? mayConfirm : [],
            may_move_files: operates && mayMoveFiles,
            expires_in_days: expiresInDays,
          },
          proof,
        ),
      );
      if (!made) return;
      minted = made;
      creating = false;
      outcome.succeed(t('ApplicationCreated', { name: minted.name }));
      await applications.reload();
    } catch (err) {
      dialogError = describeError(err);
    }
  }

  // A revoked key takes its row and the pressed Revoke with it: the key now in
  // its place takes the focus, else the one before, else the table.
  const revokeId = (index: number) => `applications-revoke-${index}`;

  async function revoke(id: string, keyName: string, index: number) {
    const question = t('ConfirmRevokeApplication', { name: keyName });
    if (!(await askConfirmation(question, 'RevokeKey'))) return;
    try {
      await api.revokeApplication(id);
      if (minted?.id === id) minted = null;
      outcome.succeed(t('ApplicationRevoked', { name: keyName }));
      await applications.reload();
      void handFocus(revokeId(index), revokeId(index - 1), 'applications-table');
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
    <TableRegion label={t('Applications')} id="applications-table">
      <table>
        <caption class="visually-hidden">{t('Applications')}</caption>
        <thead>
          <tr>
            <th>{t('Name')}</th>
            <th>{t('ApplicationScopes')}</th>
            <th>{t('ApplicationMayConfirm')}</th>
            <th>{t('ApplicationMovesFiles')}</th>
            <th>{t('Created')}</th>
            <th>{t('ApplicationExpiry')}</th>
            <th>{t('LastUsed')}</th>
            <th class="w-80"><span class="visually-hidden">{t('Actions')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#if applications.loading && rows.length === 0}
            <TableSkeleton columns={8} />
          {:else if rows.length === 0 && !applications.error}
            <tr><td colspan="8"><EmptyState>{t('NoApplications')}</EmptyState></td></tr>
          {:else}
            {#each rows as application, index (application.id)}
              <tr class:row-muted={application.expired}>
                <td><strong>{application.name}</strong></td>
                <td>
                  <!-- A wrapping row with a gap, as chips are elsewhere: stacked in
                       a narrow column, bare badges touch. -->
                  <div class="flex flex-wrap gap-1">
                    <span class="badge badge-value">{t('ScopeRead')}</span>
                    {#each application.scopes as scope (scope)}
                      <span class="badge badge-value">{t(scopeKey(scope))}</span>
                    {/each}
                  </div>
                </td>
                <td class="text-muted">
                  {#if application.may_confirm.length === 0}
                    <NoValue />
                  {:else}
                    {application.may_confirm
                      .map((guardrail) => t(guardrailKey(guardrail)))
                      .join(t('ListSeparator'))}
                  {/if}
                </td>
                <td>{t(application.may_move_files ? 'Yes' : 'No')}</td>
                <td class="cell-timestamp">
                  <span>{formatTimestamp(application.created_at, i18n.language)}</span>
                  {#if application.created_by}
                    <span class="text-xs text-muted" title={t('PerformedBy')}
                      ><span class="visually-hidden">{t('PerformedBy')}</span
                      >{application.created_by}</span
                    >
                  {/if}
                </td>
                <td class="cell-timestamp">
                  <span>
                    {application.expires_at
                      ? formatTimestamp(application.expires_at, i18n.language)
                      : t('Never')}
                  </span>
                  {#if application.expired}
                    <span class="badge badge-danger">{t('ApplicationExpired')}</span>
                  {:else if expiresWithinAWeek(application.expires_at)}
                    <span class="badge badge-warning">{t('ApplicationExpiresSoon')}</span>
                  {/if}
                </td>
                <td class="cell-timestamp">
                  <span>
                    {application.last_used_at
                      ? formatTimestamp(application.last_used_at, i18n.language)
                      : t('Never')}
                  </span>
                  {#if !application.expired && idleFor90Days(application.last_used_at, application.created_at)}
                    <span class="badge badge-warning">{t('ApplicationLongUnused')}</span>
                  {/if}
                </td>
                <td>
                  <button
                    id={revokeId(index)}
                    class="btn btn-danger btn-sm"
                    aria-label="{t('RevokeKey')} – {application.name}"
                    title={t('RevokeKey')}
                    onclick={() => void revoke(application.id, application.name, index)}
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

        <div class="form-group">
          <label class="form-label" for="application-expiry">{t('ApplicationExpiry')}</label>
          <select
            id="application-expiry"
            class="form-select"
            aria-describedby="application-expiry-help"
            bind:value={expiresInDays}
          >
            {#each EXPIRIES as expiry (expiry.label)}
              <option value={expiry.days}>{t(expiry.label)}</option>
            {/each}
          </select>
          <p id="application-expiry-help" class="form-hint">{t('ApplicationExpiryHelp')}</p>
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
          <!-- Held without a name, as every other form here holds its submit
               until the required field is filled. -->
          <button type="submit" class="btn btn-primary" disabled={!name.trim()}
            >{t('CreateKey')}</button
          >
        </div>
      </form>
    </Modal>
  {/if}
</div>
