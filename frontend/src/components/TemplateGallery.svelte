<script lang="ts">
  import type { Component, Snippet } from 'svelte';
  import CardTitle from './CardTitle.svelte';
  import HelpToggle from './HelpToggle.svelte';
  import type { TemplateSummary } from '../api/types';
  import {
    AlertTriangle,
    Baby,
    FolderInput,
    Info,
    Languages,
    LayoutTemplate,
    MicVocal,
    MonitorPlay,
    Sparkles,
    Telescope,
    UsersRound,
  } from '../lib/icons';
  import { formatCount } from '../api/format';
  import { href } from '../lib/router.svelte';
  import { i18n, t } from '../lib/i18n.svelte';

  /**
   * The templates the Rules screen offers, one row each, each opening the dialog that adds it.
   * In place of the empty table while no rule is written, folded below the table once one is.
   */
  let {
    templates,
    title,
    lead,
    onChoose,
    foldable = false,
    open = $bindable(false),
    children,
  }: {
    templates: TemplateSummary[];
    title: string;
    lead: string;
    onChoose: (template: TemplateSummary) => void;
    /** Folded to its title, which counts the templates, until it is opened. */
    foldable?: boolean;
    open?: boolean;
    /** What follows the rows, as the empty screen's way to write a rule instead. */
    children?: Snippet;
  } = $props();

  const ICONS: Record<string, Component<{ size?: number }>> = {
    '4k': MonitorPlay,
    concerts: MicVocal,
    documentaries: Telescope,
    children: Baby,
    anime: Sparkles,
    'foreign-films': Languages,
    requesters: UsersRound,
  };

  /** The title of either form, which the screen hands the focus to. */
  const TITLE = 'templates-title';

  // Requesters are added person by person: the template is updated, not replaced.
  const actionOf = (template: TemplateSummary) =>
    t(
      template.added_version === null
        ? 'TemplateAdd'
        : template.parameter === 'requesters'
          ? 'TemplateUpdate'
          : 'TemplateReplace',
    );
</script>

{#snippet rows()}
  <p class="text-muted text-base">{lead}</p>
  <ul class="template-list">
    {#each templates as template (template.id)}
      {@const Icon = ICONS[template.id] ?? LayoutTemplate}
      <li class="template-row">
        <span class="template-icon" aria-hidden="true"><Icon size={18} /></span>
        <div class="template-row-text">
          <h3>
            {template.name}
            {#if template.added_version !== null}
              <span class="badge badge-success">{t('TemplateAddedBadge')}</span>
            {/if}
          </h3>
          <p class="template-card-text">{template.description}</p>
          {#if template.needs.length > 0}
            <p class="template-note is-warning">
              <AlertTriangle size={14} aria-hidden="true" />
              <span>
                {t('TemplateNeeds', {
                  sources: template.needs.map((source) => source.name).join(t('ListSeparator')),
                })}
                <a class="text-link" href={href('/sources')}>{t('TemplateNeedsLink')}</a>
              </span>
            </p>
          {/if}
          {#if template.parameter === 'requesters' && template.requesters.length === 0}
            <p class="template-note">
              <Info size={14} aria-hidden="true" />
              <span>{t('TemplateRequestersEmpty')}</span>
            </p>
          {/if}
          {#if template.parameter === 'quality_profiles' && template.quality_profiles.length === 0}
            <p class="template-note">
              <Info size={14} aria-hidden="true" />
              <span>{t('TemplateQualityProfilesEmpty')}</span>
            </p>
          {/if}
        </div>
        <div class="template-row-foot">
          {#if template.category !== null}
            <span class="template-target">
              <FolderInput size={14} aria-hidden="true" />
              <span class="visually-hidden">{t('Category')}</span>
              <span>{template.category}</span>
            </span>
          {/if}
          <button
            class="btn btn-secondary btn-sm"
            aria-label="{actionOf(template)} – {template.name}"
            onclick={() => onChoose(template)}
          >
            {actionOf(template)}
          </button>
        </div>
      </li>
    {/each}
  </ul>
  {@render children?.()}
{/snippet}

<!-- The "?" lies beside a summary, since a button inside one is a control inside another. -->
<div id="templates" class={foldable ? 'fold-wrap' : undefined}>
  {#if foldable}
    <details class="card fold-card" bind:open>
      <summary class="card-header">
        <h2 class="card-title" id={TITLE} tabindex="-1">
          {t('WithNote', { text: title, note: formatCount(templates.length, i18n.language) })}
        </h2>
      </summary>
      {@render rows()}
    </details>
    <span class="fold-help"><HelpToggle card="Templates" title={TITLE} /></span>
  {:else}
    <section class="card" aria-labelledby={TITLE}>
      <div class="card-header">
        <CardTitle card="Templates" id={TITLE} tabindex={-1}>{title}</CardTitle>
      </div>
      {@render rows()}
    </section>
  {/if}
</div>
