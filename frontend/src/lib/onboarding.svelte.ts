import type { OnboardingStatus } from '../api/types';

/**
 * The getting-started guide as the shell last read it, one copy for every reader.
 *
 * `Layout` reads it again on every navigation and after `invalidateStatus()`.
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
