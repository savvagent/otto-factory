<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';

  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { session } from '$lib/session.svelte';
  import * as webauthn from '$lib/webauthn';
  import Alert from '$lib/components/Alert.svelte';
  import Button from '$lib/components/Button.svelte';

  /**
   * Sign in.
   *
   * **There is no field on this page**, and that is the whole design. The
   * passkey is discoverable, so the browser resolves which account is signing
   * in and nothing is submitted beforehand. A login form that took an address
   * would be an oracle for which of an enterprise's employees hold accounts —
   * a target list for the phishing campaign that comes next — and every
   * previous version of this page needed careful, fragile machinery to avoid
   * being one.
   */

  const next = $derived(page.url.searchParams.get('next'));

  let pending = $state(false);
  let error = $state<string | undefined>(undefined);
  let supported = $state(true);

  $effect(() => {
    supported = webauthn.isSupported();
  });

  async function signIn() {
    pending = true;
    error = undefined;
    try {
      const started = await api.loginStart();
      const credential = await webauthn.authenticate(started.challenge as never);
      await api.loginFinish(started.ceremonyId, credential);
      await session.refresh();
      await goto(next ?? '/', { replaceState: true });
    } catch (e) {
      error = messageFor(e, m.error_could_not_sign_in());
    } finally {
      pending = false;
    }
  }
</script>

<svelte:head><title>{m.login_page_title()}</title></svelte:head>

<div class="mx-auto max-w-sm py-8">
  <h1 class="text-lg font-semibold">{m.login_heading()}</h1>
  <p class="mt-1 text-sm text-faint">
    {#if next}
      {m.login_continue_hint()}
    {:else}
      {m.login_passkey_hint()}
    {/if}
  </p>

  {#if !supported}
    <div class="mt-4">
      <Alert>{m.login_unsupported()}</Alert>
    </div>
  {:else}
    {#if error}<div class="mt-4"><Alert>{error}</Alert></div>{/if}

    <div class="mt-5">
      <Button {pending} onclick={signIn}>{m.login_submit()}</Button>
    </div>
  {/if}

  <div class="mt-6 space-y-2 border-t border-edge/50 pt-4 text-xs text-faint">
    <!--
      Two complete sentences, and the link is one of them. A sentence split
      around an anchor is a sentence no translator can reorder — and word order
      is exactly what differs between these six languages.
    -->
    <p>
      {m.login_recovery_hint()}
      <a class="text-muted underline hover:text-ink" href="/claim">{m.login_recovery_link()}</a>
    </p>
    <p class="pt-2">
      {m.login_no_account()}
      <a class="text-muted underline hover:text-ink" href="/signup">{m.login_create_account()}</a>
    </p>
  </div>
</div>
