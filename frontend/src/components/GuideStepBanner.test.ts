import { describe, it, expect, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';

import { renderWithI18n } from '../test/render';
import { onboardingStatus } from '../test/fixtures';
import { publishOnboarding } from '../lib/onboarding.svelte';
import GuideStepBanner from './GuideStepBanner.svelte';

const STRINGS = {
  GuideStepBanner: 'Getting started, step {number} of {total}: {title}',
  GuideStepDoneNext: 'Step {number} of {total} is done. Next: {next}',
  GuideRequiredDone: 'Every required step is done.',
  GuideOptionalNext: 'Step {number} of {total} is done. Next, optional: {next}',
  GuideOptionalStepBanner: 'Getting started, optional step: {title}',
  GuideOptionalDoneNext: 'Optional step done. Next: {next}',
  GuideSkipStep: 'Skip this step',
  GuideMetadataTitle: 'Choose metadata sources',
  GuideMetadataAction: 'Open the sources',
  GuideRuleAction: 'Create a rule',
  GuideLiveTitle: 'Turn off the global dry-run',
  GuideLiveAction: 'Open the routing settings',
  GuideBack: 'Back to the guide',
  GuideInstanceTitle: 'Connect Radarr or Sonarr',
  GuideCategoriesTitle: 'Map root folders to categories',
  GuideCategoriesAction: 'Open categories and folders',
  GuideRuleTitle: 'Write a first rule',
  GuideSimulationAction: 'Open the simulation',
};

afterEach(() => publishOnboarding(null));

describe('GuideStepBanner', () => {
  it('says where the reader stands on the screen of the next step', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' }, strings: STRINGS });

    expect(
      screen.getByText('Getting started, step 2 of 4: Map root folders to categories'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Back to the guide' }).getAttribute('href')).toBe('/');
  });

  it('stays silent on the screen of a step that is not next', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'rule' }, strings: STRINGS });

    expect(screen.queryByRole('link', { name: 'Back to the guide' })).toBeNull();
  });

  /** The moment a step ticks is the moment the reader asks what comes next. */
  it('offers the next step on the screen of a step just done', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'instance' }, strings: STRINGS });

    expect(
      screen.getByText('Step 1 of 4 is done. Next: Map root folders to categories'),
    ).toBeTruthy();
    const next = screen.getByRole('link', { name: 'Open categories and folders' });
    expect(next.getAttribute('href')).toBe('/categories');
    // A banner's action, secondary: the screen keeps its one primary button.
    expect(next.classList.contains('btn-secondary')).toBe(true);
    expect(screen.getByRole('link', { name: 'Back to the guide' })).toBeTruthy();
  });

  it('sends the reader back to finish once no step is left ahead', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories', 'rule', 'simulation', 'live']));
    renderWithI18n(GuideStepBanner, { props: { step: 'simulation' }, strings: STRINGS });

    expect(screen.getByText('Every required step is done.')).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Open the simulation' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Back to the guide' })).toBeTruthy();
  });

  /** An optional step is offered in its turn, with a way past it. */
  it('offers an optional step next, and a way past it', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories']));
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' }, strings: STRINGS });

    expect(
      screen.getByText('Step 2 of 4 is done. Next, optional: Choose metadata sources'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Open the sources' }).getAttribute('href')).toBe(
      '/sources',
    );
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe(
      '/rules?new=1',
    );
  });

  it('says on an optional step that it is optional, with a way past it', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories']));
    renderWithI18n(GuideStepBanner, { props: { step: 'metadata' }, strings: STRINGS });

    expect(
      screen.getByText('Getting started, optional step: Choose metadata sources'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe(
      '/rules?new=1',
    );
  });

  it('leads on once an optional step is done', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories', 'metadata']));
    renderWithI18n(GuideStepBanner, { props: { step: 'metadata' }, strings: STRINGS });

    expect(screen.getByText('Optional step done. Next: Write a first rule')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Create a rule' })).toBeTruthy();
  });

  /** Past the last optional step there is only the guide, where Finish is. */
  it('skips the last optional step back to the guide', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories', 'rule', 'simulation']));
    renderWithI18n(GuideStepBanner, { props: { step: 'simulation' }, strings: STRINGS });

    expect(
      screen.getByText('Step 4 of 4 is done. Next, optional: Turn off the global dry-run'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe('/');
  });

  it('stays silent once the guide is skipped', () => {
    publishOnboarding(onboardingStatus(['instance'], { state: 'dismissed' }));
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' }, strings: STRINGS });

    expect(screen.queryByRole('link', { name: 'Back to the guide' })).toBeNull();
  });

  /**
   * The sentence saying a step is done is the one that tells the reader to
   * move on, so it is said in a region that exists before it changes: one
   * that arrives with its text announces nothing.
   */
  it('says a step turning done in the region its banner already held', () => {
    publishOnboarding(onboardingStatus());
    renderWithI18n(GuideStepBanner, { props: { step: 'instance' }, strings: STRINGS });
    const region = screen.getByRole('status');
    expect(region).toHaveTextContent('Getting started, step 1 of 4: Connect Radarr or Sonarr');

    publishOnboarding(onboardingStatus(['instance']));
    flushSync();

    expect(screen.getByRole('status')).toBe(region);
    expect(region).toHaveTextContent('Step 1 of 4 is done. Next: Map root folders to categories');
  });
});
