<script lang="ts">
  import { t } from '../lib/i18n.svelte';

  /**
   * Placeholder rows while a table loads, holding its shape so the page does not jump. Hidden
   * from a screen reader, which is told the table is busy.
   */
  let { columns }: { columns: number } = $props();

  const ROWS = 6;
</script>

<tr>
  <td colspan={columns}>
    <span class="visually-hidden" role="status">{t('Loading')}</span>
  </td>
</tr>
{#each { length: ROWS } as _, row (row)}
  <tr aria-hidden="true">
    {#each { length: columns } as _, column (column)}
      <td>
        <div class="skeleton skeleton-line" style="width: {column === 0 ? '60%' : '40%'}"></div>
      </td>
    {/each}
  </tr>
{/each}
