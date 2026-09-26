import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import { onboardingStatus } from '../test/fixtures';
import type { OnboardingStatus } from '../api/types';
import { createOutcome } from '../lib/outcome.svelte';
import { onboarding, publishOnboarding } from '../lib/onboarding.svelte';
import GettingStarted from './GettingStarted.svelte';

const STRINGS = {
  GuideTitle: 'Getting started',
  GuideIntro: 'A few steps',
  GuideProgress: 'Required steps done: {done} of {total}',
  GuideStepDone: 'Done',
  GuideStepNext: 'Next step',
  Optional: 'optional',
  GuideInstanceTitle: 'Connect Radarr or Sonarr',
  GuideInstanceAction: 'Add an instance',
  GuideCategoriesTitle: 'Map root folders to categories',
  GuideCategoriesAction: 'Open root folders',
  GuideMetadataTitle: 'Choose metadata sources',
  GuideMetadataAction: 'Open the sources',
  GuideRuleTitle: 'Write a first rule',
  GuideRuleAction: 'Create a rule',
  GuideSimulationTitle: 'Run a simulation',
  GuideSimulationAction: 'Open the simulation',
  GuideLiveTitle: 'Leave test mode',
  GuideLiveAction: 'Open the routing settings',
  GuideComplete: 'Routarr is set up.',
  GuideFinish: 'Finish',
  GuideSkip: 'Skip the guide',
  GuidePaused: 'The setup is not finished. Required steps done: {done} of {total}',
  GuideResume: 'Resume the guide',
};

function show(status: OnboardingStatus) {
  const outcome = createOutcome();
  renderWithI18n(GettingStarted, { props: { status, outcome }, strings: STRINGS });
  return outcome;
}

afterEach(() => {
  vi.restoreAllMocks();
  publishOnboarding(null);
});

describe('GettingStarted', () => {
  it('lists every step, marks what is done and names the next one', () => {
    show(onboardingStatus(['instance']));

    expect(screen.getByText('Required steps done: 1 of 4')).toBeTruthy();
    const steps = screen.getAllByRole('listitem');
    expect(steps).toHaveLength(6);
    expect(within(steps[0] as HTMLElement).getByText('Done')).toBeTruthy();
    expect(within(steps[1] as HTMLElement).getByText('Next step')).toBeTruthy();
    // Optional steps say so, and never count toward the total above.
    expect(within(steps[2] as HTMLElement).getByText('optional')).toBeTruthy();
    expect(within(steps[5] as HTMLElement).getByText('optional')).toBeTruthy();
  });

  /** The banners say "step 3 of 4": the list shows which one that is. */
  it('numbers the required steps, and only them', () => {
    show(onboardingStatus(['instance']));

    // A done step shows its tick, an optional one no number.
    const numbers = [...document.querySelectorAll('.guide-step-number')].map((mark) =>
      mark.textContent?.trim(),
    );
    expect(numbers).toEqual(['2', '3', '4']);
  });

  it('points each open step at the screen that does it, the next one first', () => {
    show(onboardingStatus(['instance']));

    // A done step offers nothing to do.
    expect(screen.queryByRole('link', { name: 'Add an instance' })).toBeNull();
    const next = screen.getByRole('link', { name: 'Open root folders' });
    expect(next.getAttribute('href')).toBe('/root-folders');
    expect(next.classList.contains('btn-primary')).toBe(true);
    expect(screen.getByRole('link', { name: 'Create a rule' }).getAttribute('href')).toBe(
      '/rules?new=1',
    );
    expect(screen.getByRole('link', { name: 'Open the sources' }).getAttribute('href')).toBe(
      '/settings#metadata',
    );
  });

  it('opens the instance dialog from the first step', () => {
    show(onboardingStatus());

    expect(screen.getByRole('link', { name: 'Add an instance' }).getAttribute('href')).toBe(
      '/instances?add=1',
    );
  });

  it('can be skipped, and the shell hears about it at once', async () => {
    const skipped = onboardingStatus([], { state: 'dismissed' });
    const set = vi.spyOn(api, 'setOnboarding').mockResolvedValue(skipped);
    show(onboardingStatus());

    await fireEvent.click(screen.getByRole('button', { name: 'Skip the guide' }));

    await waitFor(() => expect(set).toHaveBeenCalledWith('dismissed'));
    await waitFor(() => expect(onboarding.current).toEqual(skipped));
  });

  it('offers to finish once the required steps are done, optional ones left open', async () => {
    const set = vi
      .spyOn(api, 'setOnboarding')
      .mockResolvedValue(onboardingStatus([], { state: 'done' }));
    show(onboardingStatus(['instance', 'categories', 'rule', 'simulation']));

    expect(screen.getByText('Routarr is set up.')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Skip the guide' })).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Finish' }));

    await waitFor(() => expect(set).toHaveBeenCalledWith('done'));
  });

  it('shrinks to one line when skipped, and offers itself back', async () => {
    const set = vi.spyOn(api, 'setOnboarding').mockResolvedValue(onboardingStatus(['instance']));
    show(onboardingStatus(['instance'], { state: 'dismissed' }));

    expect(screen.queryByRole('list')).toBeNull();
    expect(screen.getByText('The setup is not finished. Required steps done: 1 of 4')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Resume the guide' }));

    await waitFor(() => expect(set).toHaveBeenCalledWith('pending'));
  });

  it('says nothing once it is done, or once a skipped setup is complete', () => {
    show(onboardingStatus([], { state: 'done' }));
    expect(screen.queryByText('Getting started')).toBeNull();

    show(
      onboardingStatus(['instance', 'categories', 'rule', 'simulation'], { state: 'dismissed' }),
    );
    expect(screen.queryByRole('button', { name: 'Resume the guide' })).toBeNull();
  });

  it('reports a refused choice through the screen outcome', async () => {
    vi.spyOn(api, 'setOnboarding').mockRejectedValue(
      new ApiError('The settings could not be written', 409, 'conflict'),
    );
    const outcome = show(onboardingStatus());

    await fireEvent.click(screen.getByRole('button', { name: 'Skip the guide' }));

    await waitFor(() => expect(outcome.error).toBe('The settings could not be written'));
  });
});
