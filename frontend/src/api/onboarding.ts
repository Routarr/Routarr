import type { OnboardingStatus, OnboardingStep, Warning } from './types';

/** What the guide shows for a step, and the screen that does it. */
export interface StepGuide {
  titleKey: string;
  textKey: string;
  actionKey: string;
  /** A route, with the query or hash that opens the right dialog or section. */
  to: string;
}

/**
 * Each step points at the real screen, never at a form of its own: a second
 * copy of the instance or rule form would have to follow every change of the
 * first, validation included, and would drift the first time one of them moved.
 */
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
    to: '/rules?new=1',
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
 * How far the required steps are, and which one comes next.
 *
 * Optional steps never count: they cannot hold the guide back, so counting them
 * would show "4 of 6" on an installation that is ready.
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
 * The step to offer after `id`: the next one in the list not done yet,
 * optional or not, so an optional step is offered in its turn rather than
 * jumped over in silence. Past the end of the list, a required step left
 * behind, if any.
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
 * The warnings the guide does not say itself.
 *
 * While it runs, a warning that restates an open step is that step's, and its
 * banner and the dashboard list say it already. Every other warning shows, as
 * an Arr that stopped answering, which no step brings back. Once every
 * required step is done nothing is held back: Finish is a click the reader may
 * never make, and an optional step may stay open for good.
 */
export function outsideTheGuide(warnings: Warning[], status: OnboardingStatus | null): Warning[] {
  if (!status || status.state !== 'pending' || status.complete) return warnings;
  const open = status.steps.filter((step) => !step.done).map((step) => step.id);
  return warnings.filter((warning) => !warning.guide_step || !open.includes(warning.guide_step));
}

/** The position of a required step among the required steps, from 1. */
export function requiredNumber(status: OnboardingStatus, id: OnboardingStep['id']): number {
  return status.steps.filter((step) => !step.optional).findIndex((step) => step.id === id) + 1;
}

/**
 * Read a one-shot request carried in the address, then take it out.
 *
 * A link from the guide opens a dialog with `?add=1`. Left in the address, a
 * reload or the back button would open the dialog again, long after the reader
 * closed it.
 */
export function takeQueryFlag(name: string): boolean {
  const params = new URLSearchParams(window.location.search);
  if (!params.has(name)) return false;
  params.delete(name);
  const query = params.toString();
  window.history.replaceState(
    window.history.state,
    '',
    `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`,
  );
  return true;
}
