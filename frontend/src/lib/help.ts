import { CARD_HELP, HELP, TERMS } from './help-content';
import { SCREENS, screenKey } from './routes';

export { CARD_HELP, HELP, TERMS };

/**
 * The help Routarr gives about itself: for each screen, what it is for, the
 * terms it uses and the problems met there with their fix, a few sentences
 * for each card, and the glossary every term belongs to. Every value is a
 * dictionary key, so the help is in the reader's language and needs no
 * network. `help-content.ts` holds the entries, and `help.test.ts` refuses a
 * screen, a card or a term left without one.
 */
export interface Problem {
  problem: string;
  fix: string;
  /** The screen where the fix is made, when it is not the screen itself. */
  to?: string;
}

export interface ScreenHelp {
  purpose: string;
  /** Ids of `TERMS`. */
  terms: readonly string[];
  problems: readonly Problem[];
  /** Ids of `CARD_HELP`: the cards it shows. */
  cards: readonly string[];
}

export interface CardHelp {
  /** The key of the card's title, as the panel names the card. */
  title: string;
  /** The key of what the card's "?" says. */
  text: string;
}

export interface Term {
  /** The key of the word the interface uses for it. */
  name: string;
  /** The key of its definition. */
  text: string;
}

/** The screen whose help is shown for an address: the dashboard's for one no screen answers. */
export function helpScreen(path: string): string {
  return SCREENS.includes(path) ? path : '/';
}

/** Lower case, with the marks a reader may leave out removed: "écran" is found by "ecran". */
function folded(text: string): string {
  return text.normalize('NFD').replace(/\p{M}/gu, '').toLocaleLowerCase();
}

export interface Hit {
  screen: string;
  kind: 'purpose' | 'term' | 'problem' | 'card';
  /** What the hit is about: the screen, the term, the symptom or the card's title. */
  title: string;
  text: string;
}

/**
 * Everything the help says that holds every word of `query`, a hit in a title
 * before one in a text, each in the order of the screens. A term is listed
 * under the first screen that uses it, once.
 */
export function searchHelp(query: string, translate: (key: string) => string): Hit[] {
  const words = folded(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const holds = (text: string) => {
    const haystack = folded(text);
    return words.every((word) => haystack.includes(word));
  };
  const titled: Hit[] = [];
  const texted: Hit[] = [];
  const consider = (hit: Hit) => {
    if (holds(hit.title)) titled.push(hit);
    else if (holds(`${hit.title} ${hit.text}`)) texted.push(hit);
  };
  const seen = new Set<string>();
  for (const screen of SCREENS) {
    const help = HELP[screen]!;
    consider({
      screen,
      kind: 'purpose',
      title: translate(screenKey(screen)),
      text: translate(help.purpose),
    });
    for (const id of help.terms) {
      if (seen.has(id)) continue;
      seen.add(id);
      const term = TERMS[id]!;
      consider({ screen, kind: 'term', title: translate(term.name), text: translate(term.text) });
    }
    for (const problem of help.problems) {
      consider({
        screen,
        kind: 'problem',
        title: translate(problem.problem),
        text: translate(problem.fix),
      });
    }
    for (const id of help.cards) {
      const card = CARD_HELP[id]!;
      consider({ screen, kind: 'card', title: translate(card.title), text: translate(card.text) });
    }
  }
  return [...titled, ...texted];
}

export interface GlossaryEntry {
  id: string;
  name: string;
  text: string;
  /** The screens that use it, in the order of the navigation. */
  screens: string[];
}

/** Every term, in the reader's alphabetical order, with the screens that use it. */
export function glossary(translate: (key: string) => string, language: string): GlossaryEntry[] {
  const collator = new Intl.Collator(language, { sensitivity: 'base' });
  return Object.entries(TERMS)
    .map(([id, term]) => ({
      id,
      name: translate(term.name),
      text: translate(term.text),
      screens: SCREENS.filter((screen) => HELP[screen]!.terms.includes(id)),
    }))
    .sort((a, b) => collator.compare(a.name, b.name));
}
