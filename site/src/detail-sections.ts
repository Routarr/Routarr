import type { Key } from './i18n';

/**
 * The detail page's anchors, numbered in the page's own order, for the Index panel and the
 * landing's last card. `#how` is a block inside `#why` and is listed all the same.
 */
export const DETAIL_SECTIONS: { number: string; href: string; label: Key; note: Key }[] = [
  { number: '01', href: '#why', label: 'why.span', note: 'header.desc.7' },
  { number: '02', href: '#how', label: 'header.li', note: 'header.desc' },
  { number: '03', href: '#explain', label: 'explain.span', note: 'header.desc.8' },
  { number: '04', href: '#folders', label: 'folders.span', note: 'header.desc.9' },
  { number: '05', href: '#features', label: 'header.li.3', note: 'header.desc.3' },
  { number: '06', href: '#faq', label: 'header.li.6', note: 'header.desc.6' },
];
