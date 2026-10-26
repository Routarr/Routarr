import { describe, expect, it } from 'vitest';

import { health, healthInstance } from '../test/fixtures';
import { withProbedVerdicts } from './health';

describe('withProbedVerdicts', () => {
  const source = (connected: boolean | null, configured = true) => ({
    id: 'tmdb',
    display_name: 'TMDB',
    needs_key: true,
    configured,
    connected,
    reason: connected === false ? 'key_refused' : null,
  });
  const read = (status: string, connected: boolean | null, configured = true) =>
    health({
      instances: [healthInstance({ status, detail: status === 'connected' ? null : 'Refused' })],
      metadata: {
        providers: [source(connected, configured)],
        cached_items: 0,
        media_missing_metadata: 0,
      },
    });

  it('keeps what a probe found where the database read has no verdict', () => {
    const kept = withProbedVerdicts(read('unchecked', null), read('error: refused', false));

    expect(kept.instances[0]).toMatchObject({ status: 'error: refused', detail: 'Refused' });
    expect(kept.metadata.providers[0]).toMatchObject({ connected: false, reason: 'key_refused' });
  });

  it('takes the stored verdict where it has one, and drops one of a source changed since', () => {
    const kept = withProbedVerdicts(read('disabled', null, false), read('connected', true));

    expect(kept.instances[0]?.status).toBe('disabled');
    expect(kept.metadata.providers[0]?.connected).toBeNull();
  });
});
