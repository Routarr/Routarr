<script lang="ts">
  import { Copy } from '../lib/icons';
  import { t } from '../lib/i18n.svelte';
  import { onMac } from '../lib/keys';

  /**
   * A value to paste elsewhere, shown whole and wrapped, beside the button that copies it. The
   * clipboard is closed on a plain http origin, so there the press selects it for the keyboard.
   */
  let { value, label }: { value: string; label: string } = $props();

  let shown = $state<HTMLElement | null>(null);
  let said = $state('');
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(timer));

  async function copy() {
    clearTimeout(timer);
    try {
      await navigator.clipboard.writeText(value);
      said = t('Copied');
      timer = setTimeout(() => (said = ''), 5000);
    } catch {
      const range = document.createRange();
      range.selectNodeContents(shown!);
      window.getSelection()?.removeAllRanges();
      window.getSelection()?.addRange(range);
      said = t('CopyByHand', { key: onMac() ? '⌘C' : 'Ctrl+C' });
    }
  }
</script>

<div class="copy-field">
  <code class="mono copy-value" bind:this={shown}>{value}</code>
  <button
    type="button"
    class="btn btn-secondary btn-sm"
    aria-label="{t('Copy')} – {label}"
    onclick={() => void copy()}
  >
    <Copy size={14} aria-hidden="true" />
    {t('Copy')}
  </button>
</div>
<!-- Mounted empty: a live region that arrives with its text announces nothing. -->
<p class="text-muted text-sm mt-1" role="status">{said}</p>
