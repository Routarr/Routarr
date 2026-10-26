import type { Health } from './types';

/**
 * `stored`, a read of the database alone, with the verdicts of `probed` where it has none: it
 * reads every enabled instance `unchecked` and every source `connected: null`.
 */
export function withProbedVerdicts(stored: Health, probed: Health): Health {
  const instances = stored.instances.map((instance) => {
    const found = probed.instances.find((each) => each.id === instance.id);
    if (instance.status !== 'unchecked' || !found) return instance;
    return { ...instance, status: found.status, detail: found.detail, version: found.version };
  });
  const providers = stored.metadata.providers.map((source) => {
    const found = probed.metadata.providers.find((each) => each.id === source.id);
    if (source.connected !== null || found?.configured !== source.configured) return source;
    return { ...source, connected: found.connected, reason: found.reason };
  });
  return { ...stored, instances, metadata: { ...stored.metadata, providers } };
}
