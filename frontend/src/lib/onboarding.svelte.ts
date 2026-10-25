import type { OnboardingStatus, OnboardingStep } from '../api/types';

/**
 * The getting-started guide as the shell last read it, one copy for every reader.
 *
 * `Layout` reads it again on every navigation and after `invalidateStatus()`.
 */
const state = $state<{ current: OnboardingStatus | null; failure: string | null }>({
  current: null,
  failure: null,
});

/**
 * The steps a read of this session found open: one done since is news, one done before not.
 * Plain, since it changes in the call that publishes the guide its readers derive from.
 */
let seenOpen: Partial<Record<OnboardingStep['id'], true>> = {};

export const onboarding = {
  get current(): OnboardingStatus | null {
    return state.current;
  },
  /** Why the last read failed, for the dashboard to say, or null. */
  get failure(): string | null {
    return state.failure;
  },
};

/** Whether a step was open at some read of this session, so its turning done is news. */
export function doneThisSession(id: OnboardingStep['id']): boolean {
  return seenOpen[id] === true;
}

/** Called by the shell after a read, and by a control after a write. Null forgets the session. */
export function publishOnboarding(next: OnboardingStatus | null): void {
  if (next === null) seenOpen = {};
  for (const step of next?.steps ?? []) if (!step.done) seenOpen[step.id] = true;
  state.current = next;
}

/** Called by the shell with the outcome of each read. */
export function publishOnboardingFailure(failure: string | null): void {
  state.failure = failure;
}
