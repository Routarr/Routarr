<script lang="ts">
  import { href, isCurrent } from '../lib/router.svelte';
  import { formatCount } from '../api/format';
  import { i18n, t } from '../lib/i18n.svelte';
  import { access } from '../lib/access.svelte';
  import { groupsFor, type Counts } from '../lib/navigation';
  import { HelpCircle, LogOut, X } from '../lib/icons';

  /** Amber asks for attention, red says something failed, the rest is just a
      number, so a count is never louder than what it counts. */
  const TONE: Record<keyof Counts, string> = {
    jobs: '',
    decisions: '',
    failed: 'is-critical',
    warnings: 'is-warning',
  };

  /** The sentence the badge replaces, kept as the accessible name. */
  const BADGE_LABEL: Record<keyof Counts, string> = {
    jobs: 'TasksRunning',
    decisions: 'MovesAwaitingReview',
    failed: 'FailedMoves',
    warnings: 'DiagnosticWarnings',
  };

  let {
    open = false,
    offstage = false,
    onNavigate,
    onClose,
    counts = { jobs: 0, decisions: 0, failed: 0, warnings: 0 },
    version,
    onAbout,
    onHelp,
    onSignOut,
  }: {
    open?: boolean;
    /** Out of sight, a closed drawer: nothing in it may take the focus. */
    offstage?: boolean;
    onNavigate?: () => void;
    /**
     * Closes the drawer from inside it. The open drawer makes the page inert,
     * its toggle included, and a screen reader's swipe sends no Escape.
     */
    onClose?: () => void;
    counts?: Counts;
    version?: string;
    /** Opens what Routarr is: its licence and the credits its sources ask for. */
    onAbout?: () => void;
    /** Given on a phone, whose bar has no room for Help: the drawer's foot offers it. */
    onHelp?: () => void;
    /** Given on a phone where a session can be ended, as `onHelp` is. */
    onSignOut?: () => void;
  } = $props();
</script>

<aside id="sidebar" class="sidebar{open ? ' is-open' : ''}" inert={offstage}>
  <div class="sidebar-header">
    <svg
      class="sidebar-logo"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2.5"
    >
      <path d="M6 3v12" />
      <path d="M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
      <path d="M6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />
      <path d="M15 6a9 9 0 0 0-9 9" />
    </svg>
    <span class="sidebar-title">Routarr</span>
    <button
      type="button"
      class="close-button sidebar-close"
      aria-label={t('Dismiss')}
      title={t('Dismiss')}
      onclick={onClose}
    >
      <X size={18} aria-hidden="true" />
    </button>
  </div>

  <nav class="sidebar-nav" aria-label={t('MainNavigation')}>
    {#each groupsFor(access.readOnly) as group, index (group.key ?? 'home')}
      <!-- The label is hidden in the rail, where 68px fits no word. The rule
           above the group is what survives, so the grouping still reads. -->
      {#if group.key}
        <p class="nav-group" id="nav-group-{index}">{t(group.key)}</p>
      {/if}
      <ul class="nav-list" aria-labelledby={group.key ? `nav-group-${index}` : undefined}>
        {#each group.items as item (item.to)}
          {@const active = isCurrent(item.to)}
          <li>
            <a
              href={href(item.to)}
              class="nav-item {active ? 'active' : ''}"
              aria-current={active ? 'page' : undefined}
              onclick={onNavigate}
              title={t(item.key)}
            >
              <!-- The rail hides the label from sight alone: it still names
                   the link, and the title shows it to a pointer. -->
              <item.icon size={18} />
              <span class="nav-label">{t(item.key)}</span>
              <!-- In the rail the badge alone says something waits here: the figure is for the
                   eye and the sentence for the reader, since ARIA refuses `aria-label` on a
                   `<span>`. -->
              {#if item.badge && counts[item.badge] > 0}
                {@const count = counts[item.badge]}
                <span class="nav-badge {TONE[item.badge]}" aria-hidden="true"
                  >{formatCount(count, i18n.language)}</span
                >
                <span class="visually-hidden">{t(BADGE_LABEL[item.badge], { count })}</span>
              {/if}
            </a>
          </li>
        {/each}
      </ul>
    {/each}
  </nav>

  {#if onHelp || onSignOut}
    <div class="sidebar-tools">
      {#if onHelp}
        <button type="button" class="btn btn-ghost btn-sm" onclick={onHelp} aria-keyshortcuts="?">
          <HelpCircle size={14} aria-hidden="true" />
          {t('Help')}
        </button>
      {/if}
      {#if onSignOut}
        <button type="button" class="btn btn-ghost btn-sm" onclick={onSignOut}>
          <LogOut size={14} aria-hidden="true" />
          {t('SignOut')}
        </button>
      {/if}
    </div>
  {/if}

  <!-- The version at the foot, out of the bar, which says what a click will do. -->
  {#if version}
    <button type="button" class="sidebar-foot" title={t('AboutRoutarr')} onclick={onAbout}>
      <!-- The name goes where the labels go: 68px of rail fits the number and
           nothing else. -->
      <span class="foot-name">Routarr</span> v{version}
    </button>
  {/if}
</aside>
