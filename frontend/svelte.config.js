import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  // TypeScript only: the stylesheet is plain CSS.
  preprocess: vitePreprocess(),

  compilerOptions: {
    // Runes in every component, never inferred per component.
    runes: true,
  },
};
