import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { nthCall } from '../test/spy';
import { renderWithI18n } from '../test/render';
import { paginated } from '../test/fixtures';
import { captureDownloads } from '../test/downloads';
import { api } from '../api/client';
import type { SecurityEvent } from '../api/types';
import SecurityLog from './SecurityLog.svelte';

/**
 * Who signed in, who was refused, and who changed a key or a setting. The
 * server sends what happened as a key and its parameters, so the sentence is
 * read in the reader's language: rendered as sent, the screen would show
 * `AuditSignedIn`.
 */

const STRINGS = {
  SecurityEvents: 'Security events',
  NoSecurityEvents: 'No event recorded yet.',
  SearchNameOrAddress: 'Search a name or an address',
  FilterByEvent: 'Filter by event',
  FilterByOutcome: 'Filter by outcome',
  AllEvents: 'All events',
  AllOutcomes: 'All outcomes',
  SecurityOutcomeAllowed: 'Allowed',
  SecurityOutcomeRefused: 'Refused',
  SecurityKindSignIn: 'Sign-in',
  SecurityKindApiKey: 'API key',
  SecurityEventRepeated: 'Sent again within the minute: {count}',
  AuditSignedIn: 'Signed in with the password.',
  AuditApiKeyRefused: 'A request was sent with a key that opens nothing.',
  AuditSessionEnded: 'The session {handle} was ended.',
  ExportCsv: 'Export CSV',
  None: '-',
};

function event(over: Partial<SecurityEvent> = {}): SecurityEvent {
  return {
    id: 1,
    at: '2026-10-07 09:00:00',
    kind: 'sign_in',
    outcome: 'allowed',
    subject: 'admin',
    client: '192.168.1.40',
    message: 'AuditSignedIn',
    params: {},
    repeated: 0,
    ...over,
  };
}

const show = () => renderWithI18n(SecurityLog, { strings: STRINGS });

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Security log', () => {
  it("says what happened in the reader's words, with who and from where", async () => {
    vi.spyOn(api, 'getSecurityLog').mockResolvedValue(
      paginated([
        event({ id: 1 }),
        event({
          id: 2,
          kind: 'api_key',
          outcome: 'refused',
          subject: null,
          client: '203.0.113.9',
          message: 'AuditApiKeyRefused',
          repeated: 41,
        }),
        event({ id: 3, message: 'AuditSessionEnded', params: { handle: 'ab12' } }),
      ]),
    );
    show();

    const signedIn = (await screen.findByText('Signed in with the password.')).closest('tr')!;
    expect(within(signedIn).getByText('Sign-in')).toBeTruthy();
    expect(within(signedIn).getByText('Allowed')).toBeTruthy();
    expect(within(signedIn).getByText('admin')).toBeTruthy();
    expect(within(signedIn).getByText('192.168.1.40')).toBeTruthy();

    const refused = screen
      .getByText('A request was sent with a key that opens nothing.')
      .closest('tr')!;
    expect(within(refused).getByText('API key')).toBeTruthy();
    expect(within(refused).getByText('Refused')).toBeTruthy();
    expect(within(refused).getByText('Sent again within the minute: 41')).toBeTruthy();

    expect(screen.getByText('The session ab12 was ended.')).toBeTruthy();
  });

  it('says nothing was recorded rather than showing an empty table', async () => {
    vi.spyOn(api, 'getSecurityLog').mockResolvedValue(paginated([]));
    show();

    expect(await screen.findByText('No event recorded yet.')).toBeTruthy();
  });

  /** The file holds what the screen shows: every filter goes with it. */
  it('narrows to an event, an outcome and an address, and exports what it shows', async () => {
    const list = vi.spyOn(api, 'getSecurityLog').mockResolvedValue(paginated([event()]));
    const exportLog = vi.spyOn(api, 'exportSecurityLog').mockResolvedValue(new Blob(['a,b']));
    const saved = captureDownloads();
    show();

    await userEvent.type(await screen.findByLabelText('Search a name or an address'), '203.0');
    await userEvent.selectOptions(screen.getByLabelText('Filter by event'), 'api_key');
    await userEvent.selectOptions(screen.getByLabelText('Filter by outcome'), 'refused');
    const narrowed = { search: '203.0', kind: 'api_key', outcome: 'refused', page: 1 };
    await waitFor(() =>
      expect(list).toHaveBeenLastCalledWith(expect.objectContaining(narrowed), expect.anything()),
    );
    await userEvent.click(screen.getByRole('button', { name: /export csv/i }));

    await waitFor(() => expect(saved).toHaveLength(1));
    expect(nthCall(exportLog)[0]).toMatchObject(narrowed);
    expect(saved[0]?.name).toBe('routarr-security-log.csv');
  });
});
