import { describe, it, expect, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';

import { renderWithI18n } from '../test/render';
import { onboardingStatus } from '../test/fixtures';
import type { OnboardingStep } from '../api/types';
import { publishOnboarding } from '../lib/onboarding.svelte';
import GuideStepBanner from './GuideStepBanner.svelte';

afterEach(() => publishOnboarding(null));

/** The shell's reads as a step turns done during the session: open first, then done. */
function justDone(done: OnboardingStep['id'][], step: OnboardingStep['id']) {
  publishOnboarding(onboardingStatus(done.filter((each) => each !== step)));
  publishOnboarding(onboardingStatus(done));
}

describe('GuideStepBanner', () => {
  it('says where the reader stands on the screen of the next step', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' } });

    expect(
      screen.getByText('Getting started, step 2 of 4: Map root folders to categories'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Back to the guide' }).getAttribute('href')).toBe('/');
  });

  it('stays silent on the screen of a step that is not next', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'rule' } });

    expect(screen.queryByRole('link', { name: 'Back to the guide' })).toBeNull();
  });

  it('offers the next step on the screen of a step just done', () => {
    justDone(['instance'], 'instance');
    renderWithI18n(GuideStepBanner, { props: { step: 'instance' } });

    expect(
      screen.getByText('Step 1 of 4 is done. Next: Map root folders to categories'),
    ).toBeTruthy();
    const next = screen.getByRole('link', { name: 'Open categories and folders' });
    expect(next.getAttribute('href')).toBe('/categories');
    expect(next.classList.contains('btn-secondary')).toBe(true);
    expect(screen.getByRole('link', { name: 'Back to the guide' })).toBeTruthy();
  });

  /** A step done long ago is history, and its screen is visited for other reasons. */
  it('stays silent on the screen of a step done before this visit', () => {
    publishOnboarding(onboardingStatus(['instance']));
    renderWithI18n(GuideStepBanner, { props: { step: 'instance' } });

    expect(screen.queryByRole('link', { name: 'Back to the guide' })).toBeNull();
  });

  it('sends the reader back to finish once no step is left ahead', () => {
    justDone(['instance', 'categories', 'rule', 'simulation', 'live'], 'simulation');
    renderWithI18n(GuideStepBanner, { props: { step: 'simulation' } });

    expect(screen.getByText('Every required step is done.')).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Open the simulation' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Back to the guide' })).toBeTruthy();
  });

  it('offers an optional step next, and a way past it', () => {
    justDone(['instance', 'categories'], 'categories');
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' } });

    expect(
      screen.getByText('Step 2 of 4 is done. Next, optional: Choose metadata sources'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Open the sources' }).getAttribute('href')).toBe(
      '/sources',
    );
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe(
      '/rules#templates',
    );
  });

  it('says on an optional step that it is optional, with a way past it', () => {
    publishOnboarding(onboardingStatus(['instance', 'categories']));
    renderWithI18n(GuideStepBanner, { props: { step: 'metadata' } });

    expect(
      screen.getByText('Getting started, optional step: Choose metadata sources'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe(
      '/rules#templates',
    );
  });

  it('leads on once an optional step is done', () => {
    justDone(['instance', 'categories', 'metadata'], 'metadata');
    renderWithI18n(GuideStepBanner, { props: { step: 'metadata' } });

    expect(screen.getByText('Optional step done. Next: Add your first rule')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Open the templates' })).toBeTruthy();
  });

  /** Past the last optional step there is only the guide, where Finish is. */
  it('skips the last optional step back to the guide', () => {
    justDone(['instance', 'categories', 'rule', 'simulation'], 'simulation');
    renderWithI18n(GuideStepBanner, { props: { step: 'simulation' } });

    expect(
      screen.getByText('Step 4 of 4 is done. Next, optional: Turn off the global dry-run'),
    ).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Skip this step' }).getAttribute('href')).toBe('/');
  });

  it('leads from a step just done to a required step left open, not to going live', () => {
    justDone(['instance', 'rule', 'simulation'], 'rule');
    renderWithI18n(GuideStepBanner, { props: { step: 'rule' } });

    expect(
      screen.getByText('Step 3 of 4 is done. Next: Map root folders to categories'),
    ).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Open the guardrails' })).toBeNull();
  });

  it('stays silent once the guide is skipped', () => {
    publishOnboarding(onboardingStatus(['instance'], { state: 'dismissed' }));
    renderWithI18n(GuideStepBanner, { props: { step: 'categories' } });

    expect(screen.queryByRole('link', { name: 'Back to the guide' })).toBeNull();
  });

  it('says a step turning done in the region its banner already held', () => {
    publishOnboarding(onboardingStatus());
    renderWithI18n(GuideStepBanner, { props: { step: 'instance' } });
    const region = screen.getByRole('status');
    expect(region).toHaveTextContent('Getting started, step 1 of 4: Connect Radarr or Sonarr');

    publishOnboarding(onboardingStatus(['instance']));
    flushSync();

    expect(screen.getByRole('status')).toBe(region);
    expect(region).toHaveTextContent('Step 1 of 4 is done. Next: Map root folders to categories');
  });
});
