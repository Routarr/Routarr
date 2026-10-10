import { mount } from 'svelte';
import App from './App.svelte';
import { startDictionary } from './lib/i18n.svelte';
import './index.css';

/**
 * The dictionary is in place before the first render rather than alongside it.
 *
 * Every screen reads it synchronously through `t()`, so mounting first would
 * paint one frame of raw keys, `RulesEngine` where a heading belongs, before
 * the strings arrived.
 */
await startDictionary();

mount(App, { target: document.getElementById('root')! });
