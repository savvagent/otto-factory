<script lang="ts">
  import { page } from '$app/state';
  import type { Snippet } from 'svelte';

  import { m } from '$lib/paraglide/messages';
  import { signIn } from '$lib/login';
  import { OrgContext, provideOrg } from '$lib/org.svelte';
  import { session } from '$lib/session.svelte';
  import Loading from '$lib/components/Loading.svelte';

  let { children }: { children: Snippet } = $props();

  const slug = $derived(page.params.org ?? '');

  const context = new OrgContext(() => page.params.org ?? '');
  provideOrg(context);

  /** Signed in to this org (by slug or id), as opposed to some other one. */
  const matches = $derived.by(() => {
    const info = session.info;
    if (!info) return false;
    const wanted = slug.toLowerCase();
    return wanted === info.org.slug.toLowerCase() || wanted === info.org.id.toLowerCase();
  });

  /** Signing in again for this org was just tried and changed nothing. */
  let missing = $state(false);

  /**
   * Resolve the org named in the URL, from the session.
   *
   * **A console session is signed in to exactly one org** — the platform's token
   * is bound to it. So there is nothing to fetch: the org, the role, and the plan
   * come from `GET /api/session`, and a URL naming any other org means the
   * visitor has to sign in *for that org* (`/auth/login?org=…`, which the
   * platform answers with that org if they belong to it). The server enforces the
   * same thing from its side — an API call for another org is
   * `401 org_session_mismatch`, and `$lib/api` sends the tab through the same
   * sign-in.
   *
   * If sign-in comes straight back to the same mismatch, the visitor does not
   * belong to that org, and the page says it does not exist. **It does not say
   * "you don't have access"**: the server answers the same for an org that does
   * not exist, precisely so the two cannot be told apart.
   */
  $effect(() => {
    const info = session.info;
    if (!info) return;
    if (matches) {
      context.org = info.org;
      context.role = info.role;
      missing = false;
      return;
    }
    context.org = undefined;
    context.role = undefined;
    if (slug && !signIn(slug, page.url.pathname + page.url.search)) missing = true;
  });

  const nav = $derived([
    { href: `/o/${slug}`, label: m.orgnav_overview(), exact: true },
    { href: `/o/${slug}/queue`, label: m.orgnav_queue() },
    { href: `/o/${slug}/repos`, label: m.orgnav_repos() },
    ...(context.isAdmin ? [{ href: `/o/${slug}/trackers`, label: m.orgnav_trackers() }] : []),
    ...(context.isAdmin ? [{ href: `/o/${slug}/audit`, label: m.orgnav_audit() }] : [])
  ]);

  function active(href: string, exact = false): boolean {
    return exact ? page.url.pathname === href : page.url.pathname.startsWith(href);
  }
</script>

<svelte:head><title>{m.orgnav_document_title({ title: context.title })}</title></svelte:head>

{#if missing}
  <div class="mx-auto max-w-md py-10 text-center">
    <h1 class="text-lg font-semibold">{m.orgnav_missing_title()}</h1>
    <!--
      One message with the slug inside it, rather than prose either side of a
      `<code>`. The slug sits mid-sentence in English and does not in every
      language, and a sentence assembled from two fragments cannot be moved.
    -->
    <p class="mt-2 text-sm text-faint">{m.orgnav_missing_body({ slug })}</p>
  </div>
{:else}
  <div class="flex flex-col gap-6 sm:flex-row">
    <nav
      class="shrink-0 sm:w-auto sm:min-w-44"
      aria-label={m.orgnav_sections_aria({ org: context.title })}
    >
      <ul class="flex gap-1 overflow-x-auto sm:flex-col sm:overflow-visible">
        {#each nav as item (item.href)}
          <li>
            <a
              href={item.href}
              class="block rounded-md px-2.5 py-1.5 text-sm whitespace-nowrap text-muted transition hover:bg-raised hover:text-ink"
              class:bg-raised={active(item.href, item.exact)}
              class:text-ink={active(item.href, item.exact)}
              aria-current={active(item.href, item.exact) ? 'page' : undefined}
            >
              {item.label}
            </a>
          </li>
        {/each}
      </ul>
    </nav>

    <div class="min-w-0 flex-1">
      {#if !context.org}
        <Loading what={m.orgnav_loading({ org: slug })} />
      {:else}
        {@render children()}
      {/if}
    </div>
  </div>
{/if}
