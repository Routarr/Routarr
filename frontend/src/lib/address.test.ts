import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { navigate, router } from './router.svelte';
import AddressHarness from '../test/AddressHarness.svelte';

const at = () => `${window.location.pathname}${window.location.search}${window.location.hash}`;

function open(address: string) {
  window.history.replaceState({}, '', address);
  router.path = window.location.pathname;
  render(AddressHarness);
}

beforeEach(() => window.history.replaceState({}, '', '/'));
afterEach(() => vi.restoreAllMocks());

describe('filters kept in the address', () => {
  it('opens on the filters a link holds, and drops a value the list does not offer', () => {
    open('/move-log?outcome=failure&search=Akira');
    expect(screen.getByLabelText('Outcome')).toHaveValue('failure');
    expect(screen.getByLabelText('Search')).toHaveValue('Akira');
    screen.getByText('Read 1');

    open('/move-log?outcome=everything');
    expect(screen.getAllByLabelText('Outcome').at(-1)).toHaveValue('');
  });

  /** Back leaves the screen: a filter is a change of view, not a place to return to. */
  it('writes each change in place, leaves the rest of the address and drops an emptied filter', async () => {
    open('/move-log?page=2#top');
    const push = vi.spyOn(window.history, 'pushState');

    await userEvent.selectOptions(screen.getByLabelText('Outcome'), 'failure');
    expect(at()).toBe('/move-log?page=2&outcome=failure#top');
    await userEvent.selectOptions(screen.getByLabelText('Outcome'), '');
    expect(at()).toBe('/move-log?page=2#top');
    expect(push).not.toHaveBeenCalled();
  });

  it('reads the address again when a link changes it on the screen on display', async () => {
    open('/move-log');
    await userEvent.type(screen.getByLabelText('Search'), 'x');

    navigate('/move-log?outcome=failure');

    expect(await screen.findByText('Read 2')).toBeInTheDocument();
    expect(screen.getByLabelText('Outcome')).toHaveValue('failure');
    expect(screen.getByLabelText('Search')).toHaveValue('');
    expect(at()).toBe('/move-log?outcome=failure');
  });

  it('writes nothing into the address of the screen that took its place', async () => {
    open('/move-log');
    window.history.pushState({}, '', '/library');

    await userEvent.selectOptions(screen.getByLabelText('Outcome'), 'failure');

    expect(at()).toBe('/library');
  });
});
