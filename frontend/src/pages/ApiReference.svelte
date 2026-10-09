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
    decisions: 'ApiTagDecisions',
    exceptions: 'ApiTagExceptions',
    tasks: 'ApiTagTasks',
    instances: 'ApiTagInstances',
    rules: 'ApiTagRules',
    categories: 'ApiTagCategories',
    backups: 'ApiTagBackups',
  };
  const SCOPES: Record<string, string> = {
    read: 'ScopeRead',
    operate: 'ScopeOperate',
    write: 'ScopeWrite',
    configure: 'ScopeConfigure',
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
  // The screen's name is a link inside the sentence, so the sentence is cut
  // where the name goes and drawn around it.
  const keysLine = $derived(t('ApiReferenceKeys', { screen: '\u0000' }).split('\u0000'));
</script>

{#snippet spans(text: string)}
  {#each inline(text) as part, at (at)}
    {#if part.code}<code class="mono">{part.text}</code>{:else}{part.text}{/if}
  {/each}
{/snippet}

{#snippet prose(text: string)}
  {#each paragraphs(text) as paragraph, index (index)}
    <p lang="en" dir="ltr" class="api-prose">{@render spans(paragraph)}</p>
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
            <td lang="en" dir="ltr">{@render spans(row.description)}</td>
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
      <span class="badge api-method {METHOD_BADGE[op.method] ?? 'badge-info'}"
        >{op.method.toUpperCase()}</span
      >
      <code class="mono api-path">{op.path}</code>
      <span lang="en" dir="ltr" class="text-muted api-does">{op.summary}</span>
      <span class="badge badge-value api-scope"
        >{t(op.scope ? (SCOPES[op.scope] ?? op.scope) : 'ApiNoKey')}</span
      >
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
                  <td lang="en" dir="ltr">{@render spans(parameter.description ?? '')}</td>
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
            <span lang="en" dir="ltr" class="text-muted">{@render spans(response.description)}</span
            >
          </li>
        {/each}
      </ul>

      <h3>{t('ApiExample')}</h3>
      <!-- It scrolls sideways, and Safari makes no scroller focusable on its
           own: a named region in the tab order lets a keyboard reach its end. -->
      <pre
        class="mono api-example"
        tabindex="0"
        role="region"
        aria-label="{t('ApiExample')} – {title}">{curl(op, doc, server)}</pre>
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
    <div class="card"><Loading /></div>
  {:else if spec.data}
    {@const doc = spec.data}
    <!-- The contract's own introduction stays in the document a developer
         downloads: on this screen it is a page of English before the first
         operation. What a reader needs from it is where the key comes from. -->
    <div class="card">
      <p>
        {keysLine[0]}<a class="text-link" href={href('/applications')}>{t('Applications')}</a
        >{keysLine[1]}
      </p>
      <p class="text-muted text-sm mt-1">{t('ApiReferenceEnglish')}</p>
    </div>

    <div class="toolbar">
      <SearchField bind:value={search} placeholder={t('ApiSearch')} label={t('ApiSearch')} />
    </div>

    {#each groups as group (group.tag)}
      <section class="card">
        <div class="card-header">
          <h2 class="card-title">{t(TAGS[group.tag] ?? group.tag)}</h2>
        </div>
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
      <div class="card-header">
        <h2 class="card-title">{t('ApiSchemas')}</h2>
      </div>
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
