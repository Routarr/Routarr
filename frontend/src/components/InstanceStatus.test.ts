import { describe, it, expect } from 'vitest';

import { renderWithI18n } from '../test/render';
import InstanceStatus from './InstanceStatus.svelte';

const badge = (status: string, detail: string | null = null) =>
  renderWithI18n(InstanceStatus, {
    props: { status, detail },
  }).container.querySelector('.badge');

describe('InstanceStatus', () => {
  /**
   * What it ran into is written out: a title shows to a mouse alone, and a
   * keyboard or a finger would read "error" and nothing else.
   */
  it('says an instance answers, or that it failed and what it ran into', () => {
    expect(badge('connected')?.className).toContain('badge-success');
    const failed = badge('error: Nothing answers', 'Nothing answers at the address');
    expect(failed?.className).toContain('badge-danger');
    expect(failed?.textContent).toBe('Error');
    expect(failed?.parentElement?.textContent).toContain('Nothing answers at the address');
  });

  it('draws a state that is not a verdict without an alarm colour', () => {
    for (const [status, text] of [
      ['unchecked', 'Checking…'],
      ['disabled', 'Disabled'],
    ]) {
      const quiet = badge(status!);
      expect(quiet?.className).toContain('badge-value');
      expect(quiet?.textContent).toBe(text);
    }
  });
});
