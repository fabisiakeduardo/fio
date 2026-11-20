<script lang="ts">
  /**
   * Left column — everything that can act as a route source:
   * live playback streams, physical capture devices, recently-seen app
   * names, and a free-text field for a new AppName rule.
   */
  import RouteForm from "./RouteForm.svelte";
  import { createRoute, fio } from "./fio.svelte";
  import {
    captureSources,
    sinkNameForIndex,
    streamLabel,
    type RouteTarget,
  } from "./types";

  let newApp = $state("");

  const ui = $derived(fio.ui);
  const captures = $derived(ui ? captureSources(ui) : []);
  const liveAppNames = $derived(
    new Set(
      (ui?.live_streams ?? [])
        .map((s) => s.app_name ?? s.media_name)
        .filter((n): n is string => n != null),
    ),
  );

  function route(source: Parameters<typeof createRoute>[0]) {
    return (target: RouteTarget) => void createRoute(source, target);
  }

  function addAppRule(target: RouteTarget) {
    const name = newApp.trim();
    if (!name || !ui) return;
    void createRoute({ AppName: name }, target).then((ok) => {
      if (ok) newApp = "";
    });
  }
</script>

<section class="col" aria-label="sources">
  <header class="col-head">
    <h2>Sources</h2>
  </header>

  {#if ui}
    <div class="group">
      <h3>Live streams</h3>
      {#if ui.live_streams.length === 0}
        <p class="empty">nothing is playing audio right now</p>
      {:else}
        {#each ui.live_streams as stream (stream.index)}
          {@const key = stream.app_name ?? stream.media_name}
          <div class="row card">
            <div class="row-main">
              <span class="name">{streamLabel(stream)}</span>
              <span class="sub mono">→ {sinkNameForIndex(ui, stream.sink)}</span>
            </div>
            {#if key != null}
              <RouteForm
                {ui}
                source={{ AppName: key }}
                onroute={route({ AppName: key })}
              />
            {:else}
              <span class="sub">unroutable — no app name</span>
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    <div class="group">
      <h3>Capture devices</h3>
      {#if captures.length === 0}
        <p class="empty">no physical inputs found</p>
      {:else}
        {#each captures as src (src.index)}
          <div class="row card">
            <div class="row-main">
              <span class="name">{src.description ?? src.name}</span>
              <span class="sub mono">{src.name}</span>
            </div>
            <div class="row-side">
              {#if ui.default_source === src.name}
                <span class="badge accent">default mic</span>
              {/if}
              <RouteForm
                {ui}
                source={{ CaptureDevice: src.name }}
                onroute={route({ CaptureDevice: src.name })}
              />
            </div>
          </div>
        {/each}
      {/if}
    </div>

    <div class="group">
      <h3>Seen apps</h3>
      {#if ui.seen_apps.length === 0}
        <p class="empty">no app names recorded yet</p>
      {:else}
        {#each ui.seen_apps as app (app)}
          <div class="row card">
            <div class="row-main">
              <span class="name">{app}</span>
              {#if liveAppNames.has(app)}
                <span class="dot" title="currently streaming"></span>
              {/if}
            </div>
            <RouteForm
              {ui}
              source={{ AppName: app }}
              onroute={route({ AppName: app })}
            />
          </div>
        {/each}
      {/if}
    </div>

    <div class="group">
      <h3>New app rule</h3>
      <div class="row card">
        <input
          class="grow"
          placeholder="application.name — e.g. discord"
          bind:value={newApp}
        />
        <RouteForm
          {ui}
          source={{ AppName: newApp.trim() }}
          placeholder="route to…"
          disabled={newApp.trim() === ""}
          onroute={addAppRule}
        />
      </div>
      <p class="hint">
        the app-name rule key is PulseAudio's <span class="mono"
          >application.name</span
        > property (falls back to <span class="mono">media.name</span>)
      </p>
    </div>
  {/if}
</section>
