import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { instance } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import { statusRevision } from '../lib/status.svelte';
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
  InstanceFirstSync: '{name} is saved. Reading its library for the first time.',
  InstanceFirstSyncDone: '{name} is synced. Titles: {media}, root folders: {folders}',
  InstanceFirstSyncFailed: '{name} is saved, but its first sync failed.',
  InstanceFirstSyncFinished: '{name} is synced.',
  InstanceUrlHelp: "With the port. In Docker, use {service}'s container name.",
  InstanceKeyHelp: 'In {service}: Settings → General → Security → API Key.',
  ConnectionOk: '{name}: connected (v{version}, {folders} root folders)',
  OpensInNewTab: 'opens in a new tab',
  SyncEveryMinutes: 'Sync every (minutes)',
  EnabledSyncedRouted: 'Enabled',
  Cancel: 'Cancel',
  SyncAllResult: 'Instances synced: {count}',
  CopyUrl: 'Copy the webhook URL',
  WebhookUrlCopied: 'Webhook URL copied',
  WebhookUrlCopyFailed: 'The browser blocked the clipboard. Webhook URL: {url}',
};

const show = (list: ReturnType<typeof instance>[]) => {
  vi.spyOn(api, 'getInstances').mockResolvedValue(list);
  return renderWithI18n(Instances, { strings: STRINGS });
};

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, 'clipboard');
  window.history.replaceState({}, '', '/');
});

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

  /** A second write started mid-sync would end it early, freeing Sync all while it still runs. */
  it('offers no row action that writes while a sync runs', async () => {
    vi.spyOn(api, 'syncAll').mockReturnValue(new Promise(() => {}));
    show([instance({ webhook_url: '/api/v1/webhooks/i1/tok' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync all' }));
    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));

    expect(await screen.findByRole('menuitem', { name: 'Copy the webhook URL' })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: 'RotateWebhookToken' })).toBeNull();
    expect(screen.queryByRole('menuitem', { name: 'Delete' })).toBeNull();
  });

  /** Only enabled instances sync, so with none there is nothing a click could do. */
  it('cannot sync everything when every instance is disabled', async () => {
    show([instance({ enabled: false })]);

    const syncAll = await screen.findByRole('button', { name: 'Sync all' });
    await screen.findByText('Radarr');
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
    expect(screen.getByRole('button', { name: 'Edit – Sonarr' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Actions – Sonarr' })).toBeTruthy();
  });

  /**
   * Blank means "keep the stored key" on the backend, which is the only way to
   * edit a base URL without retyping a credential the user no longer has. The
   * form must therefore open empty, not pre-filled with a mask that would be
   * saved as the literal key.
   */
  it('opens the editor with the key field empty, so an edit keeps the stored one', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));

    const field = (await screen.findByLabelText('API key')) as HTMLInputElement;
    expect(field.value).toBe('');
  });

  it('sends the blank key through unchanged rather than inventing one', async () => {
    const update = vi
      .spyOn(api, 'updateInstance')
      .mockResolvedValue(instance({ last_sync_at: '2026-08-27 10:05:00' }));
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
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

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
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

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
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

  /** A dialog opens on its own form, not on the refusal of the one before. */
  it('opens the next dialog without the refusal of the last one', async () => {
    vi.spyOn(api, 'updateInstance').mockRejectedValue(
      new ApiError('base_url must start with http:// or https://', 400, 'bad_request'),
    );
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.input(url, { target: { value: 'nas:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);
    await within(await screen.findByRole('dialog')).findByRole('alert');
    await fireEvent.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }),
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

    await fireEvent.click(screen.getByRole('button', { name: 'Edit – Radarr' }));

    expect(within(await screen.findByRole('dialog')).queryByRole('alert')).toBeNull();
  });

  /** Every instance failing is a failure, whatever the count of zero synced says. */
  it('reports a sync in which every instance failed as a failure', async () => {
    vi.spyOn(api, 'syncAll').mockResolvedValue([
      {
        instance_id: 'i1',
        instance_name: 'Radarr',
        root_folders: 0,
        media: 0,
        removed: 0,
        error: 'connection refused',
      },
    ]);
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync all' }));

    const summary = await screen.findByText('Instances synced: 0');
    expect(summary.closest('.banner')?.classList.contains('banner-danger')).toBe(true);
    expect(screen.getByText('Radarr: connection refused')).toBeTruthy();
    expect(screen.queryByRole('status')).toBeNull();
  });

  /** Some instances synced and some not is a partial result, each failure named. */
  it('reports a sync in which some instances failed as a partial result', async () => {
    vi.spyOn(api, 'syncAll').mockResolvedValue([
      { instance_id: 'i1', instance_name: 'Radarr', root_folders: 3, media: 40, removed: 0 },
      {
        instance_id: 'i2',
        instance_name: 'Sonarr',
        root_folders: 0,
        media: 0,
        removed: 0,
        error: 'connection refused, twice',
      },
    ]);
    show([instance(), instance({ id: 'i2', name: 'Sonarr' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync all' }));

    const summary = await screen.findByText('Instances synced: 1');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('Sonarr: connection refused, twice')).toBeTruthy();
  });

  /** The Add dialog opens on a blank form, never on the refusal of the one before. */
  it('opens the Add dialog without the refusal of the last one', async () => {
    vi.spyOn(api, 'updateInstance').mockRejectedValue(
      new ApiError('base_url must start with http:// or https://', 400, 'bad_request'),
    );
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.input(url, { target: { value: 'nas:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);
    await within(await screen.findByRole('dialog')).findByRole('alert');
    await fireEvent.click(
      within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }),
    );
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

    await fireEvent.click(screen.getByRole('button', { name: 'Add instance' }));

    expect(within(await screen.findByRole('dialog')).queryByRole('alert')).toBeNull();
  });

  async function copyWebhookUrl() {
    show([instance({ webhook_url: '/api/v1/webhooks/i1/tok' })]);
    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy the webhook URL' }));
  }

  it('says the webhook URL was copied once the clipboard took it', async () => {
    let settle = () => {};
    const writeText = vi.fn(() => new Promise<void>((resolve) => (settle = resolve)));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    await copyWebhookUrl();
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/api/v1/webhooks/i1/tok`),
    );
    expect(screen.queryByText('Webhook URL copied')).toBeNull();

    settle();

    expect(await screen.findByRole('status')).toHaveTextContent('Webhook URL copied');
  });

  /** The clipboard exists on secure origins only, and a homelab often serves plain http. */
  it('hands the webhook URL over when the browser keeps the clipboard closed', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });

    await copyWebhookUrl();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      `${window.location.origin}/api/v1/webhooks/i1/tok`,
    );
    expect(screen.queryByText('Webhook URL copied')).toBeNull();
  });

  it('hands the webhook URL over when the browser refuses the copy', async () => {
    const writeText = vi.fn().mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    await copyWebhookUrl();

    expect(await screen.findByRole('alert')).toHaveTextContent('/api/v1/webhooks/i1/tok');
    expect(screen.queryByText('Webhook URL copied')).toBeNull();
  });

  async function addInstance(name: string) {
    await fireEvent.click(await screen.findByRole('button', { name: 'Add instance' }));
    await fireEvent.input(await screen.findByLabelText('Name'), { target: { value: name } });
    const key = screen.getByLabelText('API key');
    await fireEvent.input(key, { target: { value: 'secret' } });
    await fireEvent.submit(key.closest('form') as HTMLFormElement);
  }

  /**
   * Left to the scheduler, a new instance waits up to a whole pass with no
   * library, no root folders and the guide's first step still open.
   */
  it('syncs a new instance at once rather than at the next scheduled pass', async () => {
    vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ id: 'i9', name: 'Films' }));
    const sync = vi.spyOn(api, 'syncInstance').mockResolvedValue({
      instance_id: 'i9',
      instance_name: 'Films',
      media: 12,
      root_folders: 2,
      removed: 0,
    });
    const before = statusRevision();
    show([]);

    await addInstance('Films');

    await waitFor(() => expect(sync).toHaveBeenCalledWith('i9'));
    const done = await screen.findByText('Films is synced. Titles: 12, root folders: 2');
    expect(done.closest('.banner')?.classList.contains('banner-success')).toBe(true);
    expect(statusRevision()).toBeGreaterThan(before);
  });

  /** Saved and not synced is a partial result, and the reason is the Arr's answer. */
  it('keeps a new instance whose first sync fails, and says why', async () => {
    vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ id: 'i9', name: 'Films' }));
    vi.spyOn(api, 'syncInstance').mockRejectedValue(
      new ApiError('Radarr answered 401 Unauthorized', 502, 'upstream'),
    );
    show([]);

    await addInstance('Films');

    const summary = await screen.findByText('Films is saved, but its first sync failed.');
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('Radarr answered 401 Unauthorized')).toBeTruthy();
  });

  /**
   * The server finishes a sync the browser stopped waiting for, and a large
   * library outlasts the request. Called failed, it would send the reader to
   * doubt a key that is right.
   */
  it('follows a first sync that outlasts the request instead of calling it failed', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    try {
      vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ id: 'i9', name: 'Films' }));
      vi.spyOn(api, 'syncInstance').mockRejectedValue(new ApiError('', 0, 'timeout'));
      show([]);

      await addInstance('Films');

      expect(
        await screen.findByText('Films is saved. Reading its library for the first time.'),
      ).toBeTruthy();
      expect(screen.queryByText('Films is saved, but its first sync failed.')).toBeNull();
      const before = statusRevision();
      vi.mocked(api.getInstances).mockResolvedValue([
        instance({
          id: 'i9',
          name: 'Films',
          last_sync_at: '2026-08-27 10:05:00',
          last_sync_attempt_at: '2026-08-27 10:05:00',
          last_sync_status: 'success',
        }),
      ]);
      await vi.advanceTimersByTimeAsync(5000);

      expect(await screen.findByText('Films is synced.')).toBeTruthy();
      expect(statusRevision()).toBeGreaterThan(before);
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves an instance that synced before to its schedule', async () => {
    vi.spyOn(api, 'updateInstance').mockResolvedValue(
      instance({ last_sync_at: '2026-08-27 10:05:00' }),
    );
    const sync = vi.spyOn(api, 'syncInstance');
    show([instance({ last_sync_at: '2026-08-27 10:05:00' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.submit(url.closest('form') as HTMLFormElement);

    expect(await screen.findByText('Instance updated')).toBeTruthy();
    expect(sync).not.toHaveBeenCalled();
  });

  it('does not sync an instance saved disabled', async () => {
    vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ enabled: false }));
    const sync = vi.spyOn(api, 'syncInstance');
    show([]);

    await addInstance('Films');

    expect(await screen.findByText('Instance added')).toBeTruthy();
    expect(sync).not.toHaveBeenCalled();
  });

  const PROBED = {
    success: true,
    version: '5.2.6',
    app_name: 'Radarr',
    root_folders: 2,
    inaccessible_root_folders: 0,
  };

  async function openAdd(): Promise<HTMLElement> {
    await fireEvent.click(await screen.findByRole('button', { name: 'Add instance' }));
    return screen.findByRole('dialog');
  }

  async function typeAddressAndKey(url: string, key: string) {
    await fireEvent.input(await screen.findByLabelText('Base URL'), { target: { value: url } });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: key } });
  }

  /** A guess left in the field is saved by whoever does not notice it. */
  it('starts a new instance with an empty address and an example for its type', async () => {
    show([]);
    await openAdd();

    const url = (await screen.findByLabelText('Base URL')) as HTMLInputElement;
    expect(url.value).toBe('');
    expect(url.placeholder).toBe('http://radarr:7878');
    await userEvent.selectOptions(screen.getByLabelText('Type'), 'sonarr');
    expect(url.placeholder).toBe('http://sonarr:8989');
    expect(url.value).toBe('');
  });

  it('says where the address and the key come from, for the type chosen', async () => {
    show([]);
    await openAdd();

    expect(await screen.findByLabelText('Base URL')).toHaveAccessibleDescription(
      "With the port. In Docker, use Radarr's container name.",
    );
    await userEvent.selectOptions(screen.getByLabelText('Type'), 'sonarr');
    expect(screen.getByLabelText('API key')).toHaveAccessibleDescription(
      'In Sonarr: Settings → General → Security → API Key.',
    );
  });

  /** An address copied from a browser bar or typed from memory often has no scheme. */
  it('adds the scheme an address typed without one lacks', async () => {
    show([]);
    await openAdd();
    const url = (await screen.findByLabelText('Base URL')) as HTMLInputElement;

    await fireEvent.input(url, { target: { value: ' 192.168.1.10:7878 ' } });
    await fireEvent.blur(url);
    expect(url.value).toBe('http://192.168.1.10:7878');

    await fireEvent.input(url, { target: { value: 'https://radarr.example.org' } });
    await fireEvent.blur(url);
    expect(url.value).toBe('https://radarr.example.org');
  });

  /** Enter submits with no blur, and a phone capitalises the first letter. */
  it('sends an address with its scheme, however it was typed and saved', async () => {
    const create = vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ enabled: false }));
    const probe = vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([]);
    const dialog = await openAdd();
    const url = await screen.findByLabelText('Base URL');
    expect(url.getAttribute('autocapitalize')).toBe('off');

    await typeAddressAndKey('radarr:7878', 'secret');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    await waitFor(() =>
      expect(probe).toHaveBeenCalledWith(
        expect.objectContaining({ base_url: 'http://radarr:7878' }),
        expect.anything(),
      ),
    );

    await fireEvent.input(await screen.findByLabelText('Name'), { target: { value: 'Films' } });
    await fireEvent.input(url, { target: { value: 'Http://radarr:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith(
        expect.objectContaining({ base_url: 'http://radarr:7878' }),
      ),
    );
  });

  it('cannot try a new instance before its address and key are typed', async () => {
    show([]);
    const dialog = await openAdd();
    const test = within(dialog).getByRole('button', { name: 'Test connection' });

    expect(test).toBeDisabled();
    await fireEvent.input(screen.getByLabelText('Base URL'), {
      target: { value: 'http://radarr:7878' },
    });
    expect(test).toBeDisabled();
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: 'secret' } });
    expect(test).toBeEnabled();
  });

  it('tries the values typed before anything is saved', async () => {
    const probe = vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    const create = vi.spyOn(api, 'createInstance');
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    await waitFor(() =>
      expect(probe).toHaveBeenCalledWith(
        {
          instance_type: 'radarr',
          base_url: 'http://radarr:7878',
          api_key: 'secret',
          id: undefined,
        },
        expect.any(AbortSignal),
      ),
    );
    expect(await within(dialog).findByRole('status')).toHaveTextContent(
      'Radarr: connected (v5.2.6, 2 root folders)',
    );
    expect(create).not.toHaveBeenCalled();
  });

  /** Read where the values were typed, and about those values only. */
  it('shows why a try failed inside the dialog, until a value it tried changes', async () => {
    vi.spyOn(api, 'probeInstance').mockRejectedValue(
      new ApiError('Nothing answers at http://localhost:7878.', 502, 'external_api_error'),
    );
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://localhost:7878', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    expect(await within(dialog).findByRole('alert')).toHaveTextContent(
      'Nothing answers at http://localhost:7878.',
    );
    await fireEvent.input(screen.getByLabelText('Base URL'), {
      target: { value: 'http://radarr:7878' },
    });
    expect(within(dialog).queryByRole('alert')).toBeNull();
  });

  /** The edit form never shows the stored key: blank stands for it. */
  it('tries an edit with the stored key when its field is left blank', async () => {
    const probe = vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([instance({ id: 'i1' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    await waitFor(() =>
      expect(probe).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'i1', api_key: '' }),
        expect.any(AbortSignal),
      ),
    );
  });

  /** A result about other values would vouch for these, a success as a failure. */
  it('holds a try to the address, the key and the type it tried', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    await within(dialog).findByRole('status');
    const url = screen.getByLabelText('Base URL');
    const key = screen.getByLabelText('API key');

    await fireEvent.input(url, { target: { value: 'http://radarr:7879' } });
    expect(within(dialog).queryByRole('status')).toBeNull();
    await fireEvent.input(url, { target: { value: 'http://radarr:7878' } });
    expect(within(dialog).getByRole('status')).toBeTruthy();

    await fireEvent.input(key, { target: { value: 'another' } });
    expect(within(dialog).queryByRole('status')).toBeNull();
    await fireEvent.input(key, { target: { value: 'secret' } });
    expect(within(dialog).getByRole('status')).toBeTruthy();

    await userEvent.selectOptions(screen.getByLabelText('Type'), 'sonarr');
    expect(within(dialog).queryByRole('status')).toBeNull();
  });

  /** A refused save is what happened last: the try before it no longer speaks. */
  it('shows one outcome at a time in the dialog', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    vi.spyOn(api, 'createInstance').mockRejectedValue(
      new ApiError('Give the instance a name.', 400, 'bad_request'),
    );
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    await within(dialog).findByRole('status');

    const form = screen.getByLabelText('Base URL').closest('form') as HTMLFormElement;
    await fireEvent.submit(form);

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Give the instance a name.');
    expect(within(dialog).queryByRole('status')).toBeNull();
  });

  /** Given up on, a try that answers later belongs to no dialog. */
  it('says nothing in the next dialog about a try still running in the last one', async () => {
    let fail!: (reason: unknown) => void;
    vi.spyOn(api, 'probeInstance').mockReturnValueOnce(
      new Promise((_, reject) => {
        fail = reject;
      }),
    );
    show([
      instance({ id: 'i1', name: 'Radarr' }),
      instance({ id: 'i2', name: 'Sonarr', instance_type: 'sonarr' }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    let dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Sonarr' }));
    dialog = await screen.findByRole('dialog');
    fail(new ApiError('Nothing answers at http://localhost:7878.', 502, 'external_api_error'));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(within(dialog).queryByRole('alert')).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Test connection' })).toBeEnabled();
  });

  /** The Arr's own interface is one click away, in a tab of its own. */
  it('links each address to the Arr it names, in a new tab', async () => {
    show([instance({ base_url: 'http://radarr.home.arpa' })]);

    const link = await screen.findByRole('link', { name: /http:\/\/radarr\.home\.arpa/ });
    expect(link.getAttribute('href')).toBe('http://radarr.home.arpa');
    expect(link.getAttribute('target')).toBe('_blank');
    expect(link.getAttribute('rel')).toContain('noopener');
    expect(link).toHaveAccessibleName(/opens in a new tab/);
  });

  /**
   * The container name the form recommends in Docker resolves inside
   * Routarr's network only, so the reader's browser cannot open it.
   */
  it('offers no link for a container name, and one for any other host', async () => {
    show([
      instance({ id: 'i1', base_url: 'http://radarr:7878' }),
      instance({ id: 'i2', name: 'Sonarr', base_url: 'http://[fd00::10]:8989' }),
    ]);

    expect(await screen.findByText('http://radarr:7878')).toBeTruthy();
    expect(screen.queryByRole('link', { name: /radarr:7878/ })).toBeNull();
    expect(screen.getByRole('link', { name: /fd00::10/ })).toBeTruthy();
  });

  /** The backend refuses any other scheme; the table does not rely on it. */
  it('links only an http or https address', async () => {
    show([instance({ base_url: 'javascript:alert(1)' })]);

    expect(await screen.findByText('javascript:alert(1)')).toBeTruthy();
    expect(screen.queryByRole('link', { name: /javascript/ })).toBeNull();
  });

  /** The guide's first step lands here with the dialog open, once. */
  it('opens the Add dialog when the guide sends the reader here', async () => {
    window.history.replaceState({}, '', '/instances?add=1');
    show([]);

    expect(await screen.findByRole('dialog')).toBeTruthy();
    expect(window.location.search).toBe('');
  });
});
