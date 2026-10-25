import type { OnboardingStatus, OnboardingStep, Warning } from './types';

/** What the guide shows for a step, and the screen that does it. */
export interface StepGuide {
  titleKey: string;
  textKey: string;
  actionKey: string;
  /** A route, with the query or hash that opens the right dialog or section. */
  to: string;
}

/** Each step opens the real screen, since a second copy of its form would drift from the first. */
export const STEP_GUIDES: Record<OnboardingStep['id'], StepGuide> = {
  instance: {
    titleKey: 'GuideInstanceTitle',
    textKey: 'GuideInstanceText',
    actionKey: 'GuideInstanceAction',
    to: '/instances?add=1',
  },
  categories: {
    titleKey: 'GuideCategoriesTitle',
    textKey: 'GuideCategoriesText',
    actionKey: 'GuideCategoriesAction',
    to: '/categories',
  },
  metadata: {
    titleKey: 'GuideMetadataTitle',
    textKey: 'GuideMetadataText',
    actionKey: 'GuideMetadataAction',
    to: '/sources',
  },
  rule: {
    titleKey: 'GuideRuleTitle',
    textKey: 'GuideRuleText',
    actionKey: 'GuideRuleAction',
    to: '/rules#templates',
  },
  simulation: {
    titleKey: 'GuideSimulationTitle',
    textKey: 'GuideSimulationText',
    actionKey: 'GuideSimulationAction',
    to: '/simulation',
  },
  live: {
    titleKey: 'GuideLiveTitle',
    textKey: 'GuideLiveText',
    actionKey: 'GuideLiveAction',
    to: '/settings#guardrails',
  },
};

/**
 * How far the required steps are, and which one comes next. Optional steps never count, or a
 * ready installation would read "4 of 6".
 */
export function guideProgress(status: OnboardingStatus): {
  done: number;
  total: number;
  current: OnboardingStep['id'] | null;
} {
  const required = status.steps.filter((step) => !step.optional);
  return {
    done: required.filter((step) => step.done).length,
    total: required.length,
    current: required.find((step) => !step.done)?.id ?? null,
  };
}

/**
 * The step to offer after `id`: the next one not done, optional ones included so none is
 * skipped in silence. Past the end, a required step left behind, if any.
 */
export function nextAfter(
  status: OnboardingStatus,
  id: OnboardingStep['id'],
): OnboardingStep | null {
  const index = status.steps.findIndex((step) => step.id === id);
  const later = status.steps.slice(index + 1).find((step) => !step.done);
  return later ?? status.steps.find((step) => !step.done && !step.optional) ?? null;
}

/**
 * The open step that says `warning` itself, while the guide runs. Once the required steps are
 * done none does, since Finish may never be pressed.
 */
export function restatingStep(
  warning: Warning,
  status: OnboardingStatus | null,
): OnboardingStep['id'] | null {
  if (!status || status.state !== 'pending' || status.complete) return null;
  const step = status.steps.find((each) => each.id === warning.guide_step);
  return step && !step.done ? step.id : null;
}

/** The warnings the guide does not say itself, which the shell counts. */
export function outsideTheGuide(warnings: Warning[], status: OnboardingStatus | null): Warning[] {
  return warnings.filter((warning) => restatingStep(warning, status) === null);
}

/** The position of a required step among the required steps, from 1. */
export function requiredNumber(status: OnboardingStatus, id: OnboardingStep['id']): number {
  return status.steps.filter((step) => !step.optional).findIndex((step) => step.id === id) + 1;
}

/**
 * Read a one-shot flag such as `?add=1`, then take it out of the address, or a reload or the
 * back button would open its dialog again.
 */
export function takeQueryFlag(name: string): boolean {
  return takeQueryValue(name) !== null;
}

/** The value of `name` in the address, taken out of it as `takeQueryFlag` takes a flag. */
export function takeQueryValue(name: string): string | null {
  const params = new URLSearchParams(window.location.search);
  const value = params.get(name);
  if (value === null) return null;
  params.delete(name);
  const query = params.toString();
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`,
  );
  return value;
}
