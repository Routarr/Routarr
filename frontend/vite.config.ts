/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  // Relative asset URLs, resolved against the `<base href>` the backend injects.
  // An absolute base would have to be chosen at build time, which would mean one
  // Docker image per mount point — the opposite of shipping a single image.
  base: './',

  plugins: [
    svelte({
      // Compile components for the browser in tests too, rather than rendering
      // them to a string: the suite asserts on what a user can reach — roles,
      // labels, what a click does — and none of that exists server-side.
      compilerOptions: { hmr: process.env.NODE_ENV !== 'production' },
    }),
  ],

  server: {
    port: 3000,
    proxy: {
      '/api': {
        target: 'http://localhost:9876',
        changeOrigin: true,
      },
    },
  },

  build: {
    outDir: 'dist',
    emptyOutDir: true,

    rollupOptions: {
      output: {
        // Everything from node_modules in one chunk of its own.
        //
        // Without this, Rollup folded the Svelte runtime and the icons into
        // whichever application chunk happened to pull them first — 55 kB
        // shipping as `ErrorBanner-<hash>.js`, which makes a network panel lie
        // about what is being downloaded. It also caches better: dependencies
        // change on a Dependabot schedule, screens change on every commit, and
        // a shared hash meant one edit invalidated both.
        manualChunks(id) {
          if (id.includes('node_modules')) return 'vendor';
        },
      },
    },
  },

  // Take Svelte's browser build, not its server one. Without this a component
  // under test renders to a string with no DOM behind it, and every query for a
  // role or a label finds nothing.
  resolve: {
    conditions: ['browser'],
  },

  test: {
    // jsdom, not happy-dom, which does not
    // drive `<select>` the way Svelte's `bind:value` reads it — the option
    // changes in the DOM, the bound variable never does, and a filter test
    // passes its click and then asserts against a request that was never made.
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    globals: true,
    // Keep production builds free of test files. `e2e/` belongs to Playwright:
    // its specs drive a real browser against a real server and would hang here.
    exclude: ['node_modules', 'dist', 'e2e'],
    coverage: {
      provider: 'v8',
      // `include` is what makes this measure the application rather than the
      // tested part of it: unless told the whole tree, v8 counts only the files
      // a test imported, reporting a high figure over a small fraction.
      include: ['src/**/*.{ts,svelte}'],
      exclude: [
        'src/**/*.test.{ts,svelte.ts}',
        'src/test/**',
        'src/main.ts',
        'src/vite-env.d.ts',
        // Type declarations, mirroring the backend payloads. There is nothing
        // to execute and counting them would flatter the total.
        'src/api/types.ts',
      ],
      reporter: ['text-summary', 'lcov'],
      // A floor, not a target. It catches a suite that stops running or a
      // screen added with no test at all; it is not meant to be negotiated with
      // on every refactor. Raise it when the real figure moves up, never lower
      // it to make a build pass.
      // Two points below what the suite actually measures — 82.3, 70.7,
      // 78.5 and 81.8 — which is the headroom the backend gate keeps at
      // 90 against 92.2. Enough that ordinary work does not trip it, not
      // so much that it stops guarding.
      thresholds: { statements: 80, branches: 68, functions: 76, lines: 80 },
    },
  },
});
