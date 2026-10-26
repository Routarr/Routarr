<script lang="ts">
  import type { FacetAxis, LibraryFacets, Vocabularies } from '../api/types';
  import { localFacets, nameFacets } from '../api/conditions';
  import { formatCount } from '../api/format';
  import { i18n, t } from '../lib/i18n.svelte';
  import HelpToggle from './HelpToggle.svelte';

  /**
   * What the library holds, beside the rules that read it, one card per axis: a rule written
   * against a value the library lacks matches nothing, which looks like a rule rightly matching
   * nothing. The bar carries the proportion, the figure the value.
   */
  let {
    facets,
    captionOf = () => undefined,
  }: {
    facets: LibraryFacets;
    /** A condition's caption from the catalogue, so a card names it in the reader's words. */
    captionOf?: (type: string) => string | undefined;
  } = $props();

  /**
   * Folded by default, and remembered per viewer: open above the rule table, it takes most of a
   * laptop screen. The folded summary still says how much it counts.
   */
  const STORE = 'routarr.facetsOpen';
  let open = $state(read());

  function read(): boolean {
    try {
      return localStorage.getItem(STORE) === '1';
    } catch {
      // Storage refused, as in a private window: folded, the default.
      return false;
    }
  }

  $effect(() => {
    try {
      localStorage.setItem(STORE, open ? '1' : '0');
    } catch {
      // Nothing to do: the preference is a convenience, not state.
    }
  });

  /** Each axis with the condition that reads it, so the caption is the answer. */
  const AXES: { key: FacetAxis; label: string; condition: string }[] = [
    { key: 'genres', label: 'FacetGenres', condition: 'genre_contains' },
    { key: 'original_languages', label: 'FacetLanguages', condition: 'original_language' },
    { key: 'origin_countries', label: 'FacetCountries', condition: 'origin_country' },
    { key: 'certifications', label: 'FacetCertifications', condition: 'certification_in' },
    { key: 'tags', label: 'FacetTags', condition: 'tag_in' },
    { key: 'series_types', label: 'FacetSeriesTypes', condition: 'series_type_is' },
    { key: 'root_folders', label: 'FacetRootFolders', condition: 'current_root_folder' },
  ];

  /** How many rows a card shows before it says how many it is holding back. */
  const SHOWN = 8;

  // Only axes carrying something. An empty card teaches nothing and the panel
  // exists to be read at a glance.
  const named = $derived(localFacets(facets, i18n.language));
  const cards = $derived(
    AXES.map((axis) => {
      // Named from the closed vocabulary of the two axes that have one: the counts say `ja`,
      // and only the vocabulary knows that is Japanese.
      const vocabulary = named.vocabularies[axis.key as keyof Vocabularies];
      const values = nameFacets(named[axis.key], vocabulary ?? []);
      return {
        ...axis,
        values: values.slice(0, SHOWN),
        hidden: Math.max(0, values.length - SHOWN),
        // Relative to the commonest value of this axis: each axis answers its own question.
        max: values[0]?.count ?? 1,
      };
    }).filter((axis) => axis.values.length > 0),
  );
</script>

<!-- The "?" lies over the summary's line rather than in it: a button inside a
     summary is a control nested in another, which a screen reader cannot
     reach on its own. -->
<div class="fold-wrap">
  <details class="card fold-card" bind:open>
    <summary class="card-header">
      <!-- A bare figure in parentheses, which agrees with no noun in any language. -->
      <h2 class="card-title" id="facets-title">
        {t('WithNote', {
          text: t('LibraryFacetsTitle'),
          note: formatCount(facets.total_media, i18n.language),
        })}
      </h2>
    </summary>

    {#if facets.without_metadata > 0}
      <!-- The count that explains a rule matching nothing for a reason no
         condition can express: those items carry nothing to match on. -->
      <div class="banner banner-warning">
        {t('FacetsWithoutMetadata', { count: facets.without_metadata })}
      </div>
    {/if}

    {#if cards.length === 0}
      <p class="text-muted">{t('FacetsEmpty')}</p>
    {:else}
      <div class="facet-grid">
        {#each cards as axis (axis.key)}
          <section class="facet-card">
            <header>
              <h3>{t(axis.label)}</h3>
              {#if captionOf(axis.condition)}
                <span class="text-muted text-sm">{captionOf(axis.condition)}</span>
              {/if}
            </header>
            {#each axis.values as facet (facet.value)}
              <div class="facet-row">
                <!-- Decorative: the figure beside it is what carries the value,
                   and it is the figure a screen reader reads. -->
                <span
                  class="facet-bar"
                  style="width: {Math.max(4, Math.round((facet.count / axis.max) * 100))}%"
                  aria-hidden="true"
                ></span>
                <span class="facet-value">
                  {facet.label ?? facet.value}
                </span>
                <span class="facet-count">{formatCount(facet.count, i18n.language)}</span>
              </div>
            {/each}
            {#if axis.hidden > 0}
              <div class="facet-more">{t('FacetsMore', { count: axis.hidden })}</div>
            {/if}
          </section>
        {/each}
      </div>
    {/if}
  </details>
  <span class="fold-help"><HelpToggle card="LibraryFacets" title="facets-title" /></span>
</div>
