<script lang="ts">
  import { Search } from '../lib/icons';

  /**
   * One search box, wherever a screen filters by typing.
   *
   * Four screens had three treatments: the library wrapped its input with a
   * magnifier and an anchored width, the logs used a different icon size and
   * no anchor, and the history had neither icon nor anchor — so the same
   * gesture looked like a different control on each, and only one of them held
   * its width when a button appeared beside it.
   *
   * The anchor is the part worth stating once. As `flex: 1 1 auto` the box
   * absorbs every spare pixel of its row, so revealing "clear filters" takes
   * that width straight back out of it and shifts everything between them
   * sideways — on the first keystroke, and by however wide that label happens
   * to be in the language being read.
   */
  let {
    value = $bindable(),
    placeholder,
    label,
    oninput,
    debounce = 0,
  }: {
    value: string;
    placeholder: string;
    /** The accessible name, which the placeholder is not: it disappears on the first keystroke. */
    label: string;
    /** Fired after the value has changed — where a screen resets its paging. */
    oninput?: () => void;
    /**
     * Milliseconds of quiet before the bound value follows the field. A screen
     * whose `deps` read the value fetches on every change, and typing "anime"
     * was five `LIKE` queries against a homelab server; with a delay it is
     * one. Zero binds on every keystroke, for a screen that submits instead.
     */
    debounce?: number;
  } = $props();

  // The field's own text; `value` follows it, at once or after the quiet, and
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
