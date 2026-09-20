import tseslint from 'typescript-eslint';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import svelteConfig from './svelte.config.js';

// A short, type-aware rule set beside `svelte-check`. The compiler already
// reports types, unused symbols and accessibility; this catches what a type
// checker cannot: a promise dropped on the floor, a `console.log` left in a
// screen, an import that should have been a type import.
export default tseslint.config(
  { ignores: ['dist/', 'coverage/', 'node_modules/', 'playwright-report/', 'test-results/'] },

  ...svelte.configs.recommended,

  {
    files: ['**/*.ts', '**/*.svelte'],
    plugins: { '@typescript-eslint': tseslint.plugin },
    languageOptions: {
      parserOptions: {
        // The Playwright suite has a tsconfig of its own; the one file outside
        // both is the runner's configuration.
        projectService: { allowDefaultProject: ['playwright.config.ts'] },
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: ['.svelte'],
      },
    },
    rules: {
      '@typescript-eslint/no-floating-promises': 'error',
      '@typescript-eslint/no-misused-promises': 'error',
      '@typescript-eslint/await-thenable': 'error',
      '@typescript-eslint/consistent-type-imports': ['error', { fixStyle: 'inline-type-imports' }],
      '@typescript-eslint/no-import-type-side-effects': 'error',
      'no-duplicate-imports': 'error',
      eqeqeq: ['error', 'always', { null: 'ignore' }],
      'no-debugger': 'error',
      'no-console': ['error', { allow: ['warn', 'error'] }],
    },
  },

  // `.svelte.ts` keeps the Svelte parser the recommended set gave it: its
  // rune rules read the file through that parser and no other.
  {
    files: ['**/*.ts'],
    ignores: ['**/*.svelte.ts'],
    languageOptions: { parser: tseslint.parser },
  },

  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      parserOptions: { parser: tseslint.parser, svelteConfig },
    },
  },

  // Node, not the browser: the Playwright suite and the build configuration
  // print on purpose.
  {
    files: ['e2e/**/*.ts', 'playwright.config.ts', 'vite.config.ts'],
    languageOptions: { globals: globals.node },
    rules: { 'no-console': 'off' },
  },
);
