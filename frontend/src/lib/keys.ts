/**
 * Whether a key belongs to an input method composing text, as the Enter that confirms a Japanese
 * conversion does. Safari sends that Enter after `compositionend`, marked only by `keyCode` 229.
 */
export const composing = (event: KeyboardEvent): boolean =>
  event.isComposing || event.keyCode === 229;
