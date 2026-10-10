import { describe, it, expect, vi } from 'vitest';
import { fireEvent, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { CARD_HELP, HELP, TERMS } from '../lib/help';
import { screenKey } from '../lib/routes';
import HelpPanel from './HelpPanel.svelte';

/**
 * The help opens on the screen on display: what it is for, its terms, the
 * problems met there with a way to the screen that fixes each, and its cards.
 * A search runs through every screen's help, and the glossary lists every
 * term with the screens that use it.
 */

const RULES = HELP['/rules']!;
const PROBLEM =
  HELP['/rules']!.problems.find((problem) => problem.to) ?? HELP['/rules']!.problems[0]!;
const TERM = RULES.terms[0]!;
const STRINGS = {
  Help: 'Help',
  HelpTitle: 'Help: {screen}',
  HelpSearchLabel: 'Search the help',
  HelpSearchPlaceholder: 'Search…',
  HelpResults: 'Results: {count}',
  HelpNoResults: 'Nothing matches "{query}".',
  HelpThisScreen: 'This screen',
  HelpGlossary: 'Glossary',
  HelpTerms: 'Terms',
  HelpProblems: 'When something goes wrong',
  HelpCards: 'On this screen',
  HelpFixOn: 'Open {screen}',
  HelpUsedOn: 'Used on:',
  ListSeparator: ', ',
  Dismiss: 'Dismiss',
  RulesEngine: 'Rules',
  Dashboard: 'Dashboard',
  Instances: 'Instances',
  [RULES.purpose]: 'Rules decide where each title goes.',
  [TERMS[TERM]!.name]: 'Quokka',
  [TERMS[TERM]!.text]: 'A word only this test uses.',
  [PROBLEM.problem]: 'A rule matches nothing.',
  [PROBLEM.fix]: 'Check its conditions.',
};

const show = (path = '/rules', onClose = vi.fn(), screenAt?: string) =>
  renderWithI18n(HelpPanel, { props: { path, screen: screenAt, onClose }, strings: STRINGS });

describe('HelpPanel', () => {
  it('opens on the screen on display, with what it is for, its terms and its problems', () => {
    show();

    expect(screen.getByRole('dialog', { name: 'Help: Rules' })).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'This screen' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(screen.getByRole('heading', { level: 3, name: 'Rules' })).toBeInTheDocument();
    expect(screen.getByText('Rules decide where each title goes.')).toBeInTheDocument();
    expect(screen.getByText('Quokka')).toBeInTheDocument();
    expect(screen.getByText('A rule matches nothing.')).toBeInTheDocument();
    expect(screen.getAllByRole('term').length).toBe(RULES.terms.length + RULES.cards.length);
  });

  it('leads from a problem to the screen that fixes it, and closes on the way', async () => {
    const onClose = vi.fn();
    show('/rules', onClose);
    if (!PROBLEM.to) return;

    const target = screenKey(PROBLEM.to);
    const link = screen.getByRole('link', {
      name: `Open ${(STRINGS as Record<string, string>)[target] ?? target}`,
    });
    expect(link.getAttribute('href')).toContain(PROBLEM.to);
    await fireEvent.click(link);
    expect(onClose).toHaveBeenCalled();
  });

  it('opens on the screen the quick search found, rather than the one on display', () => {
    show('/rules', vi.fn(), '/instances');

    expect(screen.getByRole('dialog', { name: 'Help: Instances' })).toBeInTheDocument();
  });

  it('searches every screen and shows the help of the one a result belongs to', async () => {
    show('/instances');
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search the help' }), 'quokka');

    expect(screen.getByRole('status')).toHaveTextContent('Results: 1');
    await userEvent.click(screen.getByRole('button', { name: /Quokka/ }));

    expect(screen.getByRole('heading', { level: 3, name: 'Rules' })).toBeInTheDocument();
    expect(screen.getByRole('searchbox', { name: 'Search the help' })).toHaveValue('');
  });

  it('says when nothing matches', async () => {
    show();
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search the help' }), 'zzzz');

    expect(screen.getByText('Nothing matches "zzzz".')).toBeInTheDocument();
  });

  it('lists every term in the glossary, each with the screens that use it', async () => {
    show('/instances');
    await userEvent.click(screen.getByRole('tab', { name: 'Glossary' }));

    expect(screen.getAllByRole('term')).toHaveLength(Object.keys(TERMS).length);
    const quokka = screen.getByText('Quokka').nextElementSibling as HTMLElement;
    await userEvent.click(within(quokka).getByRole('button', { name: 'Rules' }));
    expect(screen.getByRole('heading', { level: 3, name: 'Rules' })).toBeInTheDocument();
  });

  it('walks its tabs with the arrow keys', async () => {
    show();
    const tab = screen.getByRole('tab', { name: 'This screen' });
    tab.focus();
    await userEvent.keyboard('{ArrowRight}');

    expect(screen.getByRole('tab', { name: 'Glossary' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Glossary' })).toHaveFocus();
  });

  it('closes on its close button', async () => {
    const onClose = vi.fn();
    show('/rules', onClose);
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('names each card of the screen with what its "?" says', () => {
    show();

    for (const id of RULES.cards) {
      expect(screen.getByText(CARD_HELP[id]!.title)).toBeInTheDocument();
    }
  });
  it('leaves out what a screen has none of, and links only a fix made elsewhere', () => {
    const bare = Object.keys(HELP).find((path) => HELP[path]!.cards.length === 0)!;
    show(bare);

    expect(screen.queryByRole('heading', { name: 'On this screen' })).toBeNull();
    const here = HELP[bare]!.problems.filter((problem) => !problem.to).length;
    expect(screen.queryAllByRole('link')).toHaveLength(HELP[bare]!.problems.length - here);
  });

  it('walks back with the left arrow, and leaves the other keys to the tab', async () => {
    show();
    const tab = screen.getByRole('tab', { name: 'This screen' });
    tab.focus();
    await userEvent.keyboard('{Enter}');
    expect(tab).toHaveAttribute('aria-selected', 'true');

    await userEvent.keyboard('{ArrowLeft}');
    expect(screen.getByRole('tab', { name: 'Glossary' })).toHaveAttribute('aria-selected', 'true');
    await userEvent.keyboard('{ArrowLeft}');
    expect(tab).toHaveAttribute('aria-selected', 'true');
  });

  it('separates the screens a term is used on', async () => {
    const shared = Object.keys(TERMS).find(
      (id) => Object.values(HELP).filter((help) => help.terms.includes(id)).length > 1,
    )!;
    renderWithI18n(HelpPanel, {
      props: { path: '/rules', onClose: vi.fn() },
      strings: { ...STRINGS, [TERMS[shared]!.name]: 'Wombat' },
    });
    await userEvent.click(screen.getByRole('tab', { name: 'Glossary' }));

    const entry = screen.getByText('Wombat').nextElementSibling as HTMLElement;
    expect(within(entry).getAllByRole('button').length).toBeGreaterThan(1);
    expect(entry).toHaveTextContent(', ');
  });
});
