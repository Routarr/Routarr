<script lang="ts">
  import { api } from '../api/client';
  import {
    byTag,
    curl,
    fields,
    inline,
    paragraphs,
    serverUrl,
    typeLabel,
    type Field,
    type OpenApiDocument,
    type Operation,
  } from '../api/openapi';
  import { createAsync } from '../lib/async.svelte';
  import { t } from '../lib/i18n.svelte';
  import { href } from '../lib/router.svelte';
  import { Download } from '../lib/icons';
  import ErrorBanner from '../components/ErrorBanner.svelte';
  import Loading from '../components/Loading.svelte';
  import SearchField from '../components/SearchField.svelte';
  import TableRegion from '../components/TableRegion.svelte';

  /**
   * The API other applications call, read from the contract the running
   * version serves. Its prose is the contract's own, in English, as the API
   * is: marked `lang="en"` so a screen reader reads it as English, and
   * `dir="ltr"`, which `lang` does not set, so in Arabic its full stops and
   * code stay where English puts them.
   */
  const spec = createAsync((signal) => api.openApi(signal));
  let search = $state('');

  const TAGS: Record<string, string> = {
    status: 'ApiTagStatus',
    library: 'ApiTagLibrary',
    proposals: 'ApiTagProposals',
    exceptions: 'ApiTagExceptions',
    tasks: 'ApiTagTasks',
    instances: 'ApiTagInstances',
  };
  const SCOPES: Record<string, string> = {
    read: 'ScopeRead',
    operate: 'ScopeOperate',
    write: 'ScopeWrite',
  };
  const METHOD_BADGE: Record<string, string> = {
    get: 'badge-info',
    post: 'badge-success',
    put: 'badge-warning',
    patch: 'badge-warning',
    delete: 'badge-danger',
  };

  function matches(op: Operation): boolean {
    const wanted = search.trim().toLowerCase();
    return (
      !wanted ||
      op.path.toLowerCase().includes(wanted) ||
      op.summary.toLowerCase().includes(wanted) ||
      op.method.includes(wanted)
    );
  }

  const groups = $derived(
    spec.data
      ? byTag(spec.data)
          .map((group) => ({ ...group, operations: group.operations.filter(matches) }))
          .filter((group) => group.operations.length > 0)
      : [],
  );
  const server = $derived(spec.data ? serverUrl(spec.data, window.location.origin) : '');
  const schemas = $derived(Object.keys(spec.data?.components?.schemas ?? {}).sort());
</script>

{#snippet prose(text: string)}
  {#each paragraphs(text) as paragraph, index (index)}
    <p lang="en" dir="ltr" class="api-prose">
      {#each inline(paragraph) as part, at (at)}
        {#if part.code}<code class="mono">{part.text}</code>{:else}{part.text}{/if}
      {/each}
    </p>
  {/each}
{/snippet}

{#snippet fieldTable(caption: string, rows: Field[])}
  <TableRegion label={caption}>
    <table>
      <caption class="visually-hidden">{caption}</caption>
      <thead>
        <tr>
          <th>{t('Name')}</th>
          <th>{t('Type')}</th>
          <th>{t('ApiRequired')}</th>
          <th>{t('Description')}</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.name)}
          <tr>
            <td class="mono">{row.name}</td>
            <td class="mono">{row.type}</td>
            <td>{t(row.required ? 'Yes' : 'No')}</td>
            <td lang="en" dir="ltr">{row.description}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </TableRegion>
{/snippet}

{#snippet operation(doc: OpenApiDocument, op: Operation)}
  {@const title = `${op.method.toUpperCase()} ${op.path}`}
  <details class="api-operation">
    <summary>
      <span class="badge {METHOD_BADGE[op.method] ?? 'badge-info'}">{op.method.toUpperCase()}</span>
      <code class="mono">{op.path}</code>
      <span class="badge badge-value"
        >{t(op.scope ? (SCOPES[op.scope] ?? op.scope) : 'ApiNoKey')}</span
      >
      <span lang="en" dir="ltr" class="text-muted">{op.summary}</span>
    </summary>
    <div class="api-operation-body">
      {@render prose(op.description)}

      {#if op.parameters.length > 0}
        <h3>{t('ApiParameters')}</h3>
        <TableRegion label="{t('ApiParameters')} – {title}">
          <table>
            <caption class="visually-hidden">{t('ApiParameters')} – {title}</caption>
            <thead>
              <tr>
                <th>{t('Name')}</th>
                <th>{t('ApiIn')}</th>
                <th>{t('Type')}</th>
                <th>{t('ApiRequired')}</th>
                <th>{t('Description')}</th>
              </tr>
            </thead>
            <tbody>
              {#each op.parameters as parameter (`${parameter.in}-${parameter.name}`)}
                <tr>
                  <td class="mono">{parameter.name}</td>
                  <td class="mono">{parameter.in}</td>
                  <td class="mono">{typeLabel(parameter.schema)}</td>
                  <td>{t(parameter.required ? 'Yes' : 'No')}</td>
                  <td lang="en" dir="ltr">{parameter.description ?? ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </TableRegion>
      {/if}

      {#if op.body}
        {@const rows = fields(op.body, doc)}
        <h3>{t('ApiRequestBody')} <code class="mono">{typeLabel(op.body)}</code></h3>
        {#if rows.length > 0}
          {@render fieldTable(`${t('ApiRequestBody')} – ${title}`, rows)}
        {/if}
      {/if}

      <h3>{t('ApiResponses')}</h3>
      <ul class="api-responses">
        {#each op.responses as response (response.code)}
          <li>
            <code class="mono">{response.code}</code>
            {#if response.schema}<code class="mono">{typeLabel(response.schema)}</code>{/if}
            <span lang="en" dir="ltr" class="text-muted">{response.description}</span>
          </li>
        {/each}
      </ul>

      <h3>{t('ApiExample')}</h3>
      <pre class="mono api-example">{curl(op, doc, server)}</pre>
    </div>
  </details>
{/snippet}

<div>
  <div class="page-header">
    <div>
      <h1 class="page-title">{t('ApiReference')}</h1>
      <p class="page-subtitle">{t('ApiReferenceSubtitle')}</p>
    </div>
    <div class="flex gap-2">
      <a class="btn btn-secondary" href={href('/api/v1/openapi.json')} download="openapi.json">
        <Download size={16} />
        {t('ApiDownloadSpec')}
      </a>
    </div>
  </div>

  <ErrorBanner
    message={spec.error}
    onDismiss={() => (spec.error = null)}
    onRetry={() => void spec.reload()}
  />

  {#if spec.loading && !spec.data}
    <Loading />
  {:else if spec.data}
    {@const doc = spec.data}
    <div class="card">
      <p class="text-muted text-sm">{t('ApiReferenceEnglish')}</p>
      {@render prose(doc.info.description ?? '')}
    </div>

    <div class="card">
      <SearchField bind:value={search} placeholder={t('ApiSearch')} label={t('ApiSearch')} />
    </div>

    {#each groups as group (group.tag)}
      <section class="card">
        <h2 class="card-title">{t(TAGS[group.tag] ?? group.tag)}</h2>
        {@render prose(group.description)}
        {#each group.operations as op (op.id)}
          {@render operation(doc, op)}
        {/each}
      </section>
    {:else}
      <div class="card">
        <p class="text-muted">{t('ApiNoMatch')}</p>
      </div>
    {/each}

    <section class="card">
      <h2 class="card-title">{t('ApiSchemas')}</h2>
      {#each schemas as name (name)}
        {@const schema = doc.components?.schemas?.[name] ?? {}}
        {@const rows = fields(schema, doc)}
        <details class="api-operation">
          <summary><code class="mono">{name}</code></summary>
          <div class="api-operation-body">
            {@render prose(schema.description ?? '')}
            {#if rows.length > 0}
              {@render fieldTable(`${t('ApiSchemas')} – ${name}`, rows)}
            {:else}
              <p class="mono">{typeLabel(schema)}</p>
            {/if}
          </div>
        </details>
      {/each}
    </section>
  {/if}
</div>
