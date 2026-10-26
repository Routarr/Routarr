import { describe, it, expect, vi, afterEach } from 'vitest';
import { nthCall } from '../test/spy';
import { fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { health, healthInstance, instance } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import type { WebhookCheck } from '../api/types';
import { statusRevision } from '../lib/status.svelte';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { unloading } from '../test/leaving';
import { navigate, router } from '../lib/router.svelte';
import Instances from './Instances.svelte';
import { publishAccess } from '../lib/access.svelte';
import { backendConstant } from '../test/backend';

const readOf = (over: Partial<WebhookCheck> = {}): WebhookCheck => ({
  state: 'ok',
  token: 'ok',
  missing_triggers: [],
  method: null,
  tags: [],
  other_instance: null,
  problems: [],
  notes: [],
  checked_at: new Date(Date.now() - 60_000).toISOString(),
  ...over,
});
const MISSES_RENAME = 'The webhook in Radarr leaves On Rename unticked.';

/** Each instance answering, as `/health` says once it probed. */
const answering = (list: ReturnType<typeof instance>[]) =>
  health({
    instances: list.map((each) =>
      healthInstance({
        id: each.id,
        name: each.name,
        status: each.enabled ? 'connected' : 'disabled',
      }),
    ),
  });

const show = (list: ReturnType<typeof instance>[]) => {
  vi.spyOn(api, 'getInstances').mockResolvedValue(list);
  vi.spyOn(api, 'getHealth').mockResolvedValue(answering(list));
  return renderWithI18n(Instances);
};

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, 'clipboard');
});

describe('Instances', () => {
  it('invites a first instance instead of showing an empty table', async () => {
    show([]);

    expect(await screen.findByText('No instance connected yet.')).toBeTruthy();
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

    expect(await screen.findByRole('menuitem', { name: 'Copy URL' })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: 'Rotate the webhook token' })).toBeNull();
    expect(screen.queryByRole('menuitem', { name: 'Delete' })).toBeNull();
  });

  it('asks before rotating a webhook token, and rotates nothing on Cancel', async () => {
    const rotate = vi.spyOn(api, 'rotateWebhookToken');
    show([instance({ webhook_url: '/api/v1/webhooks/i1/tok' })]);

    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    await fireEvent.click(
      await screen.findByRole('menuitem', { name: 'Rotate the webhook token' }),
    );

    expect(await answerConfirmation(null)).toBe(
      'Rotate the webhook token of "Radarr"? The Arr\'s webhook stops working until the new URL is pasted into it.',
    );
    expect(rotate).not.toHaveBeenCalled();
  });

  /** The delete takes the instance's titles, mappings and exceptions with it. */
  it('deletes nothing when the deletion is cancelled', async () => {
    const remove = vi.spyOn(api, 'deleteInstance');
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));

    expect(await answerConfirmation(null)).toBe(
      'Delete "Radarr"? Its media, mappings and exceptions are removed from Routarr (nothing is deleted in the Arr).',
    );
    expect(remove).not.toHaveBeenCalled();
  });

  /** A sync that would remove most of a library holds it: removed only once a person asks. */
  it('shows the titles a sync held and removes them once confirmed', async () => {
    const remove = vi.spyOn(api, 'removeMissing').mockResolvedValue({
      instance_id: 'i1',
      instance_name: 'Radarr',
      root_folders: 2,
      media: 3,
      removed: 40,
    });
    show([instance({ removal_held: 40 })]);

    expect(await screen.findByText('Held: 40')).toBeTruthy();
    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    await fireEvent.click(
      await screen.findByRole('menuitem', { name: 'Remove the titles it no longer reports' }),
    );
    expect(await answerConfirmation()).toContain('no longer reports 40 titles');

    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    expect(nthCall(remove)[0]).toBe('i1');
    expect(await screen.findByText('"Radarr": titles removed: 40.')).toBeTruthy();
  });

  it('offers no removal while no title is held', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    expect(await screen.findByRole('menuitem', { name: 'Delete' })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: /no longer reports/ })).toBeNull();
  });

  it('deletes the instance whose row was used once confirmed', async () => {
    const sonarr = instance({ id: 'i2', name: 'Sonarr', instance_type: 'sonarr' });
    show([instance(), sonarr]);
    const remove = vi.spyOn(api, 'deleteInstance').mockResolvedValue(undefined);
    const before = statusRevision();

    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Sonarr/ }));
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    vi.spyOn(api, 'getInstances').mockResolvedValue([instance()]);
    expect(await answerConfirmation()).toContain('"Sonarr"');

    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    expect(nthCall(remove)[0]).toBe('i2');
    expect(await screen.findByText('Instance deleted')).toBeTruthy();
    await waitFor(() => expect(screen.queryByText('Sonarr')).toBeNull());
    expect(statusRevision()).toBeGreaterThan(before);
  });

  /** Only enabled instances sync, so with none there is nothing a click could do. */
  it('cannot sync everything when every instance is disabled', async () => {
    show([instance({ enabled: false })]);

    const syncAll = await screen.findByRole('button', { name: 'Sync all' });
    await screen.findByText('Radarr');
    expect((syncAll as HTMLButtonElement).disabled).toBe(true);
  });

  /** The server refuses to read a disabled instance, so its row offers no sync. */
  it('offers no sync on the row of a disabled instance, and says why', async () => {
    show([instance({ enabled: false })]);

    const sync = (await screen.findByRole('button', {
      name: 'Sync now – Radarr',
    })) as HTMLButtonElement;
    expect(sync.disabled).toBe(true);
    expect(sync.title).toBe("Instance 'Radarr' is disabled. Enable it to sync it.");
  });

  /** Every stored key is sealed, said once in the subtitle: a column would repeat a constant. */
  it('names what each instance runs under its name, with no column for its type or key', async () => {
    show([instance({ api_key_masked: 'abc•••••xyz' })]);

    const row = await screen.findByRole('row', { name: /Radarr/ });
    expect(within(row).getAllByRole('cell')[0]).toHaveTextContent(/^Radarr\s*Radarr \(Movies\)$/);
    expect(screen.getAllByRole('columnheader').map((header) => header.textContent?.trim())).toEqual(
      ['Name', 'Status', 'Base URL', 'Last sync', 'Last webhook', 'Actions'],
    );
    expect(screen.queryByText('Encrypted')).toBeNull();
    expect(screen.queryByText('abc•••••xyz')).toBeNull();
  });

  /** The list answers at once, the probe after a connect timeout on an Arr that is down. */
  it('says it checks whether each instance answers until the probe says', async () => {
    let probed = (_: unknown) => {};
    vi.spyOn(api, 'getInstances').mockResolvedValue([instance()]);
    vi.spyOn(api, 'getHealth').mockReturnValue(
      new Promise((resolve) => (probed = resolve as (value: unknown) => void)),
    );
    renderWithI18n(Instances);

    const row = await screen.findByRole('row', { name: /Radarr/ });
    await waitFor(() => expect(row).toHaveTextContent('Checking…'));
    probed(
      health({
        instances: [
          healthInstance({ status: 'error: refused', detail: 'Nothing answers at the address' }),
        ],
      }),
    );

    expect(await within(row).findByText('Error')).toHaveClass('badge-danger');
    expect(row).toHaveTextContent('Nothing answers at the address');
  });

  /** A probe asks every Arr and source again, OMDb's daily quota included. */
  it('reads its instances again every minute without probing them', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      show([instance()]);
      expect(await screen.findByText('Connected')).toBeTruthy();
      const opened = vi.mocked(api.getHealth).mock.calls.length;

      await vi.advanceTimersByTimeAsync(60_000);

      expect(vi.mocked(api.getInstances).mock.calls.length).toBeGreaterThan(1);
      const polled = vi.mocked(api.getHealth).mock.calls.slice(opened);
      expect(polled.every(([options]) => options?.probe === false)).toBe(true);
      expect(screen.getByText('Connected')).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  /** A verdict on the address the probe tried says nothing of the one saved since. */
  it('forgets what the probe found of an instance once it is saved, probing nothing', async () => {
    vi.spyOn(api, 'updateInstance').mockResolvedValue(
      instance({ last_sync_at: '2026-08-27 10:05:00' }),
    );
    show([instance()]);
    expect(await screen.findByText('Connected')).toBeTruthy();
    const opened = vi.mocked(api.getHealth).mock.calls.length;

    await fireEvent.click(screen.getByRole('button', { name: 'Edit – Radarr' }));
    const url = await screen.findByLabelText('Base URL');
    await fireEvent.input(url, { target: { value: 'http://nas:7878' } });
    await fireEvent.submit(url.closest('form') as HTMLFormElement);

    const row = await screen.findByRole('row', { name: /Radarr/ });
    await waitFor(() => expect(within(row).getByText('Unknown')).toBeTruthy());
    expect(vi.mocked(api.getHealth).mock.calls.slice(opened)).toEqual([]);
  });

  it('edits from a pencil named by its action and its instance', async () => {
    show([instance()]);

    const edit = await screen.findByRole('button', { name: 'Edit – Radarr' });
    expect(edit).toHaveTextContent(/^$/);
    expect(edit).toHaveAttribute('title', 'Edit');
    expect(edit.querySelector('svg')).toBeTruthy();
  });

  it('offers to set a webhook up from its row, opening the editor on its steps', async () => {
    show([
      instance({
        webhook_path: '/api/v1/webhook/i1',
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_token: 'tok',
        webhook_check: readOf({ state: 'absent' }),
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Set up – Radarr' }));

    const dialog = await screen.findByRole('dialog', { name: 'Edit instance' });
    const webhook = within(dialog).getByText('Webhook', { selector: 'summary *' });
    expect((webhook.closest('details') as HTMLDetailsElement).open).toBe(true);
    expect(within(dialog).getByRole('button', { name: 'Copy – Webhook URL' })).toBeVisible();
  });

  /** Pre-filled with a mask, the form would save the mask as the key. */
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

  it('says why the last sync failed in the row', async () => {
    show([
      instance({
        last_sync_status: 'error: connection refused',
        last_sync_error: 'connection refused',
        last_sync_attempt_at: '2026-08-27 10:00:00',
      }),
    ]);

    const row = await screen.findByRole('row', { name: /Radarr/ });
    expect(row).toHaveTextContent('connection refused');
  });

  it('says an instance waits for its key, and how to give it one', async () => {
    show([instance({ api_key_set: false, api_key_masked: '' })]);

    const row = await screen.findByRole('row', { name: /Radarr/ });
    expect(row).toHaveTextContent('Key missing');
    expect(row).toHaveTextContent('No API key is stored. Edit the instance to enter it.');
    expect(within(row).queryByText('Encrypted')).toBeNull();
  });

  it('asks before dropping a changed instance form, and keeps it on Cancel', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    expect(unloading()).toBe(false);
    const name = await screen.findByLabelText('Name');
    await userEvent.type(name, ' 4K');

    expect(unloading()).toBe(true);
    navigate('/rules');
    expect(await answerConfirmation(null)).toBe('Close the instance without saving your changes?');
    expect(router.path).not.toBe('/rules');
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(await answerConfirmation(null)).toBe('Close the instance without saving your changes?');
    expect(name).toHaveValue('Radarr 4K');
  });

  it('closes an untouched instance form without a question', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  /** Save turns disabled while it writes, which drops its focus out of the dialog. */
  it('gives the focus back to Save once a save is refused', async () => {
    let refuse = () => {};
    vi.spyOn(api, 'updateInstance').mockReturnValue(
      new Promise((_, reject) => (refuse = () => reject(new Error('Refused')))),
    );
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const save = await screen.findByRole('button', { name: 'Save' });
    await userEvent.click(save);
    await waitFor(() => expect(save).toBeDisabled());
    dropFocus();
    refuse();

    await waitFor(() => expect(document.activeElement).toBe(save));
  });

  /** As Settings draws a number: as wide as the numbers it takes, its unit after it. */
  it('draws the sync interval as a short field, its unit after it', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const every = await screen.findByLabelText('Sync every');

    expect(every).toHaveClass('max-w-12ch');
    expect(every).toHaveAccessibleDescription('minutes');
  });

  /**
   * A number field emptied binds `null`, and the server wants an integer. Sent,
   * the save would come back 422 from serde, in English, behind the dialog.
   */
  it('will not save an instance whose sync interval was emptied', async () => {
    show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const every = await screen.findByLabelText('Sync every');
    const save = screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement;
    expect(save.disabled).toBe(false);

    await fireEvent.input(every, { target: { value: '' } });
    expect(save.disabled).toBe(true);
    await fireEvent.input(every, { target: { value: '0' } });
    expect(save.disabled).toBe(true);
    await fireEvent.input(every, { target: { value: '1441' } });
    expect(save.disabled).toBe(true);
    await fireEvent.input(every, { target: { value: '30' } });
    expect(save.disabled).toBe(false);
  });

  it.each([
    ['Sync now – Radarr', 'syncInstance', 1],
    ['Sync all', 'syncAll', 0],
  ] as const)('stops following %s once the screen closes', async (button, method, at) => {
    const sync = vi.spyOn(api, method).mockReturnValue(new Promise(() => {}) as never);
    const { unmount } = show([instance()]);

    await fireEvent.click(await screen.findByRole('button', { name: button }));
    await waitFor(() => expect(sync).toHaveBeenCalled());
    const following = nthCall(sync as unknown as { mock: { calls: unknown[][] } })[at] as
      { signal?: AbortSignal } | undefined;
    expect(following?.signal?.aborted).toBe(false);

    unmount();
    expect(following?.signal?.aborted).toBe(true);
  });

  /** The ceiling is the scheduler's, read where `backend/src/jobs/mod.rs` writes it. */
  it('says why an interval outside its bounds holds Save', async () => {
    const ceiling = backendConstant('backend/src/jobs/mod.rs', 'MAX_SYNC_INTERVAL_MINUTES');
    expect(ceiling).toBeGreaterThan(1);
    const bounds = `Enter a whole number from 1 to ${ceiling}.`;
    show([instance()]);
    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const every = await screen.findByLabelText('Sync every');
    expect(every).toHaveAttribute('max', String(ceiling));

    await fireEvent.input(every, { target: { value: '' } });

    expect(every).toHaveAttribute('aria-invalid', 'true');
    expect(every).toHaveAccessibleDescription(`minutes ${bounds}`);
    await fireEvent.input(every, { target: { value: '30' } });
    expect(every).not.toHaveAttribute('aria-invalid');
    expect(screen.queryByText(bounds)).toBeNull();
  });

  it('syncs the instance whose button was pressed', async () => {
    const sync = vi
      .spyOn(api, 'syncInstance')
      .mockResolvedValue({ media: 3, root_folders: 2 } as never);
    show([instance({ id: 'i1', name: 'Radarr' }), instance({ id: 'i2', name: 'Sonarr' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Sync now – Sonarr' }));

    await waitFor(() =>
      expect(sync).toHaveBeenCalledWith('i2', { signal: expect.any(AbortSignal) }),
    );
  });

  it('marks a disabled instance rather than hiding it', async () => {
    show([instance({ enabled: false })]);

    expect(await screen.findByText('Disabled')).toBeTruthy();
    expect(screen.getByText('Radarr')).toBeTruthy();
  });

  /** The 4K Radarr kept to proposals says so in its row, and its switch travels with the form. */
  it('marks an instance kept to proposals and saves its switch', async () => {
    const update = vi.spyOn(api, 'updateInstance').mockResolvedValue(instance());
    show([instance({ auto_apply: false })]);

    expect(await screen.findByText('Proposals only')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Edit – Radarr' }));
    const switched = (await screen.findByLabelText('Automatic application')) as HTMLInputElement;
    expect(switched.checked).toBe(false);
    expect(switched).toHaveAccessibleDescription(/automatic application is on in Settings/);
    await userEvent.click(switched);
    await fireEvent.submit(switched.closest('form') as HTMLFormElement);

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(nthCall(update)[1]).toMatchObject({ auto_apply: true });
  });

  /** An event reads as the Arr's form names its switch, one no switch sends as the Arr wrote it. */
  it('says when each instance last sent a webhook, and what to do while none has', async () => {
    const heard = new Date(Date.now() - 120_000).toISOString();
    show([
      instance({
        id: 'i1',
        name: 'Radarr',
        webhook_path: '/api/v1/webhook/i1',
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_token: 'tok',
        last_webhook_at: heard,
        last_webhook_event: 'Download',
        last_webhook_event_label: 'On File Import',
      }),
      instance({
        id: 'i2',
        name: 'Radarr 4K',
        webhook_path: '/api/v1/webhook/i2',
        webhook_url: '/api/v1/webhook/i2/tok',
        webhook_token: 'tok',
      }),
      instance({
        id: 'i3',
        name: 'Radarr Kids',
        last_webhook_at: heard,
        last_webhook_event: 'Test',
      }),
      instance({ id: 'i4', name: 'Radarr Docs', last_webhook_at: heard }),
    ]);

    const label = await screen.findByText('On File Import');
    expect(label.classList.contains('mono')).toBe(true);
    expect(label.closest('td')).toHaveTextContent('2 minutes ago');
    expect(screen.queryByText('Download')).toBeNull();
    const silent = screen.getByText('Radarr 4K').closest('tr') as HTMLElement;
    expect(within(silent).getAllByText('Never').length).toBeGreaterThan(0);
    const tested = screen.getByText('Radarr Kids').closest('tr') as HTMLElement;
    expect(within(tested).getByText('Test').classList.contains('mono')).toBe(true);
    const unnamed = screen.getByText('Radarr Docs').closest('tr') as HTMLElement;
    expect(within(unnamed).getByText('Unknown')).toBeTruthy();

    await fireEvent.click(screen.getByRole('button', { name: 'Edit – Radarr' }));
    const dialog = await screen.findByRole('dialog');
    const delivery = within(dialog).getByText('On File Import');
    expect(delivery.classList.contains('mono')).toBe(true);
    expect(delivery.parentElement).toHaveTextContent(
      'Last delivery: On File Import, 2 minutes ago.',
    );
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    await fireEvent.click(screen.getByRole('button', { name: 'Edit – Radarr 4K' }));
    expect(
      await screen.findByText('No delivery yet. Once the address is in Radarr, press Test there.'),
    ).toBeTruthy();
  });

  /**
   * A connection needing a fix is marked in its row. One not found reads "Not set up" only while
   * nothing arrived: a proxy rewriting the path hides a working one from the check.
   */
  it('marks a webhook needing a fix in its row, and one missing while none arrived', async () => {
    const heard = new Date(Date.now() - 120_000).toISOString();
    show([
      instance({
        id: 'i1',
        name: 'Radarr',
        webhook_check: readOf({
          state: 'misconfigured',
          missing_triggers: ['On Rename'],
          problems: [MISSES_RENAME],
        }),
      }),
      instance({ id: 'i2', name: 'Radarr 4K', webhook_check: readOf({ state: 'absent' }) }),
      instance({
        id: 'i3',
        name: 'Radarr Kids',
        last_webhook_at: heard,
        last_webhook_event: 'Test',
        webhook_check: readOf({ state: 'absent' }),
      }),
    ]);

    const faulty = (await screen.findByText('Needs a fix')).closest('tr') as HTMLElement;
    expect(within(faulty).getByText('Radarr')).toBeTruthy();
    expect(within(faulty).getByText('Needs a fix').classList.contains('badge-warning')).toBe(true);
    const missing = screen.getByText('Radarr 4K').closest('tr') as HTMLElement;
    expect(within(missing).getByText('Not set up')).toBeTruthy();
    const proxied = screen.getByText('Radarr Kids').closest('tr') as HTMLElement;
    expect(within(proxied).queryByText('Not set up')).toBeNull();
  });

  it("reads the Arr's connection in the editor, a try's reading over the stored one", async () => {
    const tried = vi.spyOn(api, 'probeInstance').mockResolvedValue({
      ...PROBED,
      webhook: readOf({ state: 'misconfigured', problems: [MISSES_RENAME], notes: ['Tagged.'] }),
    });
    show([
      instance({
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_path: '/api/v1/webhook/i1',
        webhook_token: 'tok',
        webhook_check: readOf(),
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const dialog = await screen.findByRole('dialog');
    expect(
      within(dialog).getByText('The webhook in Radarr sends every event Routarr acts on.'),
    ).toBeTruthy();
    expect(within(dialog).getByText('Checked 1 minute ago.')).toBeTruthy();

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));
    await waitFor(() => expect(tried).toHaveBeenCalledTimes(1));
    expect(await within(dialog).findByText(MISSES_RENAME)).toBeTruthy();
    expect(within(dialog).getByText('Tagged.')).toBeTruthy();
    expect(
      within(dialog).queryByText('The webhook in Radarr sends every event Routarr acts on.'),
    ).toBeNull();
  });

  /** The switches used often above a set-up done once, each group under its name. */
  it('groups the editor as its connection, its behaviour, then its webhook', async () => {
    show([
      instance({
        webhook_path: '/api/v1/webhook/i1',
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_token: 'tok',
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));

    const dialog = await screen.findByRole('dialog');
    const connection = within(dialog).getByRole('group', { name: 'Connection' });
    expect(within(connection).getByLabelText('Base URL')).toBeTruthy();
    expect(within(connection).getByLabelText('API key')).toBeTruthy();
    expect(within(connection).getByRole('button', { name: 'Test connectivity' })).toBeTruthy();
    const behaviour = within(dialog).getByRole('group', { name: 'Behaviour' });
    expect(within(behaviour).getByLabelText('Enabled (synced and routed)')).toBeTruthy();
    expect(within(behaviour).getByLabelText('Automatic application')).toBeTruthy();
    expect(within(behaviour).getByLabelText('Sync every')).toBeTruthy();
    const webhook = within(dialog).getByText('Webhook', { selector: 'summary *' });
    const after = (first: Node, second: Node) =>
      Boolean(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(after(connection, behaviour) && after(behaviour, webhook)).toBe(true);
    const footer = dialog.querySelector('.dialog-actions') as HTMLElement;
    expect(
      within(footer)
        .getAllByRole('button')
        .map((button) => button.textContent?.trim()),
    ).toEqual(['Cancel', 'Save']);
  });

  it('has no webhook part before the instance is saved', async () => {
    show([]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Add instance' }));

    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByRole('group', { name: 'Connection' })).toBeTruthy();
    expect(within(dialog).queryByText('Webhook', { selector: 'summary *' })).toBeNull();
  });

  /** A webhook that works is set up once, so its steps fold away under the state they reached. */
  it.each([
    ['works', { webhook_check: readOf() }, false, 'Working'],
    ['delivered', { last_webhook_at: new Date().toISOString() }, false, 'Working'],
    ['is missing', { webhook_check: readOf({ state: 'absent' }) }, true, 'Not set up'],
    [
      'needs a fix',
      { webhook_check: readOf({ state: 'misconfigured', problems: [MISSES_RENAME] }) },
      true,
      'Needs a fix',
    ],
  ])(
    'opens the webhook part by itself only while it is not working: one that %s',
    async (_, over, open, state) => {
      show([
        instance({
          webhook_path: '/api/v1/webhook/i1',
          webhook_url: '/api/v1/webhook/i1/tok',
          webhook_token: 'tok',
          ...over,
        }),
      ]);

      await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));

      const dialog = await screen.findByRole('dialog');
      const summary = within(dialog)
        .getByText('Webhook', { selector: 'summary *' })
        .closest('summary') as HTMLElement;
      expect((summary.closest('details') as HTMLDetailsElement).open).toBe(open);
      expect(summary).toHaveTextContent(state);
    },
  );

  it('sets the webhook up in numbered steps, the events named as the Arr shows them', async () => {
    show([
      instance({
        webhook_path: '/api/v1/webhook/i1',
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_token: 'tok',
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));

    const dialog = await screen.findByRole('dialog');
    const steps = within(within(dialog).getByRole('list')).getAllByRole('listitem');
    expect(steps.map((step) => step.textContent?.trim().split('.')[0])).toEqual([
      'In Radarr, open Settings → Connect, press + and choose Webhook',
      'Paste this address as its URL',
      'Turn on Show Advanced, then under Headers add X-Routarr-Token with this token',
      'Tick On File Import, On Rename, On Movie Added, On Movie Delete, On Movie File Delete',
      'Save, then press Test in Radarr',
    ]);
    expect(within(steps[1]!).getByRole('button', { name: 'Copy – Webhook URL' })).toBeTruthy();
    expect(
      within(steps[2]!).getByRole('button', {
        name: 'Copy – Token for the X-Routarr-Token header',
      }),
    ).toBeTruthy();
  });

  it.each(['Edit – Radarr', 'Add instance'])(
    'opens %s without the refusal of the last dialog',
    async (next) => {
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
      await answerConfirmation();
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

      await fireEvent.click(screen.getByRole('button', { name: next }));

      expect(within(await screen.findByRole('dialog')).queryByRole('alert')).toBeNull();
    },
  );

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
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

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

  /**
   * Both setups: the address and the token for an Arr that sends headers, which keeps the token
   * out of the logged URL, and the address carrying the token for one that does not.
   */
  it('shows both webhook setups in the editor whole, each value beside its Copy', async () => {
    show([
      instance({
        webhook_url: '/api/v1/webhook/i1/tok',
        webhook_path: '/api/v1/webhook/i1',
        webhook_token: 'tok',
      }),
    ]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));

    const origin = window.location.origin;
    const dialog = await screen.findByRole('dialog');
    const shown = (name: string) =>
      within(dialog).getByRole('button', { name: `Copy – ${name}` }).previousElementSibling
        ?.textContent;
    expect([
      shown('Webhook URL'),
      shown('Token for the X-Routarr-Token header'),
      shown('Webhook URL for an older Arr'),
    ]).toEqual([`${origin}/api/v1/webhook/i1`, 'tok', `${origin}/api/v1/webhook/i1/tok`]);
  });

  async function copyWebhookUrl() {
    show([instance({ webhook_url: '/api/v1/webhooks/i1/tok' })]);
    await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
    await fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy URL' }));
  }

  it('says the webhook URL was copied once the clipboard took it', async () => {
    let settle = () => {};
    const writeText = vi.fn(() => new Promise<void>((resolve) => (settle = resolve)));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    await copyWebhookUrl();
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(`${window.location.origin}/api/v1/webhooks/i1/tok`),
    );
    expect(screen.queryByText('Webhook URL copied to the clipboard')).toBeNull();

    settle();

    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent('Webhook URL copied to the clipboard'),
    );
  });

  it('hands the webhook URL over when the browser keeps the clipboard closed', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });

    await copyWebhookUrl();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      `${window.location.origin}/api/v1/webhooks/i1/tok`,
    );
    expect(screen.queryByText('Webhook URL copied to the clipboard')).toBeNull();
  });

  it('hands the webhook URL over when the browser refuses the copy', async () => {
    const writeText = vi.fn().mockRejectedValue(new DOMException('denied', 'NotAllowedError'));
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });

    await copyWebhookUrl();

    expect(await screen.findByRole('alert')).toHaveTextContent('/api/v1/webhooks/i1/tok');
    expect(screen.queryByText('Webhook URL copied to the clipboard')).toBeNull();
  });

  async function addInstance(name: string) {
    await fireEvent.click(await screen.findByRole('button', { name: 'Add instance' }));
    await fireEvent.input(await screen.findByLabelText('Name'), { target: { value: name } });
    const key = screen.getByLabelText('API key');
    await fireEvent.input(key, { target: { value: 'secret' } });
    await fireEvent.submit(key.closest('form') as HTMLFormElement);
  }

  it('adds an instance once however often the form is submitted', async () => {
    const create = vi.spyOn(api, 'createInstance').mockReturnValue(new Promise(() => {}));
    show([]);

    await addInstance('Films');
    const form = screen.getByLabelText('API key').closest('form') as HTMLFormElement;
    await fireEvent.submit(form);

    expect(create).toHaveBeenCalledTimes(1);
    const button = within(form).getByRole('button', { name: 'Saving…' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

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

    await waitFor(() =>
      expect(sync).toHaveBeenCalledWith('i9', { signal: expect.any(AbortSignal) }),
    );
    const done = await screen.findByText('Films is synced. Titles: 12, root folders: 2');
    expect(done.closest('.banner')?.classList.contains('banner-success')).toBe(true);
    expect(statusRevision()).toBeGreaterThan(before);
  });

  it('keeps a new instance whose first sync fails, and says why', async () => {
    vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ id: 'i9', name: 'Films' }));
    vi.spyOn(api, 'syncInstance').mockRejectedValue(
      new ApiError('Radarr answered 401 Unauthorized', 502, 'upstream'),
    );
    show([]);

    await addInstance('Films');

    const summary = await screen.findByText(
      'Films is saved, but its first sync failed. Check its address and API key, then sync it again.',
    );
    expect(summary.closest('.banner')?.classList.contains('banner-warning')).toBe(true);
    expect(screen.getByText('Radarr answered 401 Unauthorized')).toBeTruthy();
  });

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
      expect(
        screen.queryByText(
          'Films is saved, but its first sync failed. Check its address and API key, then sync it again.',
        ),
      ).toBeNull();
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
    // The sync would follow the list's reload, which the message precedes.
    await waitFor(() => expect(api.getInstances).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(sync).not.toHaveBeenCalled();
  });

  it('does not sync an instance saved disabled', async () => {
    vi.spyOn(api, 'createInstance').mockResolvedValue(instance({ enabled: false }));
    const sync = vi.spyOn(api, 'syncInstance');
    show([]);

    await addInstance('Films');

    expect(await screen.findByText('Instance added')).toBeTruthy();
    // The sync would follow the list's reload, which the message precedes.
    await waitFor(() => expect(api.getInstances).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(sync).not.toHaveBeenCalled();
  });

  const PROBED = {
    success: true,
    version: '5.2.6',
    app_name: 'Radarr',
    root_folders: 2,
    inaccessible_root_folders: 0,
    warning: null,
    webhook: null,
  };
  const CONNECTED = 'Radarr: connected (v5.2.6), root folders: 2.';

  async function openAdd(): Promise<HTMLElement> {
    await fireEvent.click(await screen.findByRole('button', { name: 'Add instance' }));
    return screen.findByRole('dialog');
  }

  async function typeAddressAndKey(url: string, key: string) {
    await fireEvent.input(await screen.findByLabelText('Base URL'), { target: { value: url } });
    await fireEvent.input(screen.getByLabelText('API key'), { target: { value: key } });
  }

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
      "With the port, and the URL base if Radarr has one. In Docker, use Radarr's container name or the host's IP address: localhost is Routarr's own container. A proxy's user name and password, written user:password@ before the host, show as ***.",
    );
    await userEvent.selectOptions(screen.getByLabelText('Type'), 'sonarr');
    expect(screen.getByLabelText('API key')).toHaveAccessibleDescription(
      'In Sonarr: Settings → General → Security → API Key.',
    );
  });

  it('warns that http sends the key unencrypted to a host past the home network', async () => {
    show([]);
    await openAdd();
    const url = await screen.findByLabelText('Base URL');
    const warning =
      'Over http://, the API key travels unencrypted. If this address is reached across the internet, use https://.';

    await fireEvent.input(url, { target: { value: 'http://radarr.example.com' } });
    const said = screen.getByText(warning);
    expect(url.getAttribute('aria-describedby')?.split(' ')).toContain(said.id);

    for (const address of ['https://radarr.example.com', 'http://radarr:7878']) {
      await fireEvent.input(url, { target: { value: address } });
      expect(screen.queryByText(warning), address).toBeNull();
      expect(url.getAttribute('aria-describedby')).toBe('instances-base-url-help');
    }
  });

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
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));
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
    const test = within(dialog).getByRole('button', { name: 'Test connectivity' });

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

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

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
    expect(create).not.toHaveBeenCalled();
  });

  it('leaves Test connectivity pressable while a try runs', async () => {
    vi.spyOn(api, 'probeInstance').mockReturnValue(new Promise(() => {}));
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');
    const test = within(dialog).getByRole('button', { name: 'Test connectivity' });

    await fireEvent.click(test);

    expect(test).toBeEnabled();
  });

  it('holds the region a try answers in before the try is pressed', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([]);
    const dialog = await openAdd();
    const region = within(dialog).getByRole('status');
    await typeAddressAndKey('http://radarr:7878', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

    await waitFor(() => expect(region).toHaveTextContent(CONNECTED));
    expect(region).not.toHaveTextContent('Not accessible');
  });

  /** A sleeping share is a folder Routarr cannot route to now, which an apply would ask about. */
  it('says how many root folders the Arr cannot reach now', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue({ ...PROBED, inaccessible_root_folders: 2 });
    show([]);
    const dialog = await openAdd();
    const region = within(dialog).getByRole('status');
    await typeAddressAndKey('http://radarr:7878', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

    await waitFor(() => expect(region).toHaveTextContent(CONNECTED));
    expect(within(region).getByText('Not accessible to Radarr now: 2.')).toBeTruthy();
  });

  it('says beside a working connection what an older Arr lacks', async () => {
    const older = 'Sonarr 3.0.10 is older than 4: a series it adds is routed at the next sync.';
    vi.spyOn(api, 'probeInstance').mockResolvedValue({ ...PROBED, warning: older });
    show([]);
    const dialog = await openAdd();
    const region = within(dialog).getByRole('status');
    await typeAddressAndKey('http://sonarr:8989', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

    await waitFor(() => expect(region).toHaveTextContent(CONNECTED));
    expect(region).toHaveTextContent(older);
  });

  it('shows why a try failed inside the dialog, until a value it tried changes', async () => {
    vi.spyOn(api, 'probeInstance').mockRejectedValue(
      new ApiError('Nothing answers at http://localhost:7878.', 502, 'external_api_error'),
    );
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://localhost:7878', 'secret');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

    expect(await within(dialog).findByRole('alert')).toHaveTextContent(
      'Nothing answers at http://localhost:7878.',
    );
    await fireEvent.input(screen.getByLabelText('Base URL'), {
      target: { value: 'http://radarr:7878' },
    });
    expect(within(dialog).queryByRole('alert')).toBeNull();
  });

  it('tries an edit with the stored key when its field is left blank', async () => {
    const probe = vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([instance({ id: 'i1' })]);

    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Radarr' }));
    const dialog = await screen.findByRole('dialog');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));

    await waitFor(() =>
      expect(probe).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'i1', api_key: '' }),
        expect.any(AbortSignal),
      ),
    );
  });

  it('holds a try to the address, the key and the type it tried', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));
    const result = within(dialog).getByRole('status');
    await waitFor(() => expect(result).toHaveTextContent(CONNECTED));
    const url = screen.getByLabelText('Base URL');
    const key = screen.getByLabelText('API key');

    await fireEvent.input(url, { target: { value: 'http://radarr:7879' } });
    expect(result).toBeEmptyDOMElement();
    await fireEvent.input(url, { target: { value: 'http://radarr:7878' } });
    expect(result).toHaveTextContent(CONNECTED);

    await fireEvent.input(key, { target: { value: 'another' } });
    expect(result).toBeEmptyDOMElement();
    await fireEvent.input(key, { target: { value: 'secret' } });
    expect(result).toHaveTextContent(CONNECTED);

    await userEvent.selectOptions(screen.getByLabelText('Type'), 'sonarr');
    expect(result).toBeEmptyDOMElement();
  });

  it('shows one outcome at a time in the dialog', async () => {
    vi.spyOn(api, 'probeInstance').mockResolvedValue(PROBED);
    vi.spyOn(api, 'createInstance').mockRejectedValue(
      new ApiError('Give the instance a name.', 400, 'bad_request'),
    );
    show([]);
    const dialog = await openAdd();
    await typeAddressAndKey('http://radarr:7878', 'secret');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));
    const result = within(dialog).getByRole('status');
    await waitFor(() => expect(result).toHaveTextContent(CONNECTED));

    const form = screen.getByLabelText('Base URL').closest('form') as HTMLFormElement;
    await fireEvent.submit(form);

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Give the instance a name.');
    expect(result).toBeEmptyDOMElement();
  });

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
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Test connectivity' }));
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await fireEvent.click(await screen.findByRole('button', { name: 'Edit – Sonarr' }));
    dialog = await screen.findByRole('dialog');
    fail(new ApiError('Nothing answers at http://localhost:7878.', 502, 'external_api_error'));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(within(dialog).queryByRole('alert')).toBeNull();
  });

  it('links each address to the Arr it names, in a new tab', async () => {
    show([instance({ base_url: 'http://radarr.home.arpa' })]);

    const link = await screen.findByRole('link', { name: /http:\/\/radarr\.home\.arpa/ });
    expect(link.getAttribute('href')).toBe('http://radarr.home.arpa');
    expect(link.getAttribute('target')).toBe('_blank');
    expect(link.getAttribute('rel')).toContain('noopener');
    expect(link).toHaveAccessibleName(/opens in a new tab/);
  });

  /**
   * The server answers a proxy's credentials masked. Left in the link, a
   * browser would offer to sign in as `***`, so the link goes without them.
   */
  it('links an address behind a proxy without its masked credentials', async () => {
    show([
      instance({ id: 'i1', base_url: 'http://***@radarr.home.arpa' }),
      instance({ id: 'i2', name: 'Sonarr', base_url: 'http://***@[fd00::10]:8989' }),
    ]);

    const link = await screen.findByRole('link', { name: /\*\*\*@radarr\.home\.arpa/ });
    expect(link.getAttribute('href')).toBe('http://radarr.home.arpa');
    expect(screen.getByRole('link', { name: /fd00::10/ }).getAttribute('href')).toBe(
      'http://[fd00::10]:8989',
    );
  });

  it('offers no link for a container name, and one for any other host', async () => {
    show([
      instance({ id: 'i1', base_url: 'http://radarr:7878' }),
      instance({ id: 'i2', name: 'Sonarr', base_url: 'http://[fd00::10]:8989' }),
    ]);

    expect(await screen.findByText('http://radarr:7878')).toBeTruthy();
    expect(screen.queryByRole('link', { name: /radarr:7878/ })).toBeNull();
    expect(screen.getByRole('link', { name: /fd00::10/ })).toBeTruthy();
  });

  /** The backend refuses any other scheme, and the table does not rely on it. */
  it('links only an http or https address', async () => {
    show([instance({ base_url: 'javascript:alert(1)' })]);

    expect(await screen.findByText('javascript:alert(1)')).toBeTruthy();
    expect(screen.queryByRole('link', { name: /javascript/ })).toBeNull();
  });

  it('opens the Add dialog when the guide sends the reader here', async () => {
    window.history.replaceState({}, '', '/instances?add=1');
    show([]);

    expect(await screen.findByRole('dialog')).toBeTruthy();
    expect(window.location.search).toBe('');
  });

  it('hands the focus to Add instance when the dialog the guide opened closes', async () => {
    window.history.replaceState({}, '', '/instances?add=1');
    show([]);
    const dialog = await screen.findByRole('dialog');

    await fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Add instance' }));
  });

  it('opens the form on its first field', async () => {
    show([]);
    await openAdd();

    expect(document.activeElement).toBe(screen.getByLabelText('Name'));
  });
});

it('hands the focus to the instance that took the place of the deleted one', async () => {
  const next = instance({ id: 'i2', name: 'Sonarr', instance_type: 'sonarr' });
  show([instance(), next]);
  vi.spyOn(api, 'deleteInstance').mockResolvedValue(undefined);

  await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
  await fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
  vi.spyOn(api, 'getInstances').mockResolvedValue([next]);
  await answerConfirmation();

  await waitFor(() =>
    expect(document.activeElement).toBe(screen.getByRole('button', { name: /Actions – Sonarr/ })),
  );
});

it('tests the connection of the instance whose row was used', async () => {
  const sonarr = instance({ id: 'i2', name: 'Sonarr', instance_type: 'sonarr' });
  show([instance(), sonarr]);
  const probe = vi.spyOn(api, 'testInstance').mockResolvedValue({
    success: true,
    version: '4.0.1',
    app_name: 'Sonarr',
    root_folders: 2,
    inaccessible_root_folders: 0,
    warning: null,
    webhook: null,
  });

  await fireEvent.click(await screen.findByRole('button', { name: /Actions – Sonarr/ }));
  await fireEvent.click(await screen.findByRole('menuitem', { name: 'Test connectivity' }));

  expect(await screen.findByText('Sonarr: connected (v4.0.1), root folders: 2.')).toBeTruthy();
  expect(screen.queryByText(/Not accessible/)).toBeNull();
  expect(probe).toHaveBeenCalledExactlyOnceWith('i2');
});

/** A warning, as the form draws the sentence. */
it('says from the row how many root folders the Arr cannot reach now', async () => {
  show([instance()]);
  vi.spyOn(api, 'testInstance').mockResolvedValue({
    success: true,
    version: '5.2.6',
    app_name: 'Radarr',
    root_folders: 3,
    inaccessible_root_folders: 2,
    warning: null,
    webhook: null,
  });

  await fireEvent.click(await screen.findByRole('button', { name: /Actions – Radarr/ }));
  await fireEvent.click(await screen.findByRole('menuitem', { name: 'Test connectivity' }));

  const said = await screen.findByText(
    'Radarr: connected (v5.2.6), root folders: 3. Not accessible to Radarr now: 2.',
  );
  expect(said.closest('.banner')).toHaveClass('banner-warning');
});

/** Every action on an instance writes or reaches the Arr, which a reader is refused. */
describe('for a reader', () => {
  const offered = () =>
    [
      'Sync all',
      'Add instance',
      'Sync now – Radarr',
      'Edit – Radarr',
      'Set up – Radarr',
      'Actions – Radarr',
    ].filter((name) => screen.queryByRole('button', { name }));
  const silent = () =>
    instance({ webhook_url: '/w', webhook_path: '/w', webhook_check: readOf({ state: 'absent' }) });

  it('offers the owner every action on an instance, and a reader none', async () => {
    const { unmount } = show([silent()]);
    await screen.findByRole('button', { name: 'Edit – Radarr' });
    expect(offered()).toHaveLength(6);
    unmount();

    publishAccess(true);
    show([silent()]);

    expect(await screen.findByRole('row', { name: /Radarr/ })).toBeInTheDocument();
    expect(offered()).toEqual([]);
  });

  it('opens no form for a reader sent with ?add=1', async () => {
    window.history.replaceState({}, '', '/instances?add=1');
    publishAccess(true);
    show([instance()]);

    await screen.findByRole('row', { name: /Radarr/ });
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
