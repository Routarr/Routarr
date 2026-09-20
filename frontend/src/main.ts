import { mount } from 'svelte';
import App from './App.svelte';
import { loadDictionary } from './lib/i18n.svelte';
import './index.css';

/**
 * The dictionary is fetched before the first render rather than alongside it.
 *
 * Every screen reads it synchronously through `t()`, so mounting first would
 * paint one frame of raw keys — `SimulationTitle` where a heading belongs —
 * before the strings arrived. It is one request against the same origin, and it
 * cannot fail in a way that blocks: `loadDictionary` swallows a failure and
 * leaves keys rendering as themselves, which is ugly but navigable.
 */
await loadDictionary();

mount(App, { target: document.getElementById('root')! });
