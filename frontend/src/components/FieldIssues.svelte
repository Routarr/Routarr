<script lang="ts">
  import type { ValidationIssue } from '../api/types';

  /** The issues the server raised about the control `field` names, said under it. */
  let { field, issues }: { field: string; issues: ValidationIssue[] } = $props();
</script>

{#if issues.length > 0}
  <div id="{field}-issue">
    {#each issues as issue, index (index)}
      <p class="field-error">{issue.message}</p>
    {/each}
  </div>
{/if}
