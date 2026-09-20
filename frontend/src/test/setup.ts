import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

import { confirmation, settle } from '../lib/confirm.svelte';
import { navigate } from '../lib/router.svelte';

// Module-level state the components share, put back after every test: a
// confirmation left pending by a test that failed before answering it, or a
// route another test navigated to, otherwise reaches the next test in the file.
afterEach(() => {
  cleanup();
  localStorage.clear();
  if (confirmation.request) settle(null);
  navigate('/', { replace: true });
});
