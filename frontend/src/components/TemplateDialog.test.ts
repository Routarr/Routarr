import { describe, it, expect, vi, afterEach } from 'vitest';
import { cleanup, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { renderWithI18n } from '../test/render';
import { answerConfirmation } from '../test/confirm';
import { dropFocus } from '../test/focus';
import { template, templatePreview } from '../test/fixtures';
import { ApiError, api } from '../api/client';
import { publishAccess } from '../lib/access.svelte';
import TemplateDialog from './TemplateDialog.svelte';
import type { Facet, TemplateSummary, TemplateTitle } from '../api/types';

afterEach(() => vi.restoreAllMocks());

function open(shown: TemplateSummary, profileOptions: Facet[] = []) {
  const onAdded = vi.fn();
  const onClose = vi.fn();
  renderWithI18n(TemplateDialog, {
    props: {
      template: shown,
      categories: [],
      languageOptions: [],
      profileOptions,
      onClose,
      onAdded,
    },
  });
  return { onAdded, onClose };
}

function title(name: string, action: TemplateTitle['action']): TemplateTitle {
  return {
    media_id: name,
    media_title: name,
    media_type: 'movie',
    instance_name: 'Radarr',
    category: 'docs',
    rule_name: 'Documentaries',
    action,
  };
}

/** What the preview's figure says, once it has an answer. */
async function reached() {
  const status = await screen.findByRole('status');
  await waitFor(() => expect(status.querySelector('.template-reach')).not.toBeNull());
  return status.textContent?.replace(/\s+/g, ' ').trim();
}

/** The figures under it, each as its value and its name. */
function figures() {
  return [...document.querySelectorAll('.template-metrics .metric')].map((metric) =>
    metric.textContent?.replace(/\s+/g, ' ').trim(),
  );
}

/** The rules keeping titles, each with how many. */
function kept() {
  return [...document.querySelectorAll('.template-kept li')].map((item) =>
    item.textContent?.replace(/\s+/g, ' ').trim(),
  );
}

describe('TemplateDialog', () => {
  it('says what it would do in the category left, then adds it there', async () => {
    const previewed = vi.spyOn(api, 'previewTemplate').mockResolvedValue(
      templatePreview({
        creates: ['docs'],
        reach: { titles: 3, to_move: 2, in_place: 1, unmapped: 0 },
        titles: [title('Free Solo', 'move'), title('Senna', 'none')],
        taken_by: [{ rule_id: 'r1', rule_name: 'Mine', titles: 2 }],
      }),
    );
    const added = { rules: [], created: ['docs'], replaced: 0 };
    const imported = vi.spyOn(api, 'importTemplate').mockResolvedValue(added);
    const { onAdded } = open(template());
    await waitFor(() => expect(previewed).toHaveBeenCalledTimes(1));

    const field = screen.getByLabelText('Category');
    await userEvent.clear(field);
    await userEvent.type(field, 'docs');
    expect(previewed).toHaveBeenCalledTimes(1);
    await userEvent.tab();

    await waitFor(() =>
      expect(previewed).toHaveBeenLastCalledWith(
        'documentaries',
        { category: 'docs', replace: false },
        expect.anything(),
      ),
    );
    expect(await reached()).toBe('5 Titles it recognises in your library');
    expect(figures()).toEqual(['2 To move', '1 Already in place', '2 Kept by your rules']);
    expect(kept()).toEqual(['Mine 2']);
    expect(screen.getByText(/The category "docs" will be created/)).toBeTruthy();
    const rows = within(screen.getByRole('table', { name: 'Titles it recognises' })).getAllByRole(
      'row',
    );
    expect(rows[1]?.textContent).toContain('Free Solo');
    expect(rows[1]?.textContent).toContain('To move');
    expect(screen.getByText('Titles shown: 2 of 3')).toBeTruthy();

    await userEvent.click(screen.getByRole('button', { name: 'Add' }));

    expect(imported).toHaveBeenCalledWith('documentaries', { category: 'docs', replace: false });
    expect(onAdded).toHaveBeenCalledWith(added);
  });

  /** Every title matched kept by an earlier rule is still a title it recognises. */
  it('counts the titles a rule placed before keeps, and says how to hand them over', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(
      templatePreview({
        taken_by: [
          { rule_id: 'r1', rule_name: 'Japanese animation', titles: 4 },
          { rule_id: 'r2', rule_name: 'Family and kids', titles: 1 },
        ],
      }),
    );
    open(template());

    expect(await reached()).toBe('5 Titles it recognises in your library');
    expect(figures()).toEqual(['5 Kept by your rules']);
    expect(kept()).toEqual(['Japanese animation 4', 'Family and kids 1']);
    expect(screen.getByText(/place its rules above yours/)).toBeTruthy();
  });

  it('says so when it recognises no title yet', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    open(template());

    expect(await screen.findByText('It recognises no title in your library yet.')).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
  });

  it('offers again what the template was added with, and asks before replacing it', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    const imported = vi
      .spyOn(api, 'importTemplate')
      .mockResolvedValue({ rules: [], created: [], replaced: 1 });
    open(template({ added_version: 1, rule_ids: ['r1'], category: 'docs' }));

    expect((screen.getByLabelText('Category') as HTMLInputElement).value).toBe('docs');
    await userEvent.click(screen.getByRole('button', { name: 'Replace' }));
    expect(imported).not.toHaveBeenCalled();
    await answerConfirmation();

    await waitFor(() =>
      expect(imported).toHaveBeenCalledWith('documentaries', { category: 'docs', replace: true }),
    );
  });

  it('says where requester tags come from when the library carries none', () => {
    const previewed = vi.spyOn(api, 'previewTemplate');
    open(template({ id: 'requesters', parameter: 'requesters', category: null }));

    expect(screen.getByText(/enable Tag Requests in the Radarr and Sonarr settings/)).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Add' }).hasAttribute('disabled')).toBe(true);
    expect(previewed).not.toHaveBeenCalled();
  });

  function routed() {
    return template({
      id: 'requesters',
      parameter: 'requesters',
      category: null,
      added_version: 1,
      requesters: [
        { tag: '3-alice', name: 'alice', titles: 4, category: 'alice-asks', added: true },
        { tag: '4-bob', name: 'bob', titles: 1, category: 'bob', added: false },
      ],
    });
  }

  /** Updating sends everyone ticked, and the backend keeps the rules of those already routed. */
  it('ticks the requesters already routed and updates with the one added, without asking', async () => {
    const previewed = vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    const imported = vi
      .spyOn(api, 'importTemplate')
      .mockResolvedValue({ rules: [], created: [], replaced: 0 });
    open(routed());
    expect((screen.getByLabelText('alice, titles: 4') as HTMLInputElement).checked).toBe(true);
    expect(screen.getByLabelText('Category for alice').hasAttribute('disabled')).toBe(true);
    const bob = screen.getByLabelText('Category for bob');
    expect(bob.hasAttribute('disabled')).toBe(true);
    const update = screen.getByRole('button', { name: 'Update' });
    expect(update.hasAttribute('disabled')).toBe(true);

    await userEvent.click(screen.getByLabelText('bob, titles: 1'));

    const sent = {
      requesters: [
        { tag: '3-alice', category: 'alice-asks' },
        { tag: '4-bob', category: 'bob' },
      ],
      replace: true,
    };
    await waitFor(() =>
      expect(previewed).toHaveBeenLastCalledWith('requesters', sent, expect.anything()),
    );
    expect(bob.hasAttribute('disabled')).toBe(false);
    await userEvent.click(update);
    expect(imported).toHaveBeenCalledWith('requesters', sent);
  });

  /** Replacing sends every person ticked: one left unticked loses their rule. */
  it('asks before removing the rule of a requester unticked', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    const imported = vi
      .spyOn(api, 'importTemplate')
      .mockResolvedValue({ rules: [], created: [], replaced: 1 });
    open(routed());

    await userEvent.click(screen.getByLabelText('alice, titles: 4'));
    await userEvent.click(screen.getByLabelText('bob, titles: 1'));
    await userEvent.click(screen.getByRole('button', { name: 'Update' }));

    expect(imported).not.toHaveBeenCalled();
    expect(await answerConfirmation()).toBe('The rules routing alice will be removed. Go on?');
    await waitFor(() =>
      expect(imported).toHaveBeenCalledWith('requesters', {
        requesters: [{ tag: '4-bob', category: 'bob' }],
        replace: true,
      }),
    );
  });

  it('asks with the languages the household watches in, under a visible label', async () => {
    const previewed = vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    open(
      template({
        id: 'foreign-films',
        parameter: 'languages',
        category: 'foreign-films',
        languages: ['fr', 'en'],
      }),
    );

    const label = screen.getByText('Languages you watch in', { selector: 'label' });
    expect(label.getAttribute('for')).toBe('template-languages');
    await waitFor(() =>
      expect(previewed).toHaveBeenLastCalledWith(
        'foreign-films',
        { category: 'foreign-films', languages: ['fr', 'en'], replace: false },
        expect.anything(),
      ),
    );
  });

  it('asks for the 4K profiles among those the instances define, and sends the ones chosen', async () => {
    const previewed = vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    const profiles: Facet[] = [
      { value: 'Ultra-HD', count: 3 },
      { value: 'HD-1080p', count: 40 },
    ];
    open(
      template({
        id: '4k',
        parameter: 'quality_profiles',
        category: '4k',
        quality_profiles: ['Ultra-HD'],
      }),
      profiles,
    );

    const label = screen.getByText('4K quality profiles', { selector: 'label' });
    expect(label.getAttribute('for')).toBe('template-profiles');
    await waitFor(() =>
      expect(previewed).toHaveBeenLastCalledWith(
        '4k',
        { category: '4k', quality_profiles: ['Ultra-HD'], replace: false },
        expect.anything(),
      ),
    );
  });

  it('says to sync first when no quality profile is known, and adds nothing', () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    open(
      template({ id: '4k', parameter: 'quality_profiles', category: '4k', quality_profiles: [] }),
    );

    expect(
      screen.getByText(
        'No quality profile is known yet. Sync an instance, then come back to choose its 4K profiles.',
      ),
    ).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Add' })).toBeDisabled();
  });

  it('keeps the dialog open with the refusal when adding fails', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    let refuse: () => void = () => {};
    vi.spyOn(api, 'importTemplate').mockReturnValue(
      new Promise((_, reject) => {
        refuse = () => reject(new ApiError('The category name is refused.', 400, 'bad_request'));
      }),
    );
    const { onAdded } = open(template());

    await userEvent.click(screen.getByRole('button', { name: 'Add' }));
    dropFocus();
    refuse();

    expect(await screen.findByText('The category name is refused.')).toBeTruthy();
    expect(onAdded).not.toHaveBeenCalled();
    // Add turned disabled while it asked, and is the way to try again.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Add' })),
    );
  });

  /** A pointer has no Escape. */
  it('closes from the button beside its title', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    const { onClose } = open(template());

    const dialog = screen.getByRole('dialog', { name: template().name });
    expect(within(dialog).getByRole('heading', { level: 2 })).toHaveTextContent(template().name);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Dismiss' }));

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('for a reader', () => {
  /** Adding a template writes rules, which a reader is refused. */
  it('offers the add to the owner and not to a reader', async () => {
    vi.spyOn(api, 'previewTemplate').mockResolvedValue(templatePreview());
    open(template());
    expect(await screen.findByRole('button', { name: 'Add' })).toBeInTheDocument();
    cleanup();

    publishAccess(true);
    open(template());

    await screen.findByRole('button', { name: 'Cancel' });
    expect(screen.queryByRole('button', { name: 'Add' })).toBeNull();
  });
});
