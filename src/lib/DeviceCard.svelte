<script lang="ts">
  /**
   * One virtual device: node names, default-mic action + badge, drift
   * badge, routes in (to its sink) and monitor feeds out (from its
   * `.monitor` source), each with a delete affordance.
   */
  import RouteForm from "./RouteForm.svelte";
  import {
    createRoute,
    deleteDevice,
    deleteRoute,
    fio,
    setDefaultMic,
  } from "./fio.svelte";
  import {
    deviceDrifted,
    monitorName,
    routeDrift,
    routeLabel,
    sinkName,
    type RouteTarget,
    type VirtualDevice,
  } from "./types";

  interface Props {
    device: VirtualDevice;
  }
  let { device }: Props = $props();

  let confirmingDelete = $state(false);

  const ui = $derived(fio.ui);
  const monitor = $derived(monitorName(device));
  const isDefaultMic = $derived(ui?.default_source === monitor);
  const missing = $derived(ui ? deviceDrifted(ui, device.id) : false);

  const routesIn = $derived(
    (ui?.routes ?? []).filter(
      (r) => "VirtualDevice" in r.target && r.target.VirtualDevice === device.id,
    ),
  );
  const routesOut = $derived(
    (ui?.routes ?? []).filter(
      (r) => "VirtualMonitor" in r.source && r.source.VirtualMonitor === device.id,
    ),
  );

  function feedMonitor(target: RouteTarget) {
    void createRoute({ VirtualMonitor: device.id }, target);
  }
</script>

<article class="device" class:missing>
  <header class="device-head">
    <div class="device-title">
      <span class="name">{device.name}</span>
      {#if isDefaultMic}
        <span class="badge accent">default mic</span>
      {/if}
      {#if missing}
        <span class="badge warn">missing</span>
      {/if}
    </div>
    <div class="device-actions">
      {#if !isDefaultMic}
        <button onclick={() => void setDefaultMic(device.id)}>
          use as default mic
        </button>
      {/if}
      {#if confirmingDelete}
        <button
          class="danger"
          onclick={() => {
            confirmingDelete = false;
            void deleteDevice(device.id);
          }}
        >
          confirm delete
        </button>
        <button onclick={() => (confirmingDelete = false)}>cancel</button>
      {:else}
        <button class="danger" onclick={() => (confirmingDelete = true)}>
          delete
        </button>
      {/if}
    </div>
  </header>

  <div class="nodes">
    <div class="node">
      <span class="node-kind">sink</span>
      <span class="mono">{sinkName(device)}</span>
    </div>
    <div class="node">
      <span class="node-kind">mic</span>
      <span class="mono">{monitor}</span>
    </div>
  </div>

  {#if routesIn.length > 0}
    <div class="route-block">
      <h4>routes in</h4>
      {#each routesIn as r (r.id)}
        <div class="route-chip" class:drifted={ui ? routeDrift(ui, r.id) : null}>
          <span class="route-label">{ui ? routeLabel(r, ui) : ""}</span>
          {#if ui && routeDrift(ui, r.id)}
            <span class="badge warn">{routeDrift(ui, r.id)}</span>
          {/if}
          <button
            class="chip-x"
            title="delete route"
            onclick={() => void deleteRoute(r.id)}>×</button
          >
        </div>
      {/each}
    </div>
  {/if}

  {#if routesOut.length > 0}
    <div class="route-block">
      <h4>monitor feeds</h4>
      {#each routesOut as r (r.id)}
        <div class="route-chip" class:drifted={ui ? routeDrift(ui, r.id) : null}>
          <span class="route-label">{ui ? routeLabel(r, ui) : ""}</span>
          {#if ui && routeDrift(ui, r.id)}
            <span class="badge warn">{routeDrift(ui, r.id)}</span>
          {/if}
          <button
            class="chip-x"
            title="delete route"
            onclick={() => void deleteRoute(r.id)}>×</button
          >
        </div>
      {/each}
    </div>
  {/if}

  {#if ui}
    <footer class="device-foot">
      <RouteForm
        {ui}
        source={{ VirtualMonitor: device.id }}
        exclude={[{ VirtualDevice: device.id }]}
        placeholder="feed monitor to…"
        onroute={feedMonitor}
      />
    </footer>
  {/if}
</article>
