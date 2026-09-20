import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { instance } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import Instances from './Instances.svelte';

/**
 * The only screen that holds an Arr credential. It must never show one back,
 * must not lose the stored one on an edit that did not mean to change it, and
 * must not let a user sync a library they have not connected yet.
 */

const STRINGS = {
  ArrInstances: 'Instances',
  AddInstance: 'Add instance',
  NoInstanceConfigured: 'No instance configured',
  SyncAll: 'Sync all',
  SyncNow: 'Sync now',
  Edit: 'Edit',
  Save: 'Save',
  Actions: 'Actions',
  TestConnection: 'Test connection',
  Name: 'Name',
  Type: 'Type',
  BaseUrl: 'Base URL',
  ApiKey: 'API key',
  ApiKeyEncrypted: 'encrypted',
  ApiKeyKeepHint: 'Leave blank to keep the current key',
  Disabled: 'disabled',
  Never: 'never',
  InstanceUpdated: 'Instance updated',
  InstanceAdded: 'Instance added',
  SyncEveryMinutes: 'Sync every (minutes)',
  EnabledSyncedRouted: 'Enabled',
};

const show = (list: ReturnType<typeof instance>[]) => {
  vi.spyOn(api, 'getInstances').mockResolvedValue(list);
  return renderWithI18n(Instances, { strings: STRINGS });
};

afterEach(() => vi.restoreAllMocks());

describe('Instances', () => {
  it('invites a first instance instead of showing an empty table', async () => {
    show([]);

    expect(await screen.findByText('No instance configured')).toBeTruthy();
  });

  it('cannot sync everything when there is nothing to sync', async () => {
    show([]);

    const syncAll = await screen.findByRole('button', { name: 'Sync all' });
    expect((syncAll as HTMLButtonElement).disabled).toBe(true);
  });

  /**
   * Never log or return a decrypted key. The row says the value is sealed; it
   * does not say what the value is, not even partially — only a plaintext key
   * left by an older version gets a partial mask, and that is a prompt to
   * re-save rather than information.
   */
  it('says the key is sealed without showing any of it', async () => {
    show([instance({ api_key_encrypted: true, api_key_masked: 'abc•••••xyz' })]);

    expect(await screen.findByText('encrypted')).toBeTruthy();
    expect(screen.queryByText('abc•••••xyz')).toBeNull();
  });

  /**
   * Encrypted at rest is a property of the stored value, not something that
   * went well: green in this interface means "this succeeded" and nothing
   * else.
   */
  it('states encryption as a fact rather than as a success', async () => {
    show([instance({ api_key_encrypted: true })]);

    const badge = await screen.findByText('encrypted');
    expect(badge.className).not.toContain('badge-success');
  });

  it('names each row action after its instance', async () => {
    show([instance({ name: 'Radarr' }), instance({ id: 'i2', name: 'Sonarr' })]);

    expect(await screen.findByRole('button', { name: 'Sync now Radarr' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Edit — Sonarr' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Actions — Sonarr' })).toBeTruthy();
  });

  /**
   * Blank means "keep the stored key" on the backend, which is the only way to
   * edit a base URL without retyping a credential the user no longer has. The
   * form must therefore open empty, not pre-filled with a mask that would be
   * saved as the literal key.
   */
  it('opens the editor with the key field empty, so an edit keeps the stored one', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit — Radarr' }));

    const field = (await screen.findByLabelText('API key')) as HTMLInputElement;
    expect(field.value).toBe('');
  });

  it('sends the blank key through unchanged rather than inventing one', async () => {
    const update = vi.spyOn(api, 'updateInstance').mockResolvedValue(undefined as never);
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit — Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.input(url, { target: { value: 'http://nas:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(nthCall(update)[1]).toMatchObject({ api_key: '', base_url: 'http://nas:7878' });
  });

  /**
   * `showModal()` makes the page inert and the overlay dims it: a refusal put
   * in the page banner sat behind the dialog, faded, with a Dismiss nobody
   * could press, and a 400 on the URL looked like a Save button doing nothing.
   */
  it('shows a refused save inside the dialog, where it can be read', async () => {
    vi.spyOn(api, 'updateInstance').mockRejectedValue(
      new ApiError('base_url must start with http:// or https://', 400, 'bad_request'),
    );
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit — Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.input(url, { target: { value: 'nas:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);

    const dialog = await screen.findByRole('dialog');
    const alert = await within(dialog).findByRole('alert');
    expect(alert).toHaveTextContent('base_url must start with http:// or https://');
  });

  /**
   * A number field emptied binds `null`, and the server wants an integer: the
   * save came back 422 from serde, in English, behind the dialog. Not sent.
   */
  it('will not save an instance whose sync interval was emptied', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit — Radarr' }));
    const every = await screen.findByLabelText('Sync every (minutes)');
    const save = screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement;
    expect(save.disabled).toBe(false);

    await fireEvent.input(every, { target: { value: '' } });
    expect(save.disabled).toBe(true);
    await fireEvent.input(every, { target: { value: '0' } });
    expect(save.disabled).toBe(true);
    await fireEvent.input(every, { target: { value: '30' } });
    expect(save.disabled).toBe(false);
  });

  it('syncs the instance whose button was pressed', async () => {
    const sync = vi
      .spyOn(api, 'syncInstance')
      .mockResolvedValue({ media: 3, root_folders: 2 } as never);
    show([instance({ id: 'i1', name: 'Radarr' }), instance({ id: 'i2', name: 'Sonarr' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync now Sonarr' }));

    await waitFor(() => expect(sync).toHaveBeenCalledWith('i2'));
  });

  it('marks a disabled instance rather than hiding it', async () => {
    show([instance({ enabled: false })]);

    expect(await screen.findByText('disabled')).toBeTruthy();
    expect(screen.getByText('Radarr')).toBeTruthy();
  });
});
