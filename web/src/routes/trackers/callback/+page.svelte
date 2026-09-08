<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';

  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { completeConnect } from '$lib/trackerState';
  import Alert from '$lib/components/Alert.svelte';
  import Loading from '$lib/components/Loading.svelte';

  /**
   * Where GitHub and Atlassian send the browser back.
   *
   * **Not under `/o/[org]/`, and it cannot be.** A redirect URI is one static
   * string registered with the provider; it cannot carry an org slug that
   * varies per customer. The org rides in the OAuth `state` instead, alongside
   * a nonce this browser minted before it left — see `$lib/trackerState`.
   *
   * **The page renders; the button-equivalent POSTs.** The authorization code
   * arrives in this URL, and everything that spends a single-use credential in
   * this product is a POST, so that a preview fetcher following the link burns
   * nothing. Here the POST is issued by script on arrival rather than by a
   * click, which is the same trade the rest of the console makes: a GET
   * rendered this page, and only a script the page ran can spend the code.
   */

  let error = $state<string | undefined>(undefined);
  let done = $state(false);

  $effect(() => {
    const params = page.url.searchParams;
    if (done) return;
    done = true;

    // A provider can refuse before we ever see a code — an admin who cancelled
    // on the consent screen lands here with `error` and nothing else.
    const denied = params.get('error_description') ?? params.get('error');
    if (denied) {
      error = m.trackercb_provider_refused({ reason: denied });
      return;
    }

    const pending = completeConnect(params.get('state'));
    if (!pending) {
      error = m.trackercb_no_pending_state();
      return;
    }

    const code = params.get('code');
    if (!code) {
      error = pending.provider === 'github' ? m.trackercb_github_no_code() : m.trackercb_no_code();
      return;
    }

    // GitHub sends the installation alongside the code; JIRA has no equivalent
    // and the site is read from Atlassian on the server side.
    const rawInstallation = params.get('installation_id');
    const installationId = rawInstallation ? Number(rawInstallation) : undefined;

    void (async () => {
      try {
        await api.connectTracker(pending.org, pending.provider, {
          code,
          ...(Number.isSafeInteger(installationId) ? { installationId } : {})
        });
        await goto(`/o/${pending.org}/trackers`, { replaceState: true });
      } catch (e) {
        error = messageFor(e, m.trackercb_error_fallback());
      }
    })();
  });
</script>

<div class="mx-auto max-w-md py-10">
  {#if error}
    <h1 class="text-lg font-semibold">{m.trackercb_failed_heading()}</h1>
    <div class="mt-3"><Alert>{error}</Alert></div>
    <p class="mt-4 text-sm text-faint">
      <a class="underline hover:text-ink" href="/orgs">{m.trackercb_back_link()}</a>
    </p>
  {:else}
    <Loading what={m.trackercb_loading()} />
  {/if}
</div>
