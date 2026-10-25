<script lang="ts">
  import type { Component } from 'svelte';
  import Layout from './components/Layout.svelte';
  import Loading from './components/Loading.svelte';
  import PageFailure from './components/PageFailure.svelte';
  import { interceptLinks, router } from './lib/router.svelte';

  /** One chunk per screen, so a first visit downloads the shell and the screen it opens. */
  const ROUTES: Record<string, () => Promise<{ default: Component }>> = {
    '/': () => import('./pages/Dashboard.svelte'),
    '/instances': () => import('./pages/Instances.svelte'),
    '/categories': () => import('./pages/Categories.svelte'),
    '/rules': () => import('./pages/Rules.svelte'),
    '/rule-tests': () => import('./pages/RuleTests.svelte'),
    '/library': () => import('./pages/Library.svelte'),
    '/simulation': () => import('./pages/Simulation.svelte'),
    '/history': () => import('./pages/History.svelte'),
    '/exceptions': () => import('./pages/Exceptions.svelte'),
    '/tasks': () => import('./pages/Tasks.svelte'),
    '/move-log': () => import('./pages/MoveLog.svelte'),
    '/security-log': () => import('./pages/SecurityLog.svelte'),
    '/diagnostics': () => import('./pages/Diagnostics.svelte'),
    '/applications': () => import('./pages/Applications.svelte'),
    '/reference': () => import('./pages/ApiReference.svelte'),
    '/sources': () => import('./pages/Sources.svelte'),
    '/settings': () => import('./pages/Settings.svelte'),
  };

  $effect(() => interceptLinks());

  // An unknown path says so inside the shell and keeps the address, so it can be corrected.
  const NOT_FOUND = () => import('./pages/NotFound.svelte');
  const load = $derived(ROUTES[router.path] ?? NOT_FOUND);
</script>

{#snippet page()}
  {#await load()}
    <Loading />
  {:then module}
    <module.default />
  {:catch error}
    <PageFailure {error} />
  {/await}
{/snippet}

<Layout children={page} />
