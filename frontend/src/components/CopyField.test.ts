import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, waitFor } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import CopyField from './CopyField.svelte';

const VALUE = 'http://nas:9876/api/v1/webhook/5be07b9c-75f8-4d32-a1e8-4a2c90d1e6f3';

function clipboard(writeText?: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, 'clipboard', {
    value: writeText ? { writeText } : undefined,
    configurable: true,
  });
}

async function copy() {
  renderWithI18n(CopyField, { props: { value: VALUE, label: 'Webhook URL' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Copy – Webhook URL' }));
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  window.getSelection()?.removeAllRanges();
});

describe('CopyField', () => {
  it('shows the whole value and copies it, saying so for a few seconds', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const writeText = vi.fn(() => Promise.resolve());
    clipboard(writeText);

    await copy();

    expect(screen.getByText(VALUE)).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith(VALUE);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Copied'));
    vi.advanceTimersByTime(5000);
    await waitFor(() => expect(screen.getByRole('status').textContent).toBe(''));
  });

  /** A homelab often serves plain http, where the browser keeps the clipboard closed. */
  it.each([
    ['closed', undefined],
    ['refusing', () => Promise.reject(new DOMException('denied', 'NotAllowedError'))],
  ])(
    'selects the value for the keyboard when the clipboard is %s, and claims no copy',
    async (_, writeText) => {
      clipboard(writeText);

      await copy();

      await waitFor(() =>
        expect(screen.getByRole('status')).toHaveTextContent('Press Ctrl+C to copy.'),
      );
      expect(window.getSelection()?.toString()).toBe(VALUE);
    },
  );

  it('names the shortcut of a Mac on a Mac', async () => {
    clipboard();
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel');

    await copy();

    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Press ⌘C to copy.'));
  });
});
