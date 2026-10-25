<script lang="ts">
  import CardTitle from './CardTitle.svelte';
  import { CheckCircle2, Circle, ListChecks } from '../lib/icons';
  import { api } from '../api/client';
  import type { OnboardingState, OnboardingStatus } from '../api/types';
  import { STEP_GUIDES, guideProgress, requiredNumber } from '../api/onboarding';
  import { handFocus } from '../lib/focus';
  import type { Outcome } from '../lib/outcome.svelte';
  import { publishOnboarding } from '../lib/onboarding.svelte';
  import { href } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * The getting-started guide on the dashboard. Each step opens the screen that does it and
   * ticks itself from the data. Skipped, it shrinks to one line that offers it back until the
   * required steps are done.
   */
  let { status, outcome }: { status: OnboardingStatus; outcome: Outcome } = $props();

  const progress = $derived(guideProgress(status));
  let busy = false;

  /** A step done is history: folded into one line of names, the guide shrinks as it goes. */
  const done = $derived(status.steps.filter((step) => step.done));
  let showDone = $state(false);

  /**
   * Where the focus goes once a choice replaces the block that held the pressed button. No
   * button turns disabled while the choice is sent, since that drops the focus too: a second
   * press is ignored instead.
   */
  const LANDING: Record<OnboardingState, string> = {
    dismissed: 'guide-resume',
    pending: 'guide-title',
    done: 'main',
  };

  async function choose(next: OnboardingState) {
    if (busy) return;
    busy = true;
    try {
      publishOnboarding(await api.setOnboarding(next));
      outcome.clear();
    } catch (err) {
      outcome.fail(err);
      return;
    } finally {
      busy = false;
    }
    await handFocus(LANDING[next]);
  }
</script>

{#if status.state === 'pending'}
  <section class="card" aria-labelledby="guide-title">
    <div class="card-header">
      <div>
        <CardTitle card="GuideTitle" class="flex items-center gap-2" id="guide-title" tabindex={-1}>
          <ListChecks size={18} aria-hidden="true" />
          {t('GuideTitle')}
        </CardTitle>
        <p class="card-note">{t('GuideIntro')}</p>
      </div>
      <div class="guide-progress">
        <span class="guide-count"
          >{t('GuideProgress', { done: progress.done, total: progress.total })}</span
        >
        <!-- The sentence beside it carries the figure for assistive technology. -->
        <progress class="guide-bar" max={progress.total} value={progress.done} aria-hidden="true"
        ></progress>
      </div>
    </div>

    {#if done.length > 0}
      <div class="guide-done">
        <p class="flex-1">
          <span class="visually-hidden">{t('GuideStepDone')}</span>
          {#each done as step, index (step.id)}
            {#if index > 0}{t('ListSeparator')}{/if}<span class="guide-done-name"
              ><CheckCircle2 size={14} aria-hidden="true" />{t(STEP_GUIDES[step.id].titleKey)}</span
            >
          {/each}
        </p>
        <button
          type="button"
          class="btn btn-ghost btn-sm"
          aria-expanded={showDone}
          aria-controls="guide-steps"
          onclick={() => (showDone = !showDone)}
        >
          {t(showDone ? 'GuideHideDone' : 'GuideShowDone')}
        </button>
      </div>
    {/if}

    <ol class="guide-steps" id="guide-steps">
      {#each status.steps.filter((step) => showDone || !step.done) as step (step.id)}
        {@const guide = STEP_GUIDES[step.id]}
        {@const current = step.id === progress.current}
        <li class="guide-step" class:is-done={step.done} class:is-current={current}>
          <!-- Numbered like the banners count, required steps only: an optional
               step never holds the guide back, so it takes no place in the total. -->
          <span class="guide-step-mark" aria-hidden="true">
            {#if step.done}
              <CheckCircle2 size={20} />
            {:else if step.optional}
              <Circle size={20} />
            {:else}
              <span class="guide-step-number">{requiredNumber(status, step.id)}</span>
            {/if}
          </span>
          <div>
            <h3 class="guide-step-title">
              {t(guide.titleKey)}
              {#if step.optional}
                <span class="badge badge-value muted">{t('OptionalStep')}</span>
              {/if}
              {#if step.done}
                <span class="visually-hidden">{t('GuideStepDone')}</span>
              {:else if current}
                <span class="visually-hidden">{t('GuideStepNext')}</span>
              {/if}
            </h3>
            <p class="guide-step-text">{t(guide.textKey)}</p>
          </div>
          {#if !step.done}
            <a
              href={href(guide.to)}
              class="btn btn-sm guide-step-action {current ? 'btn-primary' : 'btn-secondary'}"
              >{t(guide.actionKey)}</a
            >
          {/if}
        </li>
      {/each}
    </ol>

    <div class="guide-footer">
      {#if status.complete}
        <p class="guide-complete">{t('GuideComplete')}</p>
        <button class="btn btn-primary" onclick={() => void choose('done')}>
          {t('GuideFinish')}
        </button>
      {:else}
        <button class="btn btn-ghost" onclick={() => void choose('dismissed')}>
          {t('GuideSkip')}
        </button>
      {/if}
    </div>
  </section>
{:else if status.state === 'dismissed' && !status.complete}
  <div class="card guide-paused">
    <ListChecks size={16} aria-hidden="true" />
    <p class="flex-1" id="guide-paused">
      {t('GuidePaused', { done: progress.done, total: progress.total })}
    </p>
    <button
      id="guide-resume"
      class="btn btn-secondary btn-sm"
      aria-describedby="guide-paused"
      onclick={() => void choose('pending')}
    >
      {t('GuideResume')}
    </button>
  </div>
{/if}
