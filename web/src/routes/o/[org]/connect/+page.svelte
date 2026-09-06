<script lang="ts">
  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { useOrg } from '$lib/org.svelte';
  import { CLIENTS } from '$lib/clients';
  import { relative } from '$lib/format';
  import type { MintedToken, ProtectedResourceMetadata, TokenSummary } from '$lib/types';
  import Alert from '$lib/components/Alert.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import CopyField from '$lib/components/CopyField.svelte';
  import Empty from '$lib/components/Empty.svelte';
  import Field from '$lib/components/Field.svelte';
  import Loading from '$lib/components/Loading.svelte';

  /**
   * Connect an agent.
   *
   * The MCP endpoint comes from `/.well-known/oauth-protected-resource`, not
   * from a constant in this bundle. The console is a static artifact that has to
   * work against whatever origin serves it, and a hard-coded URL is how a
   * staging or self-hosted deployment ends up printing a connect command that
   * points at production. The same document supplies the grantable scopes, so
   * the checkboxes below cannot drift from what the authorization server will
   * actually accept.
   *
   * OAuth is the path shown first; the token path is offered second and
   * explains itself. A PAT is not a weaker credential — same table, same
   * audience, same scopes, same per-request introspection — but it is a secret
   * a human has to hold, and that is a real cost worth stating before someone
   * mints one out of habit.
   *
   * Every snippet on this page is verbatim in every language. A translated
   * `--transport http`, config path, or scope name is a command nobody can run.
   */

  const org = useOrg();

  let metadata = $state<ProtectedResourceMetadata | undefined>(undefined);
  let tokens = $state<TokenSummary[]>([]);
  let loading = $state(true);
  let error = $state<string | undefined>(undefined);

  let client = $state(CLIENTS[0]!.id);
  let usingToken = $state(false);

  let name = $state('');
  let scopes = $state<string[]>(['jobs:read', 'jobs:write', 'repos:read', 'messages']);
  let ttlDays = $state(90);
  let minting = $state(false);
  let mintError = $state<string | undefined>(undefined);
  let minted = $state<MintedToken | undefined>(undefined);
  let busy = $state<string | undefined>(undefined);

  $effect(() => {
    const slug = org.slug;
    if (!slug) return;

    loading = true;
    error = undefined;

    void (async () => {
      try {
        const [m_, t] = await Promise.all([api.resourceMetadata(), api.tokens(slug)]);
        if (org.slug !== slug) return;
        metadata = m_;
        tokens = t;
      } catch (e) {
        error = messageFor(e, m.connect_error_metadata());
      } finally {
        loading = false;
      }
    })();
  });

  const mcpUrl = $derived(metadata?.resource ?? '');
  const grantable = $derived(metadata?.scopes_supported ?? []);
  const recipe = $derived(CLIENTS.find((c) => c.id === client) ?? CLIENTS[0]!);

  const snippet = $derived(
    !mcpUrl ? '' : usingToken ? recipe.token(mcpUrl, minted?.token ?? '') : recipe.oauth(mcpUrl)
  );

  function toggleScope(scope: string, on: boolean) {
    scopes = on ? [...new Set([...scopes, scope])] : scopes.filter((s) => s !== scope);
  }

  async function mint(event: SubmitEvent) {
    event.preventDefault();
    minting = true;
    mintError = undefined;
    try {
      minted = await api.mintToken(org.slug, name.trim(), scopes, ttlDays);
      usingToken = true;
      name = '';
      tokens = await api.tokens(org.slug);
    } catch (e) {
      mintError = messageFor(e, m.connect_error_mint());
    } finally {
      minting = false;
    }
  }

  async function revoke(id: string) {
    busy = id;
    error = undefined;
    try {
      await api.revokeToken(org.slug, id);
      tokens = await api.tokens(org.slug);
      if (minted?.id === id) minted = undefined;
    } catch (e) {
      error = messageFor(e, m.connect_error_revoke());
    } finally {
      busy = undefined;
    }
  }
</script>

<div class="space-y-5">
  <div>
    <h1 class="text-lg font-semibold">{m.connect_title()}</h1>
    <p class="mt-0.5 text-sm text-faint">
      {m.connect_subtitle()}
    </p>
  </div>

  {#if error}<Alert>{error}</Alert>{/if}

  {#if loading && !metadata}
    <Loading what={m.connect_loading()} />
  {:else if !mcpUrl}
    <Alert>
      {m.connect_no_endpoint({ document: '/.well-known/oauth-protected-resource' })}
    </Alert>
  {:else}
    <Card title={m.connect_endpoint_title()}>
      <CopyField value={mcpUrl} />
      <p class="mt-2 text-xs text-faint">
        {m.connect_endpoint_note({ org: org.slug })}
      </p>
    </Card>

    <Card title={m.connect_client_title()}>
      {#snippet actions()}
        <div class="flex rounded-md border border-edge text-xs">
          <button
            class="rounded-l-md px-2.5 py-1 transition"
            class:bg-raised={!usingToken}
            class:text-ink={!usingToken}
            class:text-muted={usingToken}
            onclick={() => (usingToken = false)}
          >
            OAuth
          </button>
          <button
            class="rounded-r-md px-2.5 py-1 transition"
            class:bg-raised={usingToken}
            class:text-ink={usingToken}
            class:text-muted={!usingToken}
            onclick={() => (usingToken = true)}
          >
            {m.connect_tab_token()}
          </button>
        </div>
      {/snippet}

      <div class="flex flex-wrap gap-1.5">
        {#each CLIENTS as option (option.id)}
          <button
            class="rounded-md border px-2.5 py-1 text-xs transition"
            class:border-accent={client === option.id}
            class:text-ink={client === option.id}
            class:border-edge={client !== option.id}
            class:text-muted={client !== option.id}
            onclick={() => (client = option.id)}
          >
            {option.label()}
          </button>
        {/each}
      </div>

      <div class="mt-4">
        {#if recipe.location}
          <p class="mb-1.5 text-xs text-faint">
            {m.connect_location({ path: recipe.location })}
          </p>
        {/if}
        <CopyField value={snippet} />
      </div>

      {#if recipe.note}
        <p class="mt-2 text-xs text-faint">{recipe.note()}</p>
      {/if}

      {#if usingToken && !minted}
        <p class="mt-2 text-xs text-warn">
          {m.connect_token_placeholder_note()}
        </p>
      {:else if !usingToken}
        <p class="mt-2 text-xs text-faint">
          {m.connect_oauth_note()}
        </p>
      {/if}
    </Card>

    {#if minted}
      <Card title={m.connect_new_token_title()}>
        <Alert tone="warn">
          {m.connect_new_token_warning()}
        </Alert>
        <div class="mt-3">
          <CopyField value={minted.token} />
        </div>
        <p class="mt-2 text-xs text-faint">
          {m.connect_new_token_scopes({ scopes: minted.scopes.join(' ') })}
        </p>
      </Card>
    {/if}

    <Card title={m.connect_pat_title()} description={m.connect_pat_description()}>
      <form class="space-y-4" onsubmit={mint}>
        <div class="grid gap-4 sm:grid-cols-3">
          <div class="sm:col-span-2">
            <Field label={m.connect_field_name_label()} hint={m.connect_field_name_hint()}>
              <input
                class="df-input"
                placeholder={m.connect_field_name_placeholder()}
                required
                bind:value={name}
              />
            </Field>
          </div>
          <Field label={m.connect_field_ttl_label()} hint={m.connect_field_ttl_hint()}>
            <input class="df-input" type="number" min="1" max="365" bind:value={ttlDays} />
          </Field>
        </div>

        <fieldset>
          <legend class="df-label">{m.connect_scopes_legend()}</legend>
          <div class="flex flex-wrap gap-x-5 gap-y-2">
            {#each grantable as scope (scope)}
              <label class="flex items-center gap-2 text-sm text-muted">
                <input
                  type="checkbox"
                  checked={scopes.includes(scope)}
                  onchange={(e) => toggleScope(scope, e.currentTarget.checked)}
                />
                <span class="df-mono">{scope}</span>
              </label>
            {/each}
          </div>
          <p class="mt-2 text-xs text-faint">
            {m.connect_scopes_note({ scope: 'org:admin' })}
          </p>
        </fieldset>

        {#if mintError}<Alert>{mintError}</Alert>{/if}

        <Button type="submit" pending={minting}>{m.connect_mint_button()}</Button>
      </form>

      <div class="mt-6 border-t border-edge/50 pt-4">
        <h3 class="text-xs font-medium tracking-wide text-muted uppercase">
          {m.connect_live_heading({ org: org.slug })}
        </h3>
        <p class="mt-1 text-xs text-faint">
          {m.connect_live_note()}
        </p>

        {#if tokens.length === 0}
          <div class="mt-3"><Empty title={m.connect_no_tokens()} /></div>
        {:else}
          <ul class="mt-3 divide-y divide-edge/40">
            {#each tokens as token (token.id)}
              <li class="flex flex-wrap items-center gap-3 py-2.5 text-sm">
                <div class="min-w-0 flex-1">
                  <span class="text-ink">
                    {token.name ?? token.clientId ?? m.connect_token_unnamed()}
                  </span>
                  <span class="ml-2 rounded-full border border-edge px-2 py-0.5 text-xs text-faint">
                    {token.kind}
                  </span>
                  <p class="df-mono mt-0.5 text-xs text-faint">
                    {token.scopes.join(' ') || m.connect_token_no_scopes()}
                  </p>
                  <p class="text-xs text-faint">
                    {m.connect_token_meta({
                      lastUsed: relative(token.lastUsedAt),
                      expires: relative(token.expiresAt)
                    })}
                  </p>
                </div>
                <Button tone="danger" pending={busy === token.id} onclick={() => revoke(token.id)}>
                  {m.connect_revoke()}
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </Card>
  {/if}
</div>
