import { askConfirmation } from './confirm.svelte';
import { guardLeaving } from './router.svelte';

/**
 * Holds work a screen has not saved against leaving it: a link, Back or
 * Forward asks `question` first, and a reload or a closed tab gets the
 * browser's own question, the only one a page may ask there.
 *
 * Opens effects, so it is called while a component initialises.
 */
export function holdUnsaved(unsaved: () => boolean, question: () => string): void {
  $effect(() =>
    guardLeaving(async () => !unsaved() || askConfirmation(question(), 'DiscardChanges')),
  );
  $effect(() => {
    if (!unsaved()) return;
    const hold = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', hold);
    return () => window.removeEventListener('beforeunload', hold);
  });
}
