import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';
import type { Component } from 'svelte';

import { renderWithI18n } from './render';
import Applications from '../pages/Applications.svelte';
import History from '../pages/History.svelte';
import Instances from '../pages/Instances.svelte';
import Jobs from '../pages/Jobs.svelte';
import Logs from '../pages/Logs.svelte';
import MediaExplorer from '../pages/MediaExplorer.svelte';
import Overrides from '../pages/Overrides.svelte';
import RootFolders from '../pages/RootFolders.svelte';
import RuleTests from '../pages/RuleTests.svelte';
import Rules from '../pages/Rules.svelte';

/**
 * A list that failed to load is not an empty list. Under its error, "No
 * instance connected yet" sends the reader to add an instance that exists, and
 * "No rules yet" to import with Replace over rules they cannot see.
 *
 * No request is mocked: every one fails, and every key renders as itself.
 */
const SCREENS: [string, Component, string][] = [
  ['Applications', Applications as Component, 'NoApplications'],
  ['History', History as Component, 'NoDecisionRecorded'],
  ['Instances', Instances as Component, 'NoInstanceConfigured'],
  ['Jobs', Jobs as Component, 'NoTaskYet'],
  ['Logs', Logs as Component, 'NoWritesYet'],
  ['MediaExplorer', MediaExplorer as Component, 'NoMediaMatches'],
  ['Overrides', Overrides as Component, 'NoOverrides'],
  ['RootFolders', RootFolders as Component, 'NoRootFolderDiscovered'],
  ['RuleTests', RuleTests as Component, 'NoRuleTests'],
  ['Rules', Rules as Component, 'NoRulesYet'],
];

afterEach(() => vi.unstubAllGlobals());

describe('a list that failed to load', () => {
  it.each(SCREENS)('%s shows the failure and not its empty state', async (_, page, empty) => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));
    renderWithI18n(page);

    expect((await screen.findAllByRole('alert')).length).toBeGreaterThan(0);
    expect(screen.queryByText(empty)).toBeNull();
  });
});
