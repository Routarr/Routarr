import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import InstanceStatus from './InstanceStatus.svelte';

const STRINGS = {
  Checking: 'checking…',
  Connected: 'connected',
  Disabled: 'disabled',
  Error: 'error',
};

const badge = (status: string) =>
  renderWithI18n(InstanceStatus, { props: { status }, strings: STRINGS }).container.querySelector(
    '.badge',
  );

/** One badge for one question, on every screen that lists instances. */
describe('InstanceStatus', () => {
  /** The server's own words come from the network or the Arr, in English. */
  it('says an instance answers, or that it failed with what it ran into in the title', () => {
    expect(badge('connected')?.className).toContain('badge-success');
    const failed = badge('error: connection refused');
    expect(failed?.className).toContain('badge-danger');
    expect(failed?.textContent).toBe('error');
    expect(failed?.getAttribute('title')).toBe('connection refused');
  });

  /** Neither a probe still running nor a disabled instance is a failure. */
  it('draws a state that is not a verdict without an alarm colour', () => {
    for (const [status, text] of [
      ['unchecked', 'checking…'],
      ['disabled', 'disabled'],
    ]) {
      const quiet = badge(status!);
      expect(quiet?.className).toContain('badge-value');
      expect(quiet?.textContent).toBe(text);
    }
  });
});
