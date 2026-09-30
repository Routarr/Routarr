import type { Key } from './i18n';

/**
 * The API page's places, in the page's own order, for the index: how a call
 * is made, then what to do with it, then the reference to look things up in.
 */
export const API_SECTIONS: { number: string; href: string; label: Key; note: Key }[] = [
  { number: '01', href: '#calls', label: 'api.calls.span', note: 'header.desc.api' },
  { number: '02', href: '#recipes', label: 'api.recipes.span', note: 'header.desc.api.2' },
  { number: '03', href: '#reference', label: 'api.reference.span', note: 'header.desc.api.3' },
  { number: '04', href: '#schemas', label: 'api.schemas.span', note: 'header.desc.api.4' },
];
