<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import type { Snippet } from 'svelte';

  import '../app.css';
  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { resolveAtBoot } from '$lib/locale';
  import { orgOf, signIn } from '$lib/login';
  import { platformLink, type PlatformPage } from '$lib/platform';
  import { session } from '$lib/session.svelte';
  import { APP_VERSION } from '$lib/version';
  import Alert from '$lib/components/Alert.svelte';
  import Loading from '$lib/components/Loading.svelte';
  import Logo from '$lib/components/Logo.svelte';

  /**
   * Decide the language before anything renders.
   *
   * Top-level in this script rather than in `+layout.ts`, because `ssr = false`
   * makes this file browser-only while a universal load module is also
   * evaluated by the static build — where `document` and `localStorage` do not
   * exist. It runs once, synchronously, ahead of the first paint, so no page
   * ever renders English and then snaps to Spanish.
   */
  resolveAtBoot();

  let { children }: { children: Snippet } = $props();

  /** A failure resolving the session that is *not* "signed out". */
  let fatal = $state<string | undefined>(undefined);
  let signingOut = $state(false);
  /** Sign-in was just tried and the session still did not appear. */
  let signInStuck = $state(false);

  /**
   * Pages that skip the sign-in guard.
   *
   * `/docs/api` mirrors an `Auth::Public` server endpoint that has nothing to do
   * with session state, so a visitor following the footer link lands on it
   * whether or not they are signed in.
   *
   * `/` is the front page: a signed-out visitor has to be able to read it (and
   * see a "Sign in" button, and a `login_error` if a sign-in just failed). A
   * signed-in visitor is moved along by `+page.svelte`'s own effect.
   */
  const UNGATED = ['/docs/api', '/'];

  const isUngated = $derived(UNGATED.some((p) => page.url.pathname === p));

  $effect(() => {
    void resolve();
  });

  async function resolve() {
    if (session.ready) return;
    try {
      await session.refresh();
    } catch (error) {
      fatal = messageFor(error, m.error_session_resolve_failed());
    }
  }

  /**
   * The sign-in guard, in one place.
   *
   * Written as an effect over `session.ready` and the current path rather than
   * as a check in each page: a page that forgets is a page that renders a
   * skeleton to a signed-out visitor and then flashes it away.
   *
   * Sign-in is **a full navigation** to `/auth/login` — a server route that
   * answers a redirect to the otto platform — and it brings the visitor back to
   * this same URL afterwards.
   *
   * `signingOut` suppresses the guard for the moment between clearing the
   * session and arriving at `/`. Without it the guard fires first, from whatever
   * org page the button was pressed on, and sends someone who deliberately signed
   * out straight back into sign-in.
   */
  $effect(() => {
    if (isUngated || !session.ready || fatal || signingOut || session.signedIn) return;
    const { pathname, search } = page.url;
    if (!signIn(orgOf(pathname), pathname + search)) signInStuck = true;
  });

  async function signOut() {
    signingOut = true;
    try {
      await api.logout();
    } catch {
      // Cleared locally regardless. The server treats an unknown cookie as
      // already gone, so the only way to reach here with a live session is a
      // network error — and leaving the console looking signed in after someone
      // pressed "sign out" is the worse of the two wrong answers.
    } finally {
      session.clear();
      await goto('/', { replaceState: true });
      signingOut = false;
    }
  }

  /**
   * What the platform's console manages, in the order a person looks for it.
   * Identity is the platform's: this console links there rather than copying it.
   */
  const platformPages: { page: PlatformPage; label: () => string }[] = [
    { page: 'members', label: () => m.nav_platform_members() },
    { page: 'teams', label: () => m.nav_platform_teams() },
    { page: 'sso', label: () => m.nav_platform_sso() },
    { page: 'usage', label: () => m.nav_platform_usage() },
    { page: 'account', label: () => m.nav_platform_account() },
    { page: 'orgs', label: () => m.nav_platform_switch_org() }
  ];
</script>

<div class="flex min-h-full flex-col">
  <header class="border-b border-edge/60 bg-surface/40">
    <div class="mx-auto flex w-full max-w-6xl items-center gap-4 px-4 py-3">
      <a href="/" class="flex items-center gap-2" aria-label="otto-factory">
        <Logo class="size-6 text-accent" />
      </a>

      {#if session.info}
        <nav class="ml-2 flex gap-1 text-sm" aria-label={m.nav_organizations()}>
          <a
            href="/o/{session.info.org.slug}"
            class="rounded-md bg-raised px-2.5 py-1 text-ink transition"
          >
            {session.info.org.name}
          </a>
        </nav>

        <!--
          Everything identity-shaped lives at the platform. A native <details>
          rather than a hand-rolled menu: it is keyboard- and screen-reader-
          operable for free, and needs no script to stay in sync.
        -->
        <details class="relative text-sm">
          <summary
            class="cursor-pointer list-none rounded-md px-2.5 py-1 text-muted transition hover:bg-raised hover:text-ink"
          >
            {m.nav_manage()}
          </summary>
          <ul
            class="absolute left-0 z-10 mt-1 w-56 rounded-md border border-edge bg-surface p-1 shadow-lg"
          >
            {#each platformPages as item (item.page)}
              <li>
                <a
                  href={platformLink(session.info.platformUrl, item.page, session.info.org.slug)}
                  class="block rounded px-2.5 py-1.5 text-muted transition hover:bg-raised hover:text-ink"
                >
                  {item.label()}
                </a>
              </li>
            {/each}
          </ul>
        </details>
      {/if}

      <div class="ml-auto flex items-center gap-3 text-sm">
        {#if session.info}
          <span class="hidden text-faint sm:inline"
            >{session.info.user.email ?? session.info.user.name ?? ''}</span
          >
          <button
            class="rounded-md border border-edge px-2.5 py-1 text-muted transition hover:bg-raised hover:text-ink disabled:opacity-50"
            onclick={signOut}
            disabled={signingOut}
          >
            {m.nav_sign_out()}
          </button>
        {:else if session.ready}
          <a
            href="/auth/login"
            data-sveltekit-reload
            class="rounded-md border border-edge px-2.5 py-1 text-muted transition hover:bg-raised hover:text-ink"
          >
            {m.nav_sign_in()}
          </a>
        {/if}
      </div>
    </div>
  </header>

  <main class="mx-auto w-full max-w-6xl flex-1 px-4 py-6">
    {#if isUngated}
      {@render children()}
    {:else if fatal}
      <Alert>
        {fatal}
        <button class="ml-2 underline" onclick={() => location.reload()}>{m.nav_try_again()}</button
        >
      </Alert>
    {:else if signInStuck}
      <Alert>
        {m.nav_sign_in_stuck()}
        <a class="ml-2 underline" href="/auth/login" data-sveltekit-reload>{m.nav_sign_in()}</a>
      </Alert>
    {:else if !session.ready || !session.signedIn}
      <Loading what={m.nav_checking_session()} />
    {:else}
      {@render children()}
    {/if}
  </main>

  <footer class="border-t border-edge/40 px-4 py-4 text-center text-xs text-faint">
    <a class="hover:text-muted" href="/docs/api">{m.nav_api_reference()}</a>
    <span aria-hidden="true"> · </span>
    <span>v{APP_VERSION}</span>
  </footer>
</div>
