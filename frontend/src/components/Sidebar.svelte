<script lang="ts">
  import { href, isCurrent } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';
  import { GROUPS, type Counts } from '../lib/navigation';

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
    decisions: 'DecisionsAwaitingReview',
    failed: 'FailedMoves',
    warnings: 'DiagnosticWarnings',
  };

  let {
    open = false,
    offstage = false,
    onNavigate,
    counts = { jobs: 0, decisions: 0, failed: 0, warnings: 0 },
    version,
    onAbout,
  }: {
    open?: boolean;
    /** Out of sight, a closed drawer: nothing in it may take the focus. */
    offstage?: boolean;
    onNavigate?: () => void;
    counts?: Counts;
    version?: string;
    /** Opens what Routarr is: its licence and the credits its sources ask for. */
    onAbout?: () => void;
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
  </div>

  <nav class="sidebar-nav" aria-label={t('MainNavigation')}>
    {#each GROUPS as group, index (group.key ?? 'home')}
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
              <!-- The count of what waits behind this entry. In the rail,
                   where the label is hidden, the badge is the only thing that
                   says something is waiting, so it carries the sentence as its
                   accessible name. -->
              {#if item.badge && counts[item.badge] > 0}
                {@const count = counts[item.badge]}
                <!-- The figure for the eye, the sentence for the reader. An
                     `aria-label` on a `<span>` names a generic element, which
                     ARIA prohibits and readers honour unevenly. -->
                <span class="nav-badge {TONE[item.badge]}" aria-hidden="true">{count}</span>
                <span class="visually-hidden">{t(BADGE_LABEL[item.badge], { count })}</span>
              {/if}
            </a>
          </li>
        {/each}
      </ul>
    {/each}
  </nav>

  <!-- The foot of the navigation, which is where an operations tool puts its
       version. It answers no question anyone asks while working, so it stays
       out of the bar, where the two things that say what a click will do
       sit. -->
  {#if version}
    <button type="button" class="sidebar-foot" title={t('AboutRoutarr')} onclick={onAbout}>
      <!-- The name goes where the labels go: 68px of rail fits the number and
           nothing else. -->
      <span class="foot-name">Routarr</span> v{version}
    </button>
  {/if}
</aside>
