import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import ApiKeyGate from './ApiKeyGate.svelte';

/**
 * A key is generated at first start, so a browser without one is the *ordinary*
 * first visit. Mounting the shell behind it instead reads as a broken install:
 * a failing request per page, every page empty, and an "Unauthorized" badge
 * whose remedy the user would have to already know.
 */

const STRINGS = {
  ApiKeyRequired: 'This Routarr needs an API key',
  ApiKeyRejected: 'That key was refused',
  ApiKeyGeneratedFile: 'Saved as {file}',
  ApiKeyDockerCommand: 'With the Docker image:',
  ApiKeyChooseOwn: 'Or set {variable}',
  RoutarrApiKey: 'Routarr API key',
  SaveKey: 'Save key',
};

const reload = vi.fn();

beforeEach(() => {
  localStorage.clear();
  // `location.reload` would navigate for real; the assertion is that it is
  // called, not what it does.
  Object.defineProperty(window, 'location', {
    value: { ...window.location, reload },
    configurable: true,
  });
});

afterEach(() => vi.clearAllMocks());

const show = () => renderWithI18n(ApiKeyGate, { strings: STRINGS });

describe('ApiKeyGate', () => {
  it('names its field so the caption is more than decoration', async () => {
    show();

    expect(await screen.findByLabelText('Routarr API key')).toBeTruthy();
  });

  it('refuses to submit an empty key', async () => {
    show();

    const save = await screen.findByRole('button', { name: 'Save key' });
    expect((save as HTMLButtonElement).disabled).toBe(true);
  });

  it('stores the key and remounts the application', async () => {
    show();

    const field = await screen.findByLabelText('Routarr API key');
    await fireEvent.input(field, { target: { value: 'a-real-key' } });
    await fireEvent.submit(field.closest('form') as HTMLFormElement);

    expect(localStorage.getItem('routarr.apiKey')).toBe('a-real-key');
    // Every page in the tree has already fetched and failed; re-mounting them
    // all is exactly what a reload does.
    expect(reload).toHaveBeenCalled();
  });

  /**
   * A key is stored and the server refused it anyway: that is a *wrong* key,
   * not a missing one. Without saying so, pasting a bad key returns the same
   * blank screen and the user pastes it again.
   */
  it('distinguishes a refused key from a missing one', async () => {
    localStorage.setItem('routarr.apiKey', 'the-wrong-key');
    show();

    expect(await screen.findByText('That key was refused')).toBeTruthy();
  });

  it('says nothing about refusal when no key was ever stored', async () => {
    show();

    await screen.findByRole('button', { name: 'Save key' });
    expect(screen.queryByText('That key was refused')).toBeNull();
  });
});
