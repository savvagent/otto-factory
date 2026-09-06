<script lang="ts">
  import { goto } from '$app/navigation';
  import { m } from '$lib/paraglide/messages';
  import { session } from '$lib/session.svelte';
  import Empty from '$lib/components/Empty.svelte';
  import Loading from '$lib/components/Loading.svelte';

  /**
   * The org-less billing URL, which exists because something else already
   * points at it.
   *
   * `df-billing`'s quota error and `df-mcp`'s upgrade prompt are built as
   * `{public_url}/settings/billing` — an agent that runs out of bucket puts that
   * exact string in front of a human. It has no org segment because the meter
   * builds it from configuration, once, at startup. So this page resolves the
   * org from the session and forwards; it is a signpost for a URL the rest of
   * the system already prints.
   */

  $effect(() => {
    if (!session.ready || !session.signedIn) return;
    const home = session.homeOrg;
    if (home) void goto(`/o/${home}/usage`, { replaceState: true });
  });
</script>

<svelte:head><title>{m.billing_page_title()}</title></svelte:head>

{#if session.ready && session.signedIn && !session.homeOrg}
  <Empty title={m.billing_no_org_title()}>
    {m.billing_no_org_hint()}
    <a class="text-muted underline hover:text-ink" href="/orgs/new">{m.billing_create_one_link()}</a
    >.
  </Empty>
{:else}
  <Loading what={m.billing_loading()} />
{/if}
