<script lang="ts">
  import CardTitle from '../components/CardTitle.svelte';

  /** A card title with plain text, as `CardTitle` takes its content as a snippet. */
  let { card, title, id, level }: { card: string; title: string; id?: string; level?: 2 | 3 } =
    $props();
</script>

<CardTitle {card} {id} {level}>{title}</CardTitle>
