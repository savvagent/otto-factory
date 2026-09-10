<script lang="ts">
  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { LINK, around } from '$lib/labels';
  import { useOrg } from '$lib/org.svelte';
  import { relative, slugPreview } from '$lib/format';
  import type { Lease, Repo, Team, TrackerBinding, TrackerProvider } from '$lib/types';
  import Alert from '$lib/components/Alert.svelte';
  import Button from '$lib/components/Button.svelte';
  import Card from '$lib/components/Card.svelte';
  import Empty from '$lib/components/Empty.svelte';
  import Field from '$lib/components/Field.svelte';
  import Loading from '$lib/components/Loading.svelte';

  /**
   * Repos, and who is holding a lease on one right now.
   *
   * The leases are the reason this page is worth opening. A lease is advisory —
   * the server cannot see a git operation and cannot stop one — so its whole
   * value is being *visible*: "why is my agent waiting?" is answered by a name
   * and a branch, not by a lock.
   *
   * Leases are loaded per repo and only when a row is expanded. Fetching every
   * repo's leases up front would be one request per repo on every page load, to
   * render something nobody had asked to see. The tracker bindings in the same
   * expander are loaded the same way and for the same reason.
   *
   * A binding lives here rather than on a page of its own because it is a fact
   * about one repo: which project its tickets come from, and what label on an
   * issue there means "queue this". The connection those bindings hang off is
   * per-org and lives on the Trackers page.
   */

  const org = useOrg();

  let repos = $state<Repo[]>([]);
  let teams = $state<Team[]>([]);
  let includeInactive = $state(false);
  let loading = $state(true);
  let error = $state<string | undefined>(undefined);

  let expanded = $state<string | undefined>(undefined);
  let leases = $state<Record<string, Lease[] | 'loading' | 'failed'>>({});
  let bindings = $state<Record<string, TrackerBinding[] | 'loading' | 'failed'>>({});

  // Keyed by `${repo}:${provider}` so two providers on one repo edit
  // independently and a third repo's draft never lands in this one's field.
  let refDraft = $state<Record<string, string>>({});
  let labelDraft = $state<Record<string, string>>({});
  let bindingBusy = $state<string | undefined>(undefined);
  let bindingError = $state<Record<string, string | undefined>>({});

  // `owner/repo` is git's own spelling rather than prose — a translated one is
  // a placeholder nobody can act on. The JIRA hint names a concept, and is.
  const PROVIDERS: { provider: TrackerProvider; name: string; hint: string }[] = [
    { provider: 'github', name: 'GitHub', hint: 'owner/repo' },
    { provider: 'jira', name: 'JIRA', hint: m.repos_binding_hint_jira() }
  ];

  let showForm = $state(false);
  let slug = $state('');
  let name = $state('');
  let remotes = $state('');
  let teamId = $state('');
  let creating = $state(false);
  let formError = $state<string | undefined>(undefined);

  // Not $state: it identifies a request and is never rendered.
  let latest = 0;

  $effect(() => {
    const org_ = org.slug;
    const withInactive = includeInactive;
    if (!org_) return;

    // Sequence rather than an org comparison, because the include-inactive
    // toggle is a dependency too: flipped twice quickly, the first response can
    // land after the second and show the list the toggle no longer asks for.
    const seq = ++latest;

    loading = true;
    error = undefined;

    void (async () => {
      try {
        const [r, t] = await Promise.all([api.repos(org_, withInactive), api.teams(org_)]);
        if (seq !== latest) return;
        repos = r;
        teams = t;
      } catch (e) {
        if (seq !== latest) return;
        error = messageFor(e, m.repos_error_load());
      } finally {
        if (seq === latest) loading = false;
      }
    })();
  });

  async function toggle(repo: Repo) {
    if (expanded === repo.slug) {
      expanded = undefined;
      return;
    }
    expanded = repo.slug;
    void loadLeases(repo.slug);
    void loadBindings(repo.slug);
  }

  async function loadLeases(slug_: string) {
    if (leases[slug_] && leases[slug_] !== 'failed') return;
    leases = { ...leases, [slug_]: 'loading' };
    try {
      const found = await api.leases(org.slug, slug_);
      leases = { ...leases, [slug_]: found };
    } catch {
      leases = { ...leases, [slug_]: 'failed' };
    }
  }

  async function loadBindings(slug_: string) {
    if (bindings[slug_] && bindings[slug_] !== 'failed') return;
    bindings = { ...bindings, [slug_]: 'loading' };
    try {
      const found = await api.trackerBindings(org.slug, slug_);
      bindings = { ...bindings, [slug_]: found };
      // Seed the drafts from what is stored, so the field an admin edits shows
      // what is live rather than an empty box beside a bound repo.
      for (const binding of found) {
        const key = `${slug_}:${binding.provider}`;
        refDraft[key] = binding.externalRef;
        labelDraft[key] = binding.triggerLabel;
      }
    } catch {
      bindings = { ...bindings, [slug_]: 'failed' };
    }
  }

  function bindingFor(slug_: string, provider: TrackerProvider): TrackerBinding | undefined {
    const found = bindings[slug_];
    return Array.isArray(found) ? found.find((b) => b.provider === provider) : undefined;
  }

  /**
   * Re-read after a write rather than patching the local array.
   *
   * The server decides two of the fields shown here — the default trigger
   * label, and whether the binding is `live` — so a locally patched row would
   * show what was typed instead of what was stored.
   */
  async function reloadBindings(slug_: string) {
    bindings = { ...bindings, [slug_]: 'failed' };
    await loadBindings(slug_);
  }

  async function saveBinding(slug_: string, provider: TrackerProvider) {
    const key = `${slug_}:${provider}`;
    bindingBusy = key;
    bindingError = { ...bindingError, [key]: undefined };
    try {
      const label = (labelDraft[key] ?? '').trim();
      await api.bindRepo(org.slug, slug_, provider, {
        externalRef: (refDraft[key] ?? '').trim(),
        ...(label ? { triggerLabel: label } : {})
      });
      await reloadBindings(slug_);
    } catch (e) {
      bindingError = {
        ...bindingError,
        [key]: messageFor(e, m.repos_error_save_binding())
      };
    } finally {
      bindingBusy = undefined;
    }
  }

  async function removeBinding(slug_: string, provider: TrackerProvider) {
    const key = `${slug_}:${provider}`;
    bindingBusy = key;
    bindingError = { ...bindingError, [key]: undefined };
    try {
      await api.unbindRepo(org.slug, slug_, provider);
      refDraft[key] = '';
      labelDraft[key] = '';
      await reloadBindings(slug_);
    } catch (e) {
      bindingError = {
        ...bindingError,
        [key]: messageFor(e, m.repos_error_remove_binding())
      };
    } finally {
      bindingBusy = undefined;
    }
  }

  async function register(event: SubmitEvent) {
    event.preventDefault();
    creating = true;
    formError = undefined;
    try {
      // One remote per line. The server normalizes them, so the SSH and HTTPS
      // spellings of one repository collapse to a single row and either
      // resolves to it — which is why pasting all of them is the right advice.
      const parsed = remotes
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line.length > 0);

      await api.registerRepo(org.slug, {
        slug: slugPreview(slug),
        name: name.trim() || null,
        remotes: parsed,
        teamId: teamId || null
      });

      slug = '';
      name = '';
      remotes = '';
      teamId = '';
      showForm = false;
      repos = await api.repos(org.slug, includeInactive);
    } catch (e) {
      formError = messageFor(e, m.repos_error_register());
    } finally {
      creating = false;
    }
  }

  async function setActive(repo: Repo, active: boolean) {
    try {
      await api.updateRepo(org.slug, repo.slug, { active });
      repos = await api.repos(org.slug, includeInactive);
    } catch (e) {
      error = messageFor(e, m.repos_error_update());
    }
  }

  const teamName = $derived((id: string | null) =>
    id ? (teams.find((t) => t.id === id)?.slug ?? m.repos_team_unknown()) : m.repos_team_org_wide()
  );
</script>

<div class="space-y-5">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div>
      <h1 class="text-lg font-semibold">{m.repos_title()}</h1>
      <p class="mt-0.5 text-sm text-faint">
        {m.repos_subtitle()}
      </p>
    </div>
    {#if org.isAdmin}
      <Button onclick={() => (showForm = !showForm)}>
        {showForm ? m.repos_cancel() : m.repos_register_button()}
      </Button>
    {/if}
  </div>

  {#if showForm}
    <Card title={m.repos_form_title()}>
      <form class="space-y-4" onsubmit={register}>
        <div class="grid gap-4 sm:grid-cols-2">
          <Field label={m.repos_field_slug_label()} hint={m.repos_field_slug_hint()}>
            <input class="of-input of-mono" required bind:value={slug} />
          </Field>
          <Field label={m.repos_field_name_label()} hint={m.repos_field_name_hint()}>
            <input class="of-input" bind:value={name} />
          </Field>
        </div>

        <Field label={m.repos_field_remotes_label()} hint={m.repos_field_remotes_hint()}>
          <textarea class="of-input of-mono h-24" bind:value={remotes}></textarea>
        </Field>

        {#if teams.length > 0}
          <Field label={m.repos_field_team_label()} hint={m.repos_field_team_hint()}>
            <select class="of-input" bind:value={teamId}>
              <option value="">{m.repos_option_org_wide()}</option>
              {#each teams as team (team.id)}
                <option value={team.id}>{team.slug}</option>
              {/each}
            </select>
          </Field>
        {/if}

        {#if formError}<Alert>{formError}</Alert>{/if}

        <Button type="submit" pending={creating}>{m.repos_submit()}</Button>
      </form>
    </Card>
  {/if}

  <label class="flex items-center gap-2 text-xs text-muted">
    <input type="checkbox" bind:checked={includeInactive} />
    {m.repos_include_retired()}
  </label>

  {#if error}
    <Alert>{error}</Alert>
  {:else if loading && repos.length === 0}
    <Loading what={m.repos_loading()} />
  {:else if repos.length === 0}
    <Empty title={m.repos_empty_title()}>
      {#if org.isAdmin}
        {m.repos_empty_admin({ tool: 'register_repo' })}
      {:else}
        {m.repos_empty_member()}
      {/if}
    </Empty>
  {:else}
    <ul class="space-y-2">
      {#each repos as repo (repo.id)}
        <li class="of-card">
          <div class="flex flex-wrap items-center gap-3 px-4 py-3">
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="of-mono text-sm text-ink">{repo.slug}</span>
                {#if !repo.active}
                  <span class="rounded-full border border-edge px-2 py-0.5 text-xs text-faint">
                    {m.repos_badge_retired()}
                  </span>
                {/if}
              </div>
              <p class="mt-0.5 text-xs text-faint">
                {repo.name} · {repo.provider} · {repo.defaultBranch} · {teamName(repo.teamId)}
              </p>
            </div>

            <button
              class="text-xs text-muted underline hover:text-ink"
              onclick={() => toggle(repo)}
              aria-expanded={expanded === repo.slug}
            >
              {expanded === repo.slug ? m.repos_hide_leases() : m.repos_show_leases()}
            </button>

            <a
              class="text-xs text-muted underline hover:text-ink"
              href="/o/{org.slug}/queue?repo={encodeURIComponent(repo.slug)}"
            >
              {m.repos_queue_link()}
            </a>

            {#if org.isAdmin}
              <Button tone="quiet" onclick={() => setActive(repo, !repo.active)}>
                {repo.active ? m.repos_retire() : m.repos_reinstate()}
              </Button>
            {/if}
          </div>

          {#if expanded === repo.slug}
            <div class="border-t border-edge/60 px-4 py-3">
              {#if leases[repo.slug] === 'loading'}
                <Loading what={m.repos_leases_loading()} />
              {:else if leases[repo.slug] === 'failed'}
                <Alert>{m.repos_leases_failed()}</Alert>
              {:else if (leases[repo.slug] as Lease[]).length === 0}
                <p class="text-xs text-faint">
                  {m.repos_no_leases({ repo: repo.slug })}
                </p>
              {:else}
                <ul class="space-y-1.5">
                  {#each leases[repo.slug] as Lease[] as lease (lease.id)}
                    <li class="flex flex-wrap items-baseline gap-x-3 text-sm">
                      <span class="of-mono text-ink">{lease.resource}</span>
                      <span class="text-muted">
                        {lease.holderLabel ?? m.repos_lease_holder_unknown()}
                      </span>
                      {#if lease.jobId}
                        <a
                          class="of-mono text-xs text-muted underline hover:text-ink"
                          href="/o/{org.slug}/queue/{lease.jobId}"
                        >
                          {lease.jobId}
                        </a>
                      {/if}
                      <span class="text-xs text-faint">
                        {m.repos_lease_expires({ when: relative(lease.expiresAt) })}
                      </span>
                    </li>
                  {/each}
                </ul>
                <p class="mt-2 text-xs text-faint">
                  {m.repos_leases_note()}
                </p>
              {/if}
            </div>

            <div class="border-t border-edge/60 px-4 py-3">
              <h3 class="text-xs font-semibold tracking-wide text-muted uppercase">
                {m.repos_trackers_heading()}
              </h3>

              {#if bindings[repo.slug] === 'loading'}
                <Loading what={m.repos_bindings_loading()} />
              {:else if bindings[repo.slug] === 'failed'}
                <Alert>{m.repos_bindings_failed()}</Alert>
              {:else}
                <ul class="mt-2 space-y-3">
                  {#each PROVIDERS as { provider, name, hint } (provider)}
                    {@const binding = bindingFor(repo.slug, provider)}
                    {@const key = `${repo.slug}:${provider}`}
                    <li>
                      <div class="flex flex-wrap items-end gap-2">
                        <span class="w-14 shrink-0 text-sm text-ink">{name}</span>

                        {#if org.isAdmin}
                          <label class="min-w-0 flex-1">
                            <span class="sr-only">
                              {m.repos_binding_ref_label({ provider: name })}
                            </span>
                            <input
                              class="of-input of-mono"
                              placeholder={hint}
                              bind:value={refDraft[key]}
                            />
                          </label>
                          <label class="w-32 shrink-0">
                            <span class="sr-only">
                              {m.repos_binding_trigger_label({ provider: name })}
                            </span>
                            <input
                              class="of-input of-mono"
                              placeholder="otto-factory"
                              bind:value={labelDraft[key]}
                            />
                          </label>
                          <Button
                            pending={bindingBusy === key}
                            onclick={() => saveBinding(repo.slug, provider)}
                          >
                            {binding ? m.repos_binding_update() : m.repos_binding_bind()}
                          </Button>
                          {#if binding}
                            <Button
                              tone="quiet"
                              pending={bindingBusy === key}
                              onclick={() => removeBinding(repo.slug, provider)}
                            >
                              {m.repos_binding_remove()}
                            </Button>
                          {/if}
                        {:else if binding}
                          <span class="of-mono text-sm text-muted">{binding.externalRef}</span>
                          <span class="text-xs text-faint">
                            {m.repos_binding_trigger_display({ label: binding.triggerLabel })}
                          </span>
                        {:else}
                          <span class="text-xs text-faint">{m.repos_binding_unbound()}</span>
                        {/if}
                      </div>

                      {#if binding && !binding.live}
                        {@const parts = around(
                          m.repos_binding_inactive({ provider: name, link: LINK })
                        )}
                        <p class="mt-1 text-xs text-faint">
                          {parts[0]}<a
                            class="underline hover:text-ink"
                            href="/o/{org.slug}/trackers">{m.repos_trackers_heading()}</a
                          >{parts[1]}
                        </p>
                      {/if}
                      {#if bindingError[key]}
                        <p class="mt-1 text-xs text-rose-400">{bindingError[key]}</p>
                      {/if}
                    </li>
                  {/each}
                </ul>

                <p class="mt-3 text-xs text-faint">
                  {m.repos_bindings_note({ githubRef: 'owner/repo' })}
                </p>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>
