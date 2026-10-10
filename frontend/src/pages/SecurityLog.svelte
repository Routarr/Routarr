<script lang="ts">
  import { Download, RefreshCw } from '../lib/icons';
  import { ApiError, api } from '../api/client';
  import { capitalize, formatTimestamp } from '../api/format';
  import type { SecurityEventKind } from '../api/types';
  import { createAsync } from '../lib/async.svelte';
  import { createOutcome } from '../lib/outcome.svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import NoValue from '../components/NoValue.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import OutcomeBanner from '../components/OutcomeBanner.svelte';
  import TableSkeleton from '../components/TableSkeleton.svelte';
  import TableRegion from '../components/TableRegion.svelte';
  import Pager from '../components/Pager.svelte';
  import SearchField from '../components/SearchField.svelte';
  import { downloadBlob } from '../lib/download';

  /**
   * Who signed in, who was refused, and who changed a key or a setting, from
   * which address. The server writes the same events to its log, where a
   * fail2ban filter reads them.
   */
  const KINDS: SecurityEventKind[] = [
    'sign_in',
    'sign_out',
    'api_key',
    'application_key',
    'scope',
    'origin',
    'password',
    'proof',
    'backup',
    'restore',
    'configuration',
    'settings',
    'signing_secret',
    'security_log',
  ];

  /** `sign_in` reads `SecurityKindSignIn`. */
  const kindKey = (kind: string) => `SecurityKind${kind.split('_').map(capitalize).join('')}`;

  let search = $state('');
  let kind = $state('');
  let outcome = $state('');
  let page = $state(1);

  const filters = $derived({
    search: search || undefined,
    kind: kind || undefined,
    outcome: outcome || undefined,
    page,
    per_page: 50,
  });

  const events = createAsync(
    (signal) => api.getSecurityLog(filters, signal),
    () => [search, kind, outcome, page],
  );
  const exported = createOutcome();

  const rows = $derived(events.data?.data ?? []);
  const pagination = $derived(events.data?.pagination);

  async function download() {
    try {
      downloadBlob(await api.exportSecurityLog(filters), 'routarr-security-log.csv');
      exported.clear();
    } catch (err) {
      exported.fail(
        err instanceof ApiError && err.status > 0 ? t('ExportFailed', { status: err.status }) : err,
      );
    }
  }

  function clear() {
    search = '';
    kind = '';
    outcome = '';
    page = 1;
  }
</script>

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('SecurityLog')}</h1>
      <p class="page-subtitle">{t('SecurityLogSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <button class="btn btn-secondary" onclick={() => void events.reload()}>
        <RefreshCw size={16} />
        {t('Refresh')}
      </button>
      <button class="btn btn-secondary" onclick={download} disabled={rows.length === 0}>
        <Download size={16} />
        {t('ExportCsv')}
      </button>
    </div>
  </div>

  <ErrorBanner
    message={events.error}
    onDismiss={() => (events.error = null)}
    onRetry={() => void events.reload()}
  />
  <OutcomeBanner outcome={exported} />

  <div class="toolbar">
    <SearchField
      bind:value={search}
      placeholder={t('SearchNameOrAddress')}
      label={t('SearchNameOrAddress')}
      oninput={() => (page = 1)}
      debounce={200}
    />
    <select
      class="form-select"
      aria-label={t('FilterByEvent')}
      value={kind}
      onchange={(event) => {
        page = 1;
        kind = event.currentTarget.value;
      }}
    >
      <option value="">{t('AllEvents')}</option>
      {#each KINDS as value (value)}
        <option {value}>{t(kindKey(value))}</option>
      {/each}
    </select>
    <select
      class="form-select"
      aria-label={t('FilterByOutcome')}
      value={outcome}
      onchange={(event) => {
        page = 1;
        outcome = event.currentTarget.value;
      }}
    >
      <option value="">{t('AllOutcomes')}</option>
      <option value="allowed">{t('SecurityOutcomeAllowed')}</option>
      <option value="refused">{t('SecurityOutcomeRefused')}</option>
    </select>
    {#if search || kind || outcome}
      <button type="button" class="btn btn-ghost" onclick={clear}>{t('ClearFilters')}</button>
    {/if}
  </div>

  <div class="card">
    <TableRegion label={t('SecurityEvents')}>
      <table>
        <caption class="visually-hidden">{t('SecurityEvents')}</caption>
        <thead>
          <tr>
            <th>{t('When')}</th>
            <th>{t('SecurityEvent')}</th>
            <th>{t('Outcome')}</th>
            <th>{t('SecurityWho')}</th>
            <th>{t('Details')}</th>
          </tr>
        </thead>
        <tbody>
          {#if events.loading && rows.length === 0}
            <TableSkeleton columns={5} />
          {:else if rows.length === 0 && !events.error}
            <tr><td colspan="5"><EmptyState>{t('NoSecurityEvents')}</EmptyState></td></tr>
          {:else}
            {#each rows as row (row.id)}
              <tr>
                <td class="cell-timestamp" title={row.at}>
                  {formatTimestamp(row.at, i18n.language)}
                </td>
                <td><span class="badge badge-value muted">{t(kindKey(row.kind))}</span></td>
                <td>
                  <span
                    class="badge {row.outcome === 'allowed' ? 'badge-success' : 'badge-danger'}"
                  >
                    {t(
                      row.outcome === 'allowed'
                        ? 'SecurityOutcomeAllowed'
                        : 'SecurityOutcomeRefused',
                    )}
                  </span>
                </td>
                <td>
                  {#if row.subject}{row.subject}{:else}<NoValue />{/if}
                  {#if row.client}
                    <span class="mono text-xs text-muted">{row.client}</span>
                  {/if}
                </td>
                <td>
                  {t(row.message, row.params)}
                  {#if row.repeated > 0}
                    <span class="text-xs text-muted"
                      >{t('SecurityEventRepeated', { count: row.repeated })}</span
                    >
                  {/if}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </TableRegion>

    <Pager {pagination} bind:page countKey="SecurityEventCount" />
  </div>
</div>
