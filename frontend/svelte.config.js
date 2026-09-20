import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  // TypeScript in `<script lang="ts">`, and nothing else: no preprocessor for
  // CSS, because the whole stylesheet is one hand-written file of plain CSS.
  preprocess: vitePreprocess(),

  compilerOptions: {
    // Svelte 5 runes everywhere. Opting in explicitly rather than letting the
    // compiler infer per component keeps one reactivity model in the codebase
    // instead of two that look alike.
    runes: true,
  },
};
