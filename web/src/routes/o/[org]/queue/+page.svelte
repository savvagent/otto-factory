<script lang="ts">
  import { page } from '$app/state';
  import { replaceState } from '$app/navigation';

  import { api } from '$lib/api';
  import { messageFor } from '$lib/errors';
  import { m } from '$lib/paraglide/messages';
  import { useOrg } from '$lib/org.svelte';
  import { relative } from '$lib/format';
  import { statusLabel } from '$lib/labels';
  import type { Job, JobStatus, Repo, Team } from '$lib/types';
  import Alert from '$lib/components/Alert.svelte';
  import Empty from '$lib/components/Empty.svelte';
  import Loading from '$lib/components/Loading.svelte';
  import StatusPill from '$lib/components/StatusPill.svelte';

  /**
   * The queue, read-only.
   *
   * There is no button on this page that changes a job, and that is not an
   * omission. A job is created and finished by the agent doing the work, over
   * MCP; a human pressing "mark complete" here would be telling the queue
   * something they cannot observe, and the audit trail would record it as fact.
   * The console's job is to answer "what is happening, and why is my agent
   * waiting" — see the leases on the Repos page for the second half of that.
   *
   * Filters live in the URL so a view can be linked to. `replaceState` rather
   * than `goto`: changing a filter is not a place in history to go back to, and
   * a dozen entries per session makes the browser's back button useless.
   */

  const org = useOrg();

  const STATUSES: JobStatus[] = [
    'pending',
    'in-progress',
    'active',
    'completed',
    'failed',
    'cancelled'
  ];

  const status = $derived(
    (STATUSES as string[]).includes(page.url.searchParams.get('status') ?? '')
      ? (page.url.searchParams.get('status') as JobStatus)
      : undefined
  );
  const repo = $derived(page.url.searchParams.get('repo') ?? undefined);
  const team = $derived(page.url.searchParams.get('team') ?? undefined);
  const mine = $derived(page.url.searchParams.get('mine') === 'true');

  let jobs = $state<Job[]>([]);
  let repos = $state<Repo[]>([]);
  let teams = $state<Team[]>([]);
  let loading = $state(true);
  let error = $state<string | undefined>(undefined);

  $effect(() => {
    const slug = org.slug;
    if (!slug) return;
    void (async () => {
      try {
        const [r, t] = await Promise.all([api.repos(slug), api.teams(slug)]);
        if (org.slug !== slug) return;
        repos = r;
        teams = t;
      } catch {
        // The pickers are a convenience. Losing them is not worth replacing the
        // queue itself with an error box — the filters still work by URL.
      }
    })();
  });

  // Not $state: it identifies a request, it is never rendered, and making it
  // reactive would retrigger the effect that writes it.
  let latest = 0;

  $effect(() => {
    const slug = org.slug;
    // Read inside the effect so each becomes a dependency.
    const filters = { status, repo, team, mine, limit: 200 };
    if (!slug) return;

    // Every dependency of this effect — the org *and* each filter — starts a new
    // request, so staleness is decided by sequence rather than by comparing the
    // org alone. Two quick filter changes otherwise race, and the first
    // response can land after the second and repaint the table with rows that
    // do not match the controls the reader is looking at.
    const seq = ++latest;

    loading = true;
    error = undefined;

    void (async () => {
      try {
        const found = await api.jobs(slug, filters);
        if (seq !== latest) return;
        jobs = found;
      } catch (e) {
        if (seq !== latest) return;
        // An unregistered repo or team slug is a 404 naming what *is*
        // registered — better than an empty table, which reads as a quiet
        // queue rather than as a question nobody asked.
        error = messageFor(e, m.queue_load_failed());
        jobs = [];
      } finally {
        if (seq === latest) loading = false;
      }
    })();
  });

  function setFilter(key: string, value: string | undefined) {
    const url = new URL(page.url);
    if (value === undefined || value === '') url.searchParams.delete(key);
    else url.searchParams.set(key, value);
    replaceState(url, page.state);
  }

  const filtered = $derived(
    status !== undefined || repo !== undefined || team !== undefined || mine
  );
</script>

<div class="space-y-4">
  <div class="flex flex-wrap items-baseline justify-between gap-2">
    <h1 class="text-lg font-semibold">{m.queue_title()}</h1>
    <p class="text-xs text-faint">{m.queue_read_only_note()}</p>
  </div>

  <div class="of-card flex flex-wrap items-end gap-3 px-4 py-3">
    <label class="block">
      <span class="of-label">{m.queue_filter_status()}</span>
      <select
        class="of-input min-w-40"
        value={status ?? ''}
        onchange={(e) => setFilter('status', e.currentTarget.value)}
      >
        <option value="">{m.queue_filter_any()}</option>
        {#each STATUSES as option (option)}
          <!-- The value stays the wire enum; only the label is translated, and
               it comes from the same helper the pills use. -->
          <option value={option}>{statusLabel(option)}</option>
        {/each}
      </select>
    </label>

    <label class="block">
      <span class="of-label">{m.queue_filter_repo()}</span>
      <select
        class="of-input min-w-44"
        value={repo ?? ''}
        onchange={(e) => setFilter('repo', e.currentTarget.value)}
      >
        <option value="">{m.queue_filter_every_repo()}</option>
        {#if repo && !repos.some((r) => r.slug === repo)}
          <!-- A slug from the URL that is not registered. Rendered so the
               picker shows what the page is actually filtering on; without it
               the select falls back to blank and the error above looks
               unrelated to anything the user can see. -->
          <option value={repo}>{m.queue_repo_not_registered({ repo })}</option>
        {/if}
        {#each repos as option (option.id)}
          <option value={option.slug}>{option.slug}</option>
        {/each}
      </select>
    </label>

    {#if teams.length > 0 || team}
      <label class="block">
        <span class="of-label">{m.queue_filter_team()}</span>
        <select
          class="of-input min-w-44"
          value={team ?? ''}
          onchange={(e) => setFilter('team', e.currentTarget.value)}
        >
          <option value="">{m.queue_filter_every_team()}</option>
          {#if team && !teams.some((t) => t.slug === team)}
            <option value={team}>{m.queue_team_missing({ team })}</option>
          {/if}
          {#each teams as option (option.id)}
            <option value={option.slug}>{option.slug}</option>
          {/each}
        </select>
      </label>
    {/if}

    <label class="flex items-center gap-2 pb-2 text-sm text-muted">
      <input
        type="checkbox"
        checked={mine}
        onchange={(e) => setFilter('mine', e.currentTarget.checked ? 'true' : undefined)}
      />
      {m.queue_only_mine()}
    </label>

    {#if filtered}
      <button
        class="ml-auto pb-2 text-xs text-muted underline hover:text-ink"
        onclick={() => replaceState(new URL(page.url.pathname, location.origin), page.state)}
      >
        {m.queue_clear_filters()}
      </button>
    {/if}
  </div>

  {#if error}
    <Alert>{error}</Alert>
  {:else if loading && jobs.length === 0}
    <Loading what={m.queue_loading()} />
  {:else if jobs.length === 0}
    <Empty title={filtered ? m.queue_empty_filtered() : m.queue_empty_title()}>
      {#if !filtered}
        {m.queue_empty_hint()}
        <a class="text-muted underline hover:text-ink" href="/o/{org.slug}/connect">
          {m.queue_connect_one()}
        </a>.
      {/if}
    </Empty>
  {:else}
    <div class="of-card overflow-x-auto">
      <table class="w-full text-sm">
        <thead class="border-b border-edge/60 text-left text-xs text-faint">
          <tr>
            <th class="px-4 py-2 font-medium">{m.queue_col_job()}</th>
            <th class="px-4 py-2 font-medium">{m.queue_col_status()}</th>
            <th class="px-4 py-2 font-medium">{m.queue_col_agent()}</th>
            <th class="px-4 py-2 font-medium">{m.queue_col_ticket()}</th>
            <th class="px-4 py-2 font-medium">{m.queue_col_queued()}</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-edge/40">
          {#each jobs as job (job.id)}
            <tr class="hover:bg-raised/40">
              <td class="px-4 py-2">
                <a class="text-ink hover:underline" href="/o/{org.slug}/queue/{job.id}">
                  {job.title}
                </a>
                <div class="of-mono text-xs text-faint">{job.id}</div>
              </td>
              <td class="px-4 py-2"><StatusPill status={job.status} /></td>
              <td class="px-4 py-2 text-muted">
                {job.claimedByLabel ?? job.agentType ?? '—'}
              </td>
              <td class="px-4 py-2 text-muted">{job.ticketRef ?? '—'}</td>
              <td class="px-4 py-2 whitespace-nowrap text-faint">{relative(job.createdAt)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <!--
      Two sentences, not one built out of pieces. The count pluralizes and the
      cap is a separate remark that only sometimes applies; welding them into a
      single string would need a variant per plural category *and* per branch.
    -->
    <p class="text-xs text-faint">
      {m.queue_showing({ count: jobs.length })}
      {#if jobs.length === 200}{m.queue_showing_capped({ limit: 200 })}{/if}
    </p>
  {/if}
</div>
