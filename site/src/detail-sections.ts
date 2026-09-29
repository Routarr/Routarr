import type { Key } from './i18n';

/**
 * The detail page's places, in the page's own order: the index lists them, and
 * the landing's last card offers them. An index that numbers 01, 02, 03 down a
 * page whose sections come in another order sends the reader backwards. `#how`
 * is a block inside the gap's section rather than a section of its own, and is
 * listed because it is still a place.
 */
export const DETAIL_SECTIONS: { number: string; href: string; label: Key; note: Key }[] = [
  { number: '01', href: '#why', label: 'why.span', note: 'header.desc.7' },
  { number: '02', href: '#how', label: 'header.li', note: 'header.desc' },
  { number: '03', href: '#explain', label: 'explain.span', note: 'header.desc.8' },
  { number: '04', href: '#folders', label: 'folders.span', note: 'header.desc.9' },
  { number: '05', href: '#features', label: 'header.li.3', note: 'header.desc.3' },
  { number: '06', href: '#faq', label: 'header.li.6', note: 'header.desc.6' },
];
