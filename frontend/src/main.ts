import { mount } from 'svelte';
import App from './App.svelte';
import { startDictionary } from './lib/i18n.svelte';
import './index.css';

/**
 * Before the first render: every screen reads the dictionary through `t()`, and mounting first
 * paints one frame of raw keys.
 */
await startDictionary();

mount(App, { target: document.getElementById('root')! });
