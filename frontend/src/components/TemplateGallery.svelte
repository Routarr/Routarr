<script lang="ts">
  import type { Component } from 'svelte';
  import CardTitle from './CardTitle.svelte';
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
  import { href } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /** The templates the Rules screen offers, each opening the dialog that adds it. */
  let {
    templates,
    onChoose,
  }: { templates: TemplateSummary[]; onChoose: (template: TemplateSummary) => void } = $props();

  const ICONS: Record<string, Component<{ size?: number }>> = {
    '4k': MonitorPlay,
    concerts: MicVocal,
    documentaries: Telescope,
    children: Baby,
    anime: Sparkles,
    'foreign-films': Languages,
    requesters: UsersRound,
  };

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

<section class="card" id="templates" aria-labelledby="templates-title">
  <div class="card-header">
    <CardTitle card="Templates" id="templates-title" tabindex={-1}>{t('Templates')}</CardTitle>
  </div>
  <p class="text-muted text-base">{t('TemplatesLead')}</p>
  <ul class="template-grid">
    {#each templates as template (template.id)}
      {@const Icon = ICONS[template.id] ?? LayoutTemplate}
      <li class="template-card">
        <div class="template-card-head">
          <span class="template-icon" aria-hidden="true"><Icon size={18} /></span>
          <h3>{template.name}</h3>
          {#if template.added_version !== null}
            <span class="badge badge-success">{t('TemplateAddedBadge')}</span>
          {/if}
        </div>
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
        <div class="template-card-foot">
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
</section>
