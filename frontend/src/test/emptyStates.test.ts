import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen } from '@testing-library/svelte';
import type { Component } from 'svelte';

import { ENGLISH, renderWithI18n } from './render';
import Applications from '../pages/Applications.svelte';
import Categories from '../pages/Categories.svelte';
import Exceptions from '../pages/Exceptions.svelte';
import History from '../pages/History.svelte';
import Instances from '../pages/Instances.svelte';
import Library from '../pages/Library.svelte';
import MoveLog from '../pages/MoveLog.svelte';
import RuleTests from '../pages/RuleTests.svelte';
import Rules from '../pages/Rules.svelte';
import Tasks from '../pages/Tasks.svelte';

/**
 * A list that failed to load is not an empty list: under the error, its empty state would send
 * the reader to add or import what may already exist.
 */
const SCREENS: [string, Component, string][] = [
  ['Applications', Applications as Component, 'NoApplications'],
  ['Categories', Categories as Component, 'NoRootFolderDiscovered'],
  ['Exceptions', Exceptions as Component, 'NoOverrides'],
  ['History', History as Component, 'NoDecisionRecorded'],
  ['Instances', Instances as Component, 'NoInstanceConfigured'],
  ['Library', Library as Component, 'NoMediaMatches'],
  ['MoveLog', MoveLog as Component, 'NoWritesYet'],
  ['RuleTests', RuleTests as Component, 'NoRuleTests'],
  ['Rules', Rules as Component, 'NoRulesYet'],
  ['Tasks', Tasks as Component, 'NoTaskYet'],
];

afterEach(() => vi.unstubAllGlobals());

describe('a list that failed to load', () => {
  it.each(SCREENS)('%s shows the failure and not its empty state', async (_, page, empty) => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Failed to fetch')));
    renderWithI18n(page);

    expect((await screen.findAllByRole('alert')).length).toBeGreaterThan(0);
    // The sentence a reader would see, which the page renders in English here.
    const sentence = ENGLISH[empty];
    expect(sentence).toBeTruthy();
    expect(document.body.textContent).not.toContain(sentence);
  });
});
