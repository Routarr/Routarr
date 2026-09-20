import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import ErrorBanner from './ErrorBanner.svelte';

/**
 * The one component every failure passes through. It has to reach a screen
 * reader without being focused, and offer the way out when there is one.
 */

const STRINGS = { Dismiss: 'Dismiss', Retry: 'Retry' };

function render(props: Record<string, unknown>) {
  renderWithI18n(ErrorBanner, { props, strings: STRINGS });
}

describe('ErrorBanner', () => {
  it('is announced as an alert, so a failure is heard without being reached', () => {
    render({ message: 'Radarr refused the move' });

    expect(screen.getByRole('alert')).toHaveTextContent('Radarr refused the move');
  });

  it('renders nothing at all without a message', () => {
    render({ message: null });

    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('offers to retry only when told how', async () => {
    const onRetry = vi.fn();
    render({ message: 'Library unreachable', onRetry });

    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  /// A write that failed must not grow a button that does it again; retry is
  /// the caller's to offer, never the banner's to assume.
  it('shows no retry when none was offered', () => {
    render({ message: 'Rule rejected', onDismiss: () => {} });

    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument();
  });
});
