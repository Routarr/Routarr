/**
 * Drop the focus to `<body>`, as a browser does once the focused control turns
 * disabled. jsdom leaves it on the disabled control and ignores its `blur()`,
 * so a test of where the focus goes next would start from a place no reader
 * is in. jsdom does drop it when the focused element leaves the page.
 */
export function dropFocus(): void {
  const standIn = document.body.appendChild(document.createElement('input'));
  standIn.focus();
  standIn.remove();
}
