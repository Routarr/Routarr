<script lang="ts">
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
   * The getting-started guide, on the dashboard of an installation not set up yet.
   *
   * A list rather than a wizard: nothing is blocked, each step opens the screen
   * that does it, and every step ticks itself from the data, so leaving halfway
   * and coming back resumes where the installation is. Skipped, it shrinks to one
   * line that offers it back until the required steps are done.
   */
  let { status, outcome }: { status: OnboardingStatus; outcome: Outcome } = $props();

  const progress = $derived(guideProgress(status));
  let busy = false;

  /**
   * Each choice replaces the block that holds the pressed button, and the
   * focus with it, to the page, where a screen reader loses its place. It goes
   * to what takes the block's place: Resume, the guide's heading, or the
   * page's content once the guide is gone. No button is disabled while the
   * choice is sent, since a focused button that turns disabled drops the focus
   * as well: a second press is ignored instead.
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
        <h2 class="card-title flex items-center gap-2" id="guide-title" tabindex="-1">
          <ListChecks size={18} aria-hidden="true" />
          {t('GuideTitle')}
        </h2>
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

    <ol class="guide-steps">
      {#each status.steps as step (step.id)}
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
                <span class="badge badge-value muted">{t('Optional')}</span>
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
