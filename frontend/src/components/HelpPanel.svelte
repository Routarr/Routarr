<script lang="ts">
  import { untrack } from 'svelte';
  import { i18n, t } from '../lib/i18n.svelte';
  import { href } from '../lib/router.svelte';
  import { screenKey } from '../lib/routes';
  import { CARD_HELP, HELP, TERMS, glossary, helpScreen, searchHelp } from '../lib/help';
  import Modal from './Modal.svelte';
  import SearchField from './SearchField.svelte';

  /**
   * The help, beside the screen it explains: what the screen is for, its
   * terms, the problems met there with the screen that fixes each, and what
   * each of its cards shows. A search runs through the help of every screen,
   * and the glossary lists every term with the screens that use it.
   *
   * A dialog rather than a panel left open beside the page: it is read, then
   * closed, and a modal one keeps the focus in it and gives it back.
   */
  let {
    path,
    screen,
    onClose,
  }: {
    /** The address on display, whose screen the help opens on. */
    path: string;
    /** A screen to open on instead, as a search of the quick search names it. */
    screen?: string;
    onClose: () => void;
  } = $props();

  let shown = $state(untrack(() => helpScreen(screen ?? path)));
  let view = $state<'screen' | 'glossary'>('screen');
  let query = $state('');

  const help = $derived(HELP[shown]!);
  const hits = $derived(query.trim() ? searchHelp(query, t) : []);
  const terms = $derived(view === 'glossary' ? glossary(t, i18n.language) : []);

  function show(target: string) {
    shown = target;
    view = 'screen';
    query = '';
  }

  const TABS = ['screen', 'glossary'] as const;
  /** The arrow keys move between the tabs, as a tab list is walked. */
  function walk(event: KeyboardEvent) {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') return;
    event.preventDefault();
    view = view === 'screen' ? 'glossary' : 'screen';
    document.getElementById(`help-tab-${view}`)?.focus();
  }
</script>

<Modal
  label={t('HelpTitle', { screen: t(screenKey(shown)) })}
  {onClose}
  maxWidth={480}
  closeOnBackdrop
  initialFocus="help-search"
>
  <div class="help-panel">
    <div class="modal-header">
      <h2 class="modal-title">{t('Help')}</h2>
      <button
        type="button"
        class="btn btn-secondary btn-sm"
        onclick={onClose}
        aria-label={t('Dismiss')}
        title={t('Dismiss')}
      >
        ✕
      </button>
    </div>

    <SearchField
      id="help-search"
      bind:value={query}
      label={t('HelpSearchLabel')}
      placeholder={t('HelpSearchPlaceholder')}
    />

    {#if query.trim()}
      <p class="visually-hidden" role="status">{t('HelpResults', { count: hits.length })}</p>
      {#if hits.length === 0}
        <p class="help-empty">{t('HelpNoResults', { query: query.trim() })}</p>
      {:else}
        <ul class="help-hits">
          {#each hits as hit, at (at)}
            <li>
              <button type="button" class="help-hit" onclick={() => show(hit.screen)}>
                <span class="help-hit-title">{hit.title}</span>
                <span class="help-hit-where">{t(screenKey(hit.screen))}</span>
                <span class="help-hit-text">{hit.text}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else}
      <div class="help-tabs" role="tablist" aria-label={t('Help')}>
        {#each TABS as tab (tab)}
          <button
            type="button"
            role="tab"
            id="help-tab-{tab}"
            aria-selected={view === tab}
            aria-controls="help-view"
            tabindex={view === tab ? 0 : -1}
            onclick={() => (view = tab)}
            onkeydown={walk}
          >
            {t(tab === 'screen' ? 'HelpThisScreen' : 'HelpGlossary')}
          </button>
        {/each}
      </div>

      <!-- Focusable, as a tab panel whose first content is text: the keyboard
           otherwise jumps from the tabs straight past what they show. -->
      <div
        class="help-view"
        id="help-view"
        role="tabpanel"
        aria-labelledby="help-tab-{view}"
        tabindex="0"
      >
        {#if view === 'screen'}
          <h3>{t(screenKey(shown))}</h3>
          <p>{t(help.purpose)}</p>

          <h4>{t('HelpTerms')}</h4>
          <dl class="help-terms">
            {#each help.terms as id (id)}
              <dt>{t(TERMS[id]!.name)}</dt>
              <dd>{t(TERMS[id]!.text)}</dd>
            {/each}
          </dl>

          <h4>{t('HelpProblems')}</h4>
          <ul class="help-problems">
            {#each help.problems as problem (problem.problem)}
              <li>
                <strong>{t(problem.problem)}</strong>
                <span>{t(problem.fix)}</span>
                {#if problem.to}
                  <a class="text-link" href={href(problem.to)} onclick={onClose}
                    >{t('HelpFixOn', { screen: t(screenKey(problem.to)) })}</a
                  >
                {/if}
              </li>
            {/each}
          </ul>

          {#if help.cards.length > 0}
            <h4>{t('HelpCards')}</h4>
            <dl class="help-terms">
              {#each help.cards as id (id)}
                <dt>{t(CARD_HELP[id]!.title)}</dt>
                <dd>{t(CARD_HELP[id]!.text)}</dd>
              {/each}
            </dl>
          {/if}
        {:else}
          <dl class="help-terms">
            {#each terms as term (term.id)}
              <dt>{term.name}</dt>
              <dd>
                {term.text}
                <span class="help-used">
                  {t('HelpUsedOn')}
                  {#each term.screens as used, at (used)}
                    {#if at > 0}{t('ListSeparator')}{/if}<button
                      type="button"
                      class="help-link"
                      onclick={() => show(used)}>{t(screenKey(used))}</button
                    >
                  {/each}
                </span>
              </dd>
            {/each}
          </dl>
        {/if}
      </div>
    {/if}
  </div>
</Modal>
