<script lang="ts">
  import type { FacetAxis, LibraryFacets, Vocabularies } from '../api/types';
  import { nameFacets } from '../api/conditions';
  import { t } from '../lib/i18n.svelte';

  /**
   * What the library actually holds, beside the rules that read it.
   *
   * Without it, writing a rule means guessing which genres and languages the
   * library holds — and a rule written against a value it does not carry matches
   * nothing, which reads on screen exactly like a rule that correctly matches
   * nothing.
   *
   * Small multiples, one card per axis: the bar carries the proportion, the
   * figure carries the value. Rows of identical pills would give a value seen 27
   * times and one seen once the same weight, and put the only number that
   * matters in the smallest element on screen.
   */
  let { facets }: { facets: LibraryFacets } = $props();

  /**
   * Folded by default, and it remembers.
   *
   * This is reference data consulted *while* writing a rule, and it sat above
   * the rule table taking 428px of a 1440px screen, 76% of a 768px one and a
   * screen and a half of a phone — so the screen whose subject is the rule
   * list opened on something else. Folded it is one click away, and the
   * summary still says how much library it is counting.
   */
  const STORE = 'routarr.facetsOpen';
  let open = $state(read());

  function read(): boolean {
    try {
      return localStorage.getItem(STORE) === '1';
    } catch {
      // A private window, or a browser told to block site data: the panel is
      // simply folded, which is the default anyway.
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
  const cards = $derived(
    AXES.map((axis) => {
      // Named from the closed vocabulary where there is one: the counts say
      // `ja`, and only the vocabulary knows that is Japanese.
      // Two of the seven axes have a closed vocabulary; the index is partial
      // by design. `facets[axis.key]` needs no assertion at all now that
      // `AXES` is typed by the payload.
      const vocabulary = facets.vocabularies[axis.key as keyof Vocabularies];
      const values = nameFacets(facets[axis.key], vocabulary ?? []);
      return {
        ...axis,
        values: values.slice(0, SHOWN),
        hidden: Math.max(0, values.length - SHOWN),
        // Relative to the commonest value in *this* axis, not to the library:
        // a language seen 30 times and a certification seen 20 are each the top
        // of their own question, and scaling them together would flatten both.
        max: values[0]?.count ?? 1,
      };
    }).filter((axis) => axis.values.length > 0),
  );
</script>

<details class="card facet-panel" bind:open>
  <!-- A native disclosure: the keyboard handling, the toggle semantics and the
       announced state come from the browser rather than from three hand-written
       approximations. -->
  <summary class="card-header">
    <!-- The count belongs to the heading, not beside it: two elements for one
         phrase left a bureaucratic caption floating next to a title. A bare
         figure in parentheses because the number would otherwise have to agree
         with a noun — wrong at one, in every language that inflects. -->
    <h2 class="card-title">{t('InYourLibrary', { count: facets.total_media })}</h2>
  </summary>

  {#if facets.without_metadata > 0}
    <!-- The count that explains a rule matching nothing for a reason no
         condition can express: those items carry nothing to match on. -->
    <div class="banner banner-warning">
      {t('FacetsWithoutMetadata', { count: facets.without_metadata })}
    </div>
  {/if}

  {#if cards.length === 0}
    <p class="muted">{t('FacetsEmpty')}</p>
  {:else}
    <div class="facet-grid">
      {#each cards as axis (axis.key)}
        <section class="facet-card">
          <header>
            <h3>{t(axis.label)}</h3>
            <span class="mono">{axis.condition}</span>
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
              <!-- The title carries whichever text is on screen, since that is
                   the one being truncated — and a name always contains the raw
                   value, which is what a rule is written against. -->
              <span class="facet-value" title={facet.label ?? facet.value}>
                {facet.label ?? facet.value}
              </span>
              <span class="facet-count">{facet.count}</span>
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
