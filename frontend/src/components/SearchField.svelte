<script lang="ts">
  import { Search } from '../lib/icons';

  /**
   * One search box, wherever a screen filters by typing, so the same gesture
   * never looks like a different control, and every box holds its width when a
   * button appears beside it.
   *
   * The anchor is the part worth stating once. As `flex: 1 1 auto` the box
   * absorbs every spare pixel of its row, so revealing "clear filters" takes
   * that width straight back out of it and shifts everything between them
   * sideways, on the first keystroke, and by however wide that label happens
   * to be in the language being read.
   */
  let {
    value = $bindable(),
    id,
    placeholder,
    label,
    oninput,
    debounce = 0,
  }: {
    value: string;
    /** For a dialog that opens on this field. */
    id?: string;
    placeholder: string;
    /** The accessible name, which the placeholder is not: it disappears on the first keystroke. */
    label: string;
    /** Fired after the value has changed, where a screen resets its paging. */
    oninput?: () => void;
    /**
     * Milliseconds of quiet before the bound value follows the field. A screen
     * whose `deps` read the value fetches on every change, so without a delay
     * typing "anime" is five `LIKE` queries against a homelab server, and with
     * one it is a single query. Zero binds on every keystroke, for a screen
     * that submits instead.
     */
    debounce?: number;
  } = $props();

  // The field's own text. `value` follows it, at once or after the quiet, and
  // a value the parent resets (clear filters) comes back down into it.
  let draft = $derived(value);

  let timer: ReturnType<typeof setTimeout> | undefined;

  function commit() {
    clearTimeout(timer);
    timer = undefined;
    if (draft === value) return;
    value = draft;
    oninput?.();
  }

  function typed(event: Event & { currentTarget: HTMLInputElement }) {
    draft = event.currentTarget.value;
    if (debounce <= 0) {
      commit();
      return;
    }
    clearTimeout(timer);
    timer = setTimeout(commit, debounce);
  }

  $effect(() => () => clearTimeout(timer));
</script>

<div class="search-field">
  <Search size={16} class="text-muted" aria-hidden="true" />
  <input
    {id}
    class="form-input"
    type="search"
    {placeholder}
    aria-label={label}
    value={draft}
    oninput={typed}
    onkeydown={(event) => {
      if (event.key === 'Enter') commit();
    }}
    onblur={commit}
  />
</div>
