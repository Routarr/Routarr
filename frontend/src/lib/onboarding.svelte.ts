import type { OnboardingStatus } from '../api/types';

/**
 * The getting-started guide as the shell last read it.
 *
 * `Layout` owns the request, re-run on every navigation and whenever a screen
 * says the warnings changed, and publishes the answer here. The dashboard, the
 * top bar and each step's screen read this one copy, so the pill, the list and
 * a step's banner never disagree. A module-level rune for the reason the status
 * revision is one: there is one shell, and every reader means the same guide.
 */
const state = $state<{ current: OnboardingStatus | null; failure: string | null }>({
  current: null,
  failure: null,
});

export const onboarding = {
  get current(): OnboardingStatus | null {
    return state.current;
  },
  /** Why the last read failed, for the dashboard to say, or null. */
  get failure(): string | null {
    return state.failure;
  },
};

/** Called by the shell after a read, and by a control after a write. */
export function publishOnboarding(next: OnboardingStatus | null): void {
  state.current = next;
}

/** Called by the shell with the outcome of each read. */
export function publishOnboardingFailure(failure: string | null): void {
  state.failure = failure;
}
