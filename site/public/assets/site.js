/* Routarr showcase, the only script on the page.
 *
 * Deferred and non-essential: the site is fully readable and navigable with
 * JavaScript disabled. The theme bootstrap that must run before first paint is
 * the one inline snippet in <head>, whose hash is pinned in `_headers`. */

(function () {
  'use strict';

  // ---------------------------------------------------------------- theme
  var root = document.documentElement;

  // Nothing stamped means dark, because that is the page's own default. Read
  // from the system instead, the first click on a light machine would ask for
  // the theme already on screen and do nothing visible.
  function current() {
    return root.dataset.theme === 'light' ? 'light' : 'dark';
  }

  // Which cell is lit is CSS's, off `data-theme`, so it is right on the first
  // paint. What is left here is the state a screen reader is told, which only
  // exists once these buttons can do anything at all.
  var themeButtons = document.querySelectorAll('[data-theme-set]');
  function markTheme() {
    Array.prototype.forEach.call(themeButtons, function (button) {
      button.setAttribute('aria-pressed', String(button.getAttribute('data-theme-set') === current()));
    });
  }
  markTheme();
  Array.prototype.forEach.call(themeButtons, function (button) {
    button.addEventListener('click', function () {
      root.dataset.theme = button.getAttribute('data-theme-set');
      markTheme();
      try {
        localStorage.setItem('routarr.theme', root.dataset.theme);
      } catch (e) {
        /* Private mode: the choice simply does not persist. */
      }
    });
  });

  // ---------------------------------------------------------------- index
  // The <details> already opens and closes on its own. These three are the
  // conveniences a pointer-and-keyboard reader expects from a menu, and the
  // same shortcut the application answers to.
  var index = document.querySelector('.index');
  if (index) {
    var summary = index.querySelector('summary');

    document.addEventListener('keydown', function (event) {
      if ((event.ctrlKey || event.metaKey) && (event.key === 'k' || event.key === 'K')) {
        event.preventDefault();
        index.open = !index.open;
        if (index.open) summary.focus();
        return;
      }
      if (event.key === 'Escape' && index.open) {
        index.open = false;
        summary.focus();
      }
    });

    document.addEventListener('click', function (event) {
      if (index.open && !index.contains(event.target)) index.open = false;
    });

    // A destination inside it scrolls this same page, so the panel would
    // otherwise stay open over what it just took you to.
    index.addEventListener('click', function (event) {
      if (event.target.closest && event.target.closest('.index-panel a')) index.open = false;
    });
  }

  // ---------------------------------------------------------------- copy
  // The text comes from the block itself rather than a duplicate in JS, so a
  // snippet can never drift from what the button hands you.
  Array.prototype.forEach.call(document.querySelectorAll('.copy-btn'), function (button) {
    button.addEventListener('click', function () {
      var terminal = button.closest('.terminal');
      var code = terminal && terminal.querySelector('code');
      if (!code) return;

      // The two answers travel on the button as data attributes, written by
      // the page in its own language: this file is one script for four pages.
      var done = function (ok) {
        var was = button.textContent;
        button.textContent = button.getAttribute(ok ? 'data-copied' : 'data-failed') || was;
        setTimeout(function () {
          button.textContent = was;
        }, 1600);
      };

      if (navigator.clipboard && window.isSecureContext) {
        navigator.clipboard.writeText(code.innerText).then(
          function () { done(true); },
          function () { done(false); }
        );
        return;
      }

      // http:// on a LAN is a plausible way to read this page; select the text
      // so the keyboard shortcut still works.
      var range = document.createRange();
      range.selectNodeContents(code);
      var selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      done(false);
    });
  });

  // ---------------------------------------------------------------- scroll-spy
  // On a page this long the nav should say where you are. Progressive
  // enhancement only: without JS the links still navigate, nothing is lost.
  if ('IntersectionObserver' in window) {
    var links = Array.prototype.slice.call(document.querySelectorAll('.index-panel a[href^="#"]'));
    var byId = {};
    links.forEach(function (link) {
      byId[link.getAttribute('href').slice(1)] = link;
    });

    var observer = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (entry) {
          if (!entry.isIntersecting) return;
          links.forEach(function (link) { link.removeAttribute('aria-current'); });
          var link = byId[entry.target.id];
          if (link) link.setAttribute('aria-current', 'true');
        });
      },
      // A narrow band near the top of the viewport: the section whose heading
      // is there is the one being read.
      { rootMargin: '-15% 0px -75% 0px' }
    );

    Object.keys(byId).forEach(function (id) {
      var section = document.getElementById(id);
      if (section) observer.observe(section);
    });
  }
})();

/* ------------------------------------------------------- language offer
   The site speaks four languages and the root serves English, so a reader
   whose browser asks for another one lands on the wrong page. This offers the
   right one rather than taking them there: a redirect would hijack a shared
   URL, flash the wrong language first, and do nothing at all for a visitor
   without JavaScript, who, with this, simply never sees the banner.

   The offer is written in the language it offers, and travels on the switcher
   link it points at, so no translation table is needed here. A choice, once
   made, is remembered and the banner never argues again. */
(function () {
  var STORE = 'routarr.lang';
  var hint = document.getElementById('lang-hint');
  var nav = document.querySelector('.lang-nav');
  if (!hint || !nav) return;

  function remember(code) {
    try { localStorage.setItem(STORE, code); } catch (e) { /* private mode */ }
  }
  function remembered() {
    try { return localStorage.getItem(STORE); } catch (e) { return null; }
  }

  // Clicking any language is a decision, and outranks the browser from then on.
  Array.prototype.forEach.call(nav.querySelectorAll('a[hreflang]'), function (a) {
    a.addEventListener('click', function () { remember(a.getAttribute('hreflang')); });
  });

  if (remembered()) return;

  var here = document.documentElement.lang;
  var offers = {};
  Array.prototype.forEach.call(nav.querySelectorAll('a[hreflang][data-offer]'), function (a) {
    offers[a.getAttribute('hreflang')] = a;
  });

  // `navigator.languages` is in preference order; the first one the site speaks
  // wins. A regional tag counts for its base language: fr-CA asks for French.
  var wanted = navigator.languages && navigator.languages.length
    ? navigator.languages
    : [navigator.language || ''];
  var match = null;
  for (var i = 0; i < wanted.length && !match; i++) {
    var base = String(wanted[i]).toLowerCase().split('-')[0];
    if (offers[base]) match = base;
  }
  if (!match || match === here) return;

  // Built here rather than sitting empty in the markup: an <a> with no text is
  // a control with no accessible name, which is exactly what site/verify.mjs
  // refuses, and it is right to.
  var source = offers[match];
  var wrap = document.createElement('div');
  wrap.className = 'wrap';

  var link = document.createElement('a');
  link.setAttribute('href', source.getAttribute('href'));
  link.setAttribute('hreflang', match);
  link.textContent = source.getAttribute('data-offer');
  link.addEventListener('click', function () { remember(match); });

  var close = document.createElement('button');
  close.setAttribute('type', 'button');
  close.setAttribute('aria-label', source.getAttribute('data-dismiss') || 'Dismiss');
  close.textContent = '\u00d7';
  close.addEventListener('click', function () {
    remember(here);
    hint.hidden = true;
  });

  wrap.appendChild(link);
  wrap.appendChild(close);
  hint.appendChild(wrap);
  hint.hidden = false;
})();
