import { bcp47 } from '../api/format';
import { CARD_HELP, HELP, TERMS } from './help-content';
import { SCREENS, screenKey } from './routes';

export { CARD_HELP, HELP, TERMS };

/**
 * The help Routarr gives about itself, every value a dictionary key: per screen its purpose,
 * terms, problems and cards, and the glossary. `help-content.ts` holds the entries.
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

/**
 * Lower case whatever the browser's locale, with the marks a reader may leave out removed:
 * "écran" is found by "ecran", and Turkish "ışık" by "isik".
 */
export function folded(text: string): string {
  return text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase().replaceAll('ı', 'i');
}

interface Hit {
  screen: string;
  kind: 'purpose' | 'term' | 'problem' | 'card';
  /** What the hit is about: the screen, the term, the symptom or the card's title. */
  title: string;
  text: string;
}

/** Scripts that set no space between words, where any place may start one. */
const UNSPACED = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Thai}]/u;
const WORD_CHARACTER = /[\p{L}\p{N}]/u;

/** Whether `word` starts a word of `text`, both folded. */
function startsAWord(text: string, word: string): boolean {
  if (UNSPACED.test(word)) return text.includes(word);
  for (let at = text.indexOf(word); at !== -1; at = text.indexOf(word, at + 1)) {
    if (at === 0 || !WORD_CHARACTER.test(text[at - 1]!)) return true;
  }
  return false;
}

/**
 * Everything the help says that holds every word of `query`, ranked: at a word's start before
 * inside a word (an Arabic or German word may sit inside another), then a title before a text,
 * then the screens' order. A term is listed under the first screen that uses it, once.
 */
export function searchHelp(query: string, translate: (key: string) => string): Hit[] {
  const words = folded(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const ranked: Hit[][] = [[], [], [], []];
  const consider = (hit: Hit) => {
    const [title, text] = [folded(hit.title), folded(hit.text)];
    const both = `${title} ${text}`;
    if (!words.every((word) => both.includes(word))) return;
    const starting = words.every((word) => startsAWord(both, word));
    const inTitle = starting
      ? words.every((word) => startsAWord(title, word))
      : words.every((word) => title.includes(word));
    ranked[(starting ? 0 : 2) + (inTitle ? 0 : 1)]!.push(hit);
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
  return ranked.flat();
}

interface GlossaryEntry {
  id: string;
  name: string;
  text: string;
  /** The screens that use it, in the order of the navigation. */
  screens: string[];
}

function collatorFor(language: string): Intl.Collator {
  try {
    return new Intl.Collator(bcp47(language), { sensitivity: 'base' });
  } catch {
    return new Intl.Collator(undefined, { sensitivity: 'base' });
  }
}

/** Every term, in the reader's alphabetical order, with the screens that use it. */
export function glossary(translate: (key: string) => string, language: string): GlossaryEntry[] {
  const collator = collatorFor(language);
  return Object.entries(TERMS)
    .map(([id, term]) => ({
      id,
      name: translate(term.name),
      text: translate(term.text),
      screens: SCREENS.filter((screen) => HELP[screen]!.terms.includes(id)),
    }))
    .sort((a, b) => collator.compare(a.name, b.name));
}
