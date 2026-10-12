<script lang="ts">
  import { Search } from '../lib/icons';

  /**
   * The one search box, wherever a screen filters by typing.
   *
   * `.search-field` never grows: a box that took the row's spare width would give it back to
   * a "clear filters" button revealed beside it, and the row would shift on the first keystroke.
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
     * Milliseconds of quiet before the bound value follows the field, for a screen that fetches
     * on every change. Zero binds on every keystroke, for a screen that submits instead.
     */
    debounce?: number;
  } = $props();

  // The field's own text. `value` follows it, and a value the parent resets comes back into it.
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
