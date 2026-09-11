<script lang="ts">
  /**
   * Mounts the queue page inside an org context, for `page.render.test.ts`.
   *
   * The page reads its org through `useOrg()`, which throws outside the org
   * layout on purpose — so a test that mounts the page directly is testing a
   * routing mistake. This is the smallest thing that is not one. Mirrors
   * `../OrgPageHarness.svelte`.
   */
  import { untrack } from 'svelte';
  import { page } from '$app/state';
  import { OrgContext, provideOrg } from '$lib/org.svelte';
  import Page from './+page.svelte';

  let { slug, url }: { slug: string; url?: URL } = $props();

  provideOrg(new OrgContext(() => slug));

  const initialUrl = untrack(() => url);
  let current = $state(initialUrl);

  /** Lets a test drive a live filter/org change on an already-mounted instance. */
  export function setUrl(next: URL) {
    current = next;
  }

  if (initialUrl !== undefined) {
    Object.defineProperty(page, 'url', {
      configurable: true,
      get: () => current
    });
  }
</script>

<Page />
