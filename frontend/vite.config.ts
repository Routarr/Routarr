/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  // Relative asset URLs, resolved against the `<base href>` the backend injects.
  // An absolute base would have to be chosen at build time, which would mean one
  // Docker image per mount point, the opposite of shipping a single image.
  base: './',

  plugins: [
    svelte({
      compilerOptions: { hmr: process.env.NODE_ENV !== 'production' },
    }),
  ],

  server: {
    port: 3000,
    proxy: {
      // With its slash: matched as a bare prefix, `/api` would also send the
      // backend a screen whose path starts with those letters.
      '/api/': {
        target: 'http://localhost:9876',
        changeOrigin: true,
        // `changeOrigin` rewrites `Host` to the backend's own, and the backend
        // refuses a write whose Origin names another host. `X-Forwarded-Host`
        // carries the host the browser addressed, which it compares first.
        xfwd: true,
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
        // Without this, Rollup folds the Svelte runtime and the icons into
        // whichever application chunk happens to pull them first, shipping
        // them as `ErrorBanner-<hash>.js`, which makes a network panel lie
        // about what is being downloaded. It also caches better: dependencies
        // change on a Dependabot schedule, screens change on every commit, and
        // a shared hash would let one edit invalidate both.
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
    // jsdom, not happy-dom, which does not drive `<select>` the way Svelte's
    // `bind:value` reads it: the option changes in the DOM, the bound variable
    // never does, and a filter test passes its click and then asserts against a
    // request that was never made.
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    globals: true,
    // No test is looked for in the build output, and `e2e/` belongs to
    // Playwright: its specs drive a real browser against a real server and
    // would hang here.
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
        // Type declarations, mirroring the backend payloads. There is nothing
        // to execute and counting them would flatter the total.
        'src/api/types.ts',
      ],
      reporter: ['text-summary', 'lcov'],
      // Floors, not targets, and not meant to be negotiated with on every
      // refactor. Raise one when the real figure moves up, never lower it to
      // make a build pass.
      // The aggregate sits at the whole figure below what the suite measures, as
      // the backend gate does. It catches a suite that stops running, and cannot
      // see a screen with no test, a small share of the whole. The per-file
      // floor sees that one: a file nothing renders measures nothing, and a
      // file whose functions nothing calls measures its declarations alone.
      thresholds: {
        statements: 94,
        branches: 83,
        functions: 92,
        lines: 94,
        perFile: { statements: 30, branches: 30, functions: 30, lines: 30 },
      },
    },
  },
});
