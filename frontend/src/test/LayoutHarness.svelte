<script lang="ts">
  import Layout from '../components/Layout.svelte';

  /**
   * `Layout` takes its page as a snippet, and a test cannot hand a component a
   * snippet. This is the smallest thing that can give it one, a heading
   * included, as every screen has.
   */
</script>

<Layout>
  <h1 class="page-title">the screen</h1>
  <p>the page</p>
</Layout>
