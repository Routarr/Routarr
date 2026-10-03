import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { nthCall } from '../test/spy';
import { ApiError, api } from '../api/client';
import type { Application, NewApplication } from '../api/types';
import Applications from './Applications.svelte';

/**
 * The owner hands out keys here. What the screen must get right: the token is
 * shown once and only after it is made, what a key allows reads at a glance,
 * and nothing is asked of a key that cannot be asked of it.
 */

const STRINGS = {
  Applications: 'Applications',
  NewApplicationKey: 'New key',
  NoApplications: 'No application has a key yet.',
  Name: 'Name',
  ApplicationScopes: 'Allows',
  ApplicationMayConfirm: 'Guardrails it may answer',
  ApplicationMayMoveFiles: 'May move files on disk',
  ScopeRead: 'read',
  ScopeOperate: 'operate',
  ScopeWrite: 'write',
  GuardrailBatch: 'Apply a whole simulation',
  GuardrailCapacity: 'Not enough room at the destination',
  GuardrailThreshold: 'More moves than the threshold',
  Cancel: 'Cancel',
  CreateKey: 'Create a key',
  ApplicationTokenFor: 'Key for {name}',
  ApplicationCreated: 'Key created for {name}.',
  ApplicationRevoked: 'Key revoked for {name}.',
  RevokeKey: 'Revoke',
  ConfirmRevokeApplication: 'Revoke the key of {name}?',
  Never: 'never',
  None: '-',
  ListSeparator: ', ',
  Yes: 'Yes',
  No: 'No',
};

function application(over: Partial<Application> = {}): Application {
  return {
    id: 'k1',
    name: 'n8n',
    scopes: [],
    may_confirm: [],
    may_move_files: false,
    created_at: '2026-09-30 10:00:00',
    created_by: null,
    last_used_at: null,
    ...over,
  };
}

function show(listed: Application[]) {
  vi.spyOn(api, 'getApplications').mockResolvedValue(listed);
  return renderWithI18n(Applications, { strings: STRINGS });
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
    expect(within(row).getByText('read')).toBeTruthy();
    expect(within(row).getByText('operate')).toBeTruthy();
    expect(within(row).queryByText('write')).toBeNull();
    expect(
      within(row).getByText('Apply a whole simulation, Not enough room at the destination'),
    ).toBeTruthy();
    expect(within(row).getByText('Yes')).toBeTruthy();
    expect(within(row).getByText('never')).toBeTruthy();
  });

  it('makes a key and shows its token once, then no more once revoked', async () => {
    const user = userEvent.setup();
    show([]);
    const minted = { ...application({ scopes: ['write'] }), token: 'rtr_k1_secret' };
    const create = vi.spyOn(api, 'createApplication').mockResolvedValue(minted);

    await user.click(await screen.findByRole('button', { name: 'New key' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Name'), 'n8n');
    await user.click(within(dialog).getByLabelText(/^write/));
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
    });
    expect(await screen.findByText('rtr_k1_secret')).toBeTruthy();
    expect(screen.getByText('Key for n8n')).toBeTruthy();

    const revoke = vi.spyOn(api, 'revokeApplication').mockResolvedValue(undefined);
    await user.click(await screen.findByRole('button', { name: 'Revoke – n8n' }));
    expect(await answerConfirmation()).toBe('Revoke the key of n8n?');
    await waitFor(() => expect(revoke).toHaveBeenCalledWith('k1'));
    await waitFor(() => expect(screen.queryByText('rtr_k1_secret')).toBeNull());
  });

  /**
   * A guardrail or a file move is only ever asked of a key that operates. Kept
   * editable without it, a box ticked there would be stored and never read.
   */
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

    await user.click(within(dialog).getByLabelText(/^operate/));
    await user.click(batch);
    await user.click(files);
    // Taken back: what was ticked under it is not sent.
    await user.click(within(dialog).getByLabelText(/^operate/));
    await user.type(within(dialog).getByLabelText('Name'), 'cron');
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));

    await waitFor(() => expect(create).toHaveBeenCalled());
    expect(nthCall(create)[0]).toEqual({
      name: 'cron',
      scopes: [],
      may_confirm: [],
      may_move_files: false,
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
    await user.click(within(dialog).getByLabelText(/^operate/));
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
    await user.click(within(dialog).getByLabelText(/^operate/));
    await user.click(within(dialog).getByLabelText('Apply a whole simulation'));
    await user.click(within(dialog).getByRole('button', { name: 'Create a key' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

    await user.click(screen.getByRole('button', { name: 'New key' }));
    dialog = await screen.findByRole('dialog');
    expect((within(dialog).getByLabelText('Name') as HTMLInputElement).value).toBe('');
    expect((within(dialog).getByLabelText(/^operate/) as HTMLInputElement).checked).toBe(false);
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
