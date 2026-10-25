import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import ErrorBanner from './ErrorBanner.svelte';

function render(props: Record<string, unknown>) {
  renderWithI18n(ErrorBanner, { props });
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

  it('shows no retry when none was offered', () => {
    render({ message: 'Rule rejected', onDismiss: () => {} });

    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument();
  });
});
