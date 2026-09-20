<script lang="ts">
  import type { Component } from 'svelte';
  import Layout from './components/Layout.svelte';
  import Loading from './components/Loading.svelte';
  import { interceptLinks, router } from './lib/router.svelte';

  /**
   * One chunk per screen: as a single bundle, opening the dashboard would
   * download the rule builder, the log viewer and the settings form with it.
   * The chrome, the API client and the icons stay shared.
   */
  const ROUTES: Record<string, () => Promise<{ default: Component }>> = {
    '/': () => import('./pages/Dashboard.svelte'),
    '/instances': () => import('./pages/Instances.svelte'),
    '/root-folders': () => import('./pages/RootFolders.svelte'),
    '/rules': () => import('./pages/Rules.svelte'),
    '/rules/tests': () => import('./pages/RuleTests.svelte'),
    '/media': () => import('./pages/MediaExplorer.svelte'),
    '/simulation': () => import('./pages/Simulation.svelte'),
    '/history': () => import('./pages/History.svelte'),
    '/overrides': () => import('./pages/Overrides.svelte'),
    '/jobs': () => import('./pages/Jobs.svelte'),
    '/logs': () => import('./pages/Logs.svelte'),
    '/health': () => import('./pages/Health.svelte'),
    '/settings': () => import('./pages/Settings.svelte'),
  };

  $effect(() => interceptLinks());

  // An unknown path says so, inside the shell, with the address left alone.
  // Showing the dashboard — which is what this did — left somebody looking at a
  // screen they had not asked for, with no navigation entry marked active and
  // nothing explaining why. Every link here is written by this application, so
  // a miss is a typed address or an old bookmark, and both are worth correcting
  // rather than silently rewriting.
  const NOT_FOUND = () => import('./pages/NotFound.svelte');
  const load = $derived(ROUTES[router.path] ?? NOT_FOUND);
</script>

{#snippet page()}
  {#await load()}
    <Loading />
  {:then module}
    <module.default />
  {/await}
{/snippet}

<Layout children={page} />
