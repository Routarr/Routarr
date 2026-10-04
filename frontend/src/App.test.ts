import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from './test/render';
import { api } from './api/client';
import { onboardingStatus } from './test/fixtures';
import { navigate } from './lib/router.svelte';
import App from './App.svelte';

/**
 * A tab left open across an update asks for a screen's code under a name the
 * new image no longer serves. Blank, the screen gives no hint that a reload
 * fetches the new version.
 */
vi.mock('./pages/Logs.svelte', () => {
  throw new TypeError('Failed to fetch dynamically imported module');
});

afterEach(() => vi.restoreAllMocks());

describe('App', () => {
  it('reports a screen whose code no longer loads, and offers a reload', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue({ ui_theme: 'dark' });
    vi.spyOn(api, 'getOnboarding').mockResolvedValue(onboardingStatus([], { state: 'done' }));
    vi.spyOn(api, 'authMode').mockResolvedValue({
      mode: 'apikey',
      api_key_configured: true,
      api_key_pinned: false,
    });
    vi.spyOn(api, 'getStatus').mockResolvedValue({
      version: '0.1.0',
      dry_run: true,
      running_jobs: 0,
      pending_decisions: 0,
      failed_decisions: 0,
      warnings: [],
    });
    navigate('/move-log');
    renderWithI18n(App, {
      strings: { UnexpectedError: 'Unexpected error', ReloadPage: 'Reload the page' },
    });

    expect(await screen.findByRole('button', { name: 'Reload the page' })).toBeTruthy();
    expect(screen.getByText('Unexpected error')).toBeTruthy();
  });
});
