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
  import { onboarding } from '../lib/onboarding.svelte';
  import { href } from '../lib/router.svelte';
  import { t } from '../lib/i18n.svelte';

  /**
   * Where the reader stands in the guide, on the screen that does a step.
   *
   * Shown only there: on every screen it would be noise, and on none a reader
   * sent here by the guide would not know how to get back to it. Once the step
   * is done it leads straight to the next one, since that is the question the
   * tick raises, and a detour through the dashboard is one more click for it.
   * An optional step is offered in its turn with a way past it, never jumped
   * over in silence.
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
    <span class="flex-1">{shown.text}</span>
    {#if shown.kind === 'done'}
      <!-- Secondary, as every banner's action is: the screen keeps its one
           primary button, and the guide's own card is where its step leads. -->
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
