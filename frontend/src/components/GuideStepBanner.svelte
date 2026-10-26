<script lang="ts">
  import { CheckCircle2, ListChecks } from '../lib/icons';
  import type { OnboardingStep } from '../api/types';
  import {
    STEP_GUIDES,
    guideProgress,
    nextAfter,
    requiredNumber,
    type StepGuide,
  } from '../api/onboarding';
  import { doneThisSession, onboarding } from '../lib/onboarding.svelte';
  import { href } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * Where the reader stands in the guide, on the screen that does the step. A step just done
   * links to the next one, and an optional step offers a way past it rather than being skipped.
   * A step done before this session is history, and its screen is visited for other reasons.
   */
  let { step }: { step: OnboardingStep['id'] } = $props();

  type Shown =
    | { kind: 'current'; text: string }
    | { kind: 'optional'; text: string; skip: string }
    | { kind: 'done'; text: string; next: StepGuide; skip: string | null }
    | { kind: 'finished'; text: string };

  /** Where "skip" leads: the step after the one skipped, else the guide itself. */
  const skipPast = (id: OnboardingStep['id'], status: NonNullable<typeof onboarding.current>) => {
    const after = nextAfter(status, id);
    return after ? STEP_GUIDES[after.id].to : '/';
  };

  const shown = $derived.by((): Shown | null => {
    const status = onboarding.current;
    if (!status || status.state !== 'pending') return null;
    const own = status.steps.find((each) => each.id === step);
    if (!own) return null;
    const progress = guideProgress(status);
    const title = t(STEP_GUIDES[step].titleKey);
    const number = requiredNumber(status, step);

    if (!own.done) {
      if (own.optional) {
        return {
          kind: 'optional',
          text: t('GuideOptionalStepBanner', { title }),
          skip: skipPast(step, status),
        };
      }
      if (progress.current !== step) return null;
      return {
        kind: 'current',
        text: t('GuideStepBanner', { number, total: progress.total, title }),
      };
    }

    if (!doneThisSession(step)) return null;
    const next = nextAfter(status, step);
    if (!next) return { kind: 'finished', text: t('GuideRequiredDone') };
    const guide = STEP_GUIDES[next.id];
    const named = { number, total: progress.total, next: t(guide.titleKey) };
    const text = own.optional
      ? t('GuideOptionalDoneNext', named)
      : t(next.optional ? 'GuideOptionalNext' : 'GuideStepDoneNext', named);
    return {
      kind: 'done',
      text,
      next: guide,
      skip: next.optional ? skipPast(next.id, status) : null,
    };
  });
</script>

{#if shown}
  <div class="banner banner-guide">
    {#if shown.kind === 'done' || shown.kind === 'finished'}
      <CheckCircle2 size={16} aria-hidden="true" />
    {:else}
      <ListChecks size={16} aria-hidden="true" />
    {/if}
    <!-- Announces the turn to "done, next". A live region that arrives with its text says
         nothing. -->
    <span class="flex-1" role="status">{shown.text}</span>
    {#if shown.kind === 'done'}
      <!-- Secondary, as in every banner: the screen keeps its one primary button. -->
      <a href={href(shown.next.to)} class="btn btn-secondary btn-sm">{t(shown.next.actionKey)}</a>
      {#if shown.skip}
        <a href={href(shown.skip)} class="btn btn-secondary btn-sm">{t('GuideSkipStep')}</a>
      {/if}
    {:else if shown.kind === 'optional'}
      <a href={href(shown.skip)} class="btn btn-secondary btn-sm">{t('GuideSkipStep')}</a>
    {/if}
    <a href={href('/')} class="btn btn-secondary btn-sm">{t('GuideBack')}</a>
  </div>
{/if}
