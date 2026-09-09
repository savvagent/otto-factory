<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';

  import { api, ApiError } from '$lib/api';
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
    // Declared above the `try` rather than inside it so the `catch` can still
    // name the key that was offered. An `unknown_credential` is a credential
    // this deployment has no record of — the shape an admin's passkey reset
    // leaves behind — and the id has to survive the throw that reports it.
    let credential: { rawId: string } | undefined;
    try {
      const started = await api.loginStart();
      credential = await webauthn.authenticate(started.challenge as never);
      await api.loginFinish(started.ceremonyId, credential);
      await session.refresh();
      // Signing in once is what makes the label retroactive: a key registered
      // before this account had an address is filed in the vault under words
      // nobody chose, and no re-registration would replace them.
      if (session.me) await webauthn.signalAccount(session.me);
      await goto(next ?? '/', { replaceState: true });
    } catch (e) {
      // The message is set first. `signalUnknownCredential` cannot throw today,
      // but if it ever could, awaiting it before this line would leave someone
      // watching a spinner stop with nothing said at all.
      error = messageFor(e, m.error_could_not_sign_in());
      // The one signal that names no account, because this browser is not
      // signed into one. Somebody already locked out is offered the dead key
      // first — it is the oldest in the vault — and telling the vault to drop
      // it is the difference between a second attempt working and looping.
      //
      // Only from a *sign-in* failure. `unknown_credential` also comes back
      // from removing or renaming a key that is not yours, and this signal is
      // destructive: reaching it from a management error would evict a
      // perfectly good credential from the vault.
      if (credential && e instanceof ApiError && e.code === 'unknown_credential') {
        await webauthn.signalUnknownCredential(credential.rawId);
      }
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
