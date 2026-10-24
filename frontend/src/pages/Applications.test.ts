import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { nthCall } from '../test/spy';
import { ApiError, api } from '../api/client';
import type { Application, NewApplication } from '../api/types';
import Applications from './Applications.svelte';

function application(over: Partial<Application> = {}): Application {
  return {
    id: 'k1',
    name: 'n8n',
    scopes: [],
    may_confirm: [],
    may_move_files: false,
    created_at: '2026-09-30 10:00:00',
    created_by: null,
    last_used_at: new Date().toISOString(),
    expires_at: null,
    expired: false,
    ...over,
  };
}

function show(listed: Application[]) {
  vi.spyOn(api, 'getApplications').mockResolvedValue(listed);
  // A mode that asks no proof: the proof has its own tests (`lib/proof.test.ts`).
  vi.spyOn(api, 'authMode').mockResolvedValue({
    mode: 'none',
    api_key_configured: false,
    api_key_pinned: false,
  });
  return renderWithI18n(Applications);
}

afterEach(() => vi.restoreAllMocks());

describe('Applications', () => {
  it('says there are none rather than showing an empty table', async () => {
    show([]);
    expect(await screen.findByText('No application has a key yet.')).toBeTruthy();
  });

  it('reads what each key allows, read included, and what it may answer', async () => {
    show([
      application({
        scopes: ['operate'],
        may_confirm: ['batch', 'capacity'],
        may_move_files: true,
      }),
    ]);

    const row = (await screen.findByText('n8n')).closest('tr') as HTMLElement;
    expect(within(row).getByText('Read')).toBeTruthy();
    expect(within(row).getByText('Operate')).toBeTruthy();
    expect(within(row).queryByText('Write')).toBeNull();
    expect(
      within(row).getByText('Apply a whole simulation, Not enough room at the destination'),
    ).toBeTruthy();
    expect(within(row).getByText('Yes')).toBeTruthy();
    expect(within(row).getByText('Never')).toBeTruthy();
  });

  it('names who created a key under its creation date', async () => {
    show([application({ created_by: 'owner' })]);

    const row = (await screen.findByText('n8n')).closest('tr') as HTMLElement;
    const made = within(row).getByText('owner');
    expect(made.closest('td')?.classList.contains('cell-timestamp')).toBe(true);
  });

  /** An expired key is greyed until it is revoked. */
  it('marks a key that expires soon, an expired one and one left unused', async () => {
    const day = 86_400_000;
    const at = (offset: number) => new Date(Date.now() + offset).toISOString();
    show([
      application({ id: 'k1', name: 'soon', expires_at: at(3 * day) }),
      application({ id: 'k2', name: 'gone', expires_at: at(-day), expired: true }),
      application({ id: 'k3', name: 'idle', last_used_at: at(-91 * day) }),
      application({ id: 'k4', name: 'fine', expires_at: at(60 * day) }),
    ]);

    const row = async (name: string) =>
      (await screen.findByText(name)).closest('tr') as HTMLElement;
    expect(within(await row('soon')).getByText('Within a week')).toBeTruthy();
    const gone = await row('gone');
    expect(within(gone).getByText('Expired').classList.contains('badge-danger')).toBe(true);
    expect(gone.classList.contains('row-muted')).toBe(true);
    expect(within(await row('idle')).getByText('Unused for over 90 days')).toBeTruthy();
    const fine = await row('fine');
    for (const badge of ['Within a week', 'Expired', 'Unused for over 90 days']) {
      expect(within(fine).queryByText(badge)).toBeNull();
    }
  });

  it('makes a key and shows its token once, then no more once revoked', async () => {
    const user = userEvent.setup();
    show([]);
    const minted = { ...application({ scopes: ['write'] }), token: 'rtr_k1_secret' };
    const create = vi.spyOn(api, 'createApplication').mockResolvedValue(minted);

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    await user.click(within(dialog).getByLabelText(/^Write/));
    await user.selectOptions(within(dialog).getByLabelText('Expires'), 'In 90 days');
    // What the list reads once the key exists.
    vi.spyOn(api, 'getApplications').mockResolvedValue([application({ scopes: ['write'] })]);
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));

    await waitFor(() => expect(create).toHaveBeenCalled());
    const sent: NewApplication = nthCall(create)[0];
    expect(sent).toEqual({
      name: 'n8n',
      scopes: ['write'],
      may_confirm: [],
      may_move_files: false,
      expires_in_days: 90,
    });
    expect(await screen.findByText('rtr_k1_secret')).toBeTruthy();
    expect(screen.getByText('Key for n8n')).toBeTruthy();

    const revoke = vi.spyOn(api, 'revokeApplication').mockResolvedValue(undefined);
    await user.click(await screen.findByRole('button', { name: 'Revoke – n8n' }));
    expect(await answerConfirmation()).toBe(
      'Revoke the key of n8n? It stops working at once, and cannot be restored.',
    );
    await waitFor(() => expect(revoke).toHaveBeenCalledWith('k1'));
    await waitFor(() => expect(screen.queryByText('rtr_k1_secret')).toBeNull());
  });

  it('asks the guardrails and the file moves only of a key that operates', async () => {
    const user = userEvent.setup();
    show([]);
    const create = vi
      .spyOn(api, 'createApplication')
      .mockResolvedValue({ ...application(), token: 'rtr_k1_secret' });

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    const batch = within(dialog).getByLabelText('Apply a whole simulation') as HTMLInputElement;
    const files = within(dialog).getByLabelText('May move files on disk') as HTMLInputElement;
    expect(batch.disabled).toBe(true);
    expect(files.disabled).toBe(true);

    await user.click(within(dialog).getByLabelText(/^Operate/));
    await user.click(batch);
    await user.click(files);
    // Taken back: what was ticked under it is not sent.
    await user.click(within(dialog).getByLabelText(/^Operate/));
    await user.type(within(dialog).getByLabelText('Name'), 'cron');
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));

    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(nthCall(create)[0]).toEqual({
      name: 'cron',
      scopes: [],
      may_confirm: [],
      may_move_files: false,
      expires_in_days: null,
    });
  });

  it('sends the guardrails and the file move ticked for a key that operates', async () => {
    const user = userEvent.setup();
    show([]);
    const create = vi
      .spyOn(api, 'createApplication')
      .mockResolvedValue({ ...application(), token: 'rtr_k1_secret' });

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    await user.click(within(dialog).getByLabelText(/^Operate/));
    await user.click(within(dialog).getByLabelText('Not enough room at the destination'));
    await user.click(within(dialog).getByLabelText('Apply a whole simulation'));
    await user.click(within(dialog).getByLabelText('May move files on disk'));
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));

    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(nthCall(create)[0]).toEqual({
      name: 'n8n',
      scopes: ['operate'],
      may_confirm: ['capacity', 'batch'],
      may_move_files: true,
      expires_in_days: null,
    });
    // The token is read on the page, not under a dialog left open over it.
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(screen.getByText('rtr_k1_secret')).toBeTruthy();
  });

  it('starts every new key from an empty form', async () => {
    const user = userEvent.setup();
    show([]);
    vi.spyOn(api, 'createApplication').mockResolvedValue({ ...application(), token: 'rtr_k1_a' });

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    let dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    await user.click(within(dialog).getByLabelText(/^Operate/));
    await user.click(within(dialog).getByLabelText('Apply a whole simulation'));
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

    await user.click(screen.getByRole('button', { name: 'New key' }));
    dialog = await screen.findByRole('dialog');
    expect((within(dialog).getByLabelText('Name') as HTMLInputElement).value).toBe('');
    expect((within(dialog).getByLabelText(/^Operate/) as HTMLInputElement).checked).toBe(false);
    const batch = within(dialog).getByLabelText('Apply a whole simulation') as HTMLInputElement;
    expect(batch.checked).toBe(false);
  });

  it('keeps a key whose revocation was cancelled', async () => {
    const user = userEvent.setup();
    show([application()]);
    const revoke = vi.spyOn(api, 'revokeApplication').mockResolvedValue(undefined);

    await user.click(await screen.findByRole('button', { name: 'Revoke – n8n' }));
    await answerConfirmation(null);

    expect(revoke).not.toHaveBeenCalled();
    expect(screen.getByText('n8n')).toBeTruthy();
  });

  it('keeps the token just made on screen when another key is revoked', async () => {
    const user = userEvent.setup();
    const older = application({ id: 'k0', name: 'cron' });
    show([older]);
    vi.spyOn(api, 'createApplication').mockResolvedValue({
      ...application(),
      token: 'rtr_k1_secret',
    });
    const revoke = vi.spyOn(api, 'revokeApplication').mockResolvedValue(undefined);

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    vi.spyOn(api, 'getApplications').mockResolvedValue([older, application()]);
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));
    expect(await screen.findByText('rtr_k1_secret')).toBeTruthy();

    vi.spyOn(api, 'getApplications').mockResolvedValue([application()]);
    await user.click(screen.getByRole('button', { name: 'Revoke – cron' }));
    await answerConfirmation();

    expect(revoke).toHaveBeenCalledWith('k0');
    await waitFor(() => expect(screen.queryByText('cron')).toBeNull());
    expect(screen.getByText('rtr_k1_secret')).toBeTruthy();
  });

  it('holds Create until the key has a name', async () => {
    const user = userEvent.setup();
    show([]);

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    const create = within(dialog).getByRole('button', { name: 'Create a key' });
    expect((create as HTMLButtonElement).disabled).toBe(true);

    await user.type(within(dialog).getByLabelText('Name'), '  ');
    expect((create as HTMLButtonElement).disabled).toBe(true);
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    expect((create as HTMLButtonElement).disabled).toBe(false);
  });

  it('keeps the dialog open with the refusal when a name is taken', async () => {
    const user = userEvent.setup();
    show([]);
    vi.spyOn(api, 'createApplication').mockRejectedValue(
      new ApiError('Another application key is already called n8n.', 409, 'conflict'),
    );

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));

    expect(
      await within(dialog).findByText('Another application key is already called n8n.'),
    ).toBeTruthy();
  });
});

it('hands the focus to the key that took the place of the revoked one', async () => {
  const user = userEvent.setup();
  const next = application({ id: 'k2', name: 'cron' });
  show([application(), next]);
  vi.spyOn(api, 'revokeApplication').mockResolvedValue(undefined);

  await user.click(await screen.findByRole('button', { name: 'Revoke – n8n' }));
  vi.spyOn(api, 'getApplications').mockResolvedValue([next]);
  await answerConfirmation();

  await waitFor(() =>
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Revoke – cron' })),
  );
});
