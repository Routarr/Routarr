import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

import { publishAccess } from '../lib/access.svelte';
import { confirmation, settle } from '../lib/confirm.svelte';
import { navigate } from '../lib/router.svelte';

// jsdom lays nothing out, so its `scrollTo` scrolls nothing and prints "not
// implemented" on every call. The router scrolls on each navigation, the reset
// below included, and hundreds of those lines bury a real warning in the run.
window.scrollTo = () => {};

// Nor does it resize anything, and it has no `ResizeObserver` to say so. A test
// that needs a size to change stubs one of its own.
window.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

// Module-level state the components share, put back after every test: a
// confirmation left pending by a test that failed before answering it, a route
// another test navigated to, or a reader another test signed in, otherwise
// reaches the next test in the file.
afterEach(() => {
  cleanup();
  localStorage.clear();
  if (confirmation.request) settle(null);
  navigate('/');
  publishAccess(null);
});
