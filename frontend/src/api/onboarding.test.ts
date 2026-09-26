import { describe, it, expect, afterEach } from 'vitest';

import { guideProgress, nextAfter, requiredNumber, takeQueryFlag } from './onboarding';
import { onboardingStatus } from '../test/fixtures';

afterEach(() => window.history.replaceState({}, '', '/'));

describe('guideProgress', () => {
  it('counts the required steps only, since optional ones never hold the guide back', () => {
    const progress = guideProgress(onboardingStatus(['instance', 'metadata']));

    expect(progress).toEqual({ done: 1, total: 4, current: 'categories' });
  });

  it('skips an optional step when naming the next one', () => {
    expect(guideProgress(onboardingStatus(['instance', 'categories'])).current).toBe('rule');
  });

  it('names no next step once every required step is done', () => {
    const progress = guideProgress(
      onboardingStatus(['instance', 'categories', 'rule', 'simulation']),
    );

    expect(progress).toEqual({ done: 4, total: 4, current: null });
  });
});

describe('requiredNumber', () => {
  it('numbers a step among the required steps', () => {
    const status = onboardingStatus();

    expect(requiredNumber(status, 'instance')).toBe(1);
    expect(requiredNumber(status, 'rule')).toBe(3);
    expect(requiredNumber(status, 'simulation')).toBe(4);
  });
});

describe('nextAfter', () => {
  /** An optional step is offered in its turn, never jumped over in silence. */
  it('offers the next step in the list, optional or not', () => {
    const status = onboardingStatus(['instance', 'categories']);

    expect(nextAfter(status, 'categories')?.id).toBe('metadata');
    expect(nextAfter(status, 'metadata')?.id).toBe('rule');
  });

  it('passes over the steps already done', () => {
    expect(
      nextAfter(onboardingStatus(['instance', 'categories', 'metadata']), 'categories')?.id,
    ).toBe('rule');
  });

  it('comes back to a required step left behind, and to none once all are done', () => {
    const done = ['categories', 'rule', 'simulation', 'live'] as const;
    expect(nextAfter(onboardingStatus([...done]), 'live')?.id).toBe('instance');
    expect(nextAfter(onboardingStatus(['instance', ...done]), 'live')).toBeNull();
  });
});

describe('takeQueryFlag', () => {
  it('answers once, so a reload does not open the dialog again', () => {
    window.history.replaceState({}, '', '/instances?add=1&keep=yes#top');

    expect(takeQueryFlag('add')).toBe(true);
    expect(window.location.search).toBe('?keep=yes');
    expect(window.location.hash).toBe('#top');
    expect(takeQueryFlag('add')).toBe(false);
  });

  it('leaves an address without the flag alone', () => {
    window.history.replaceState({}, '', '/instances');

    expect(takeQueryFlag('add')).toBe(false);
    expect(window.location.pathname).toBe('/instances');
  });
});
