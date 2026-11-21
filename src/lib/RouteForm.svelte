<script lang="ts">
  /**
   * The "route to…" target picker, shared by every source card, the
   * free-text app-rule form, and device monitor feeds. Choosing an option
   * immediately fires `onroute` with the selected RouteTarget; the select
   * then resets to its placeholder. Targets already routed from `source`
   * are shown disabled.
   */
  import type { RouteSource, RouteTarget, UiState } from "./types";
  import { physicalSinks, sameSource, sameTarget } from "./types";

  interface Props {
    ui: UiState;
    source: RouteSource;
    /** Targets to hide entirely (e.g. a monitor feed's own sink). */
    exclude?: RouteTarget[];
    placeholder?: string;
    disabled?: boolean;
    onroute: (target: RouteTarget) => void;
  }

  let {
    ui,
    source,
    exclude = [],
    placeholder = "route to…",
    disabled = false,
    onroute,
  }: Props = $props();

  let value = $state("");

  const already = $derived(
    ui.routes.filter((r) => sameSource(r.source, source)).map((r) => r.target),
  );

  const excluded = (t: RouteTarget) => exclude.some((e) => sameTarget(e, t));

  const devices = $derived(
    ui.devices
      .filter((d) => !excluded({ VirtualDevice: d.id }))
      .map((d) => ({
        key: `vd:${d.id}`,
        label: d.name,
        hint: `fio-${d.slug}`,
        disabled: already.some((t) => sameTarget(t, { VirtualDevice: d.id })),
        target: { VirtualDevice: d.id } as RouteTarget,
      })),
  );

  const sinks = $derived(
    physicalSinks(ui)
      .filter((s) => !excluded({ PhysicalSink: s.name }))
      .map((s) => ({
        key: `ps:${s.name}`,
        label: s.description ?? s.name,
        hint: s.description ? s.name : undefined,
        disabled: already.some((t) => sameTarget(t, { PhysicalSink: s.name })),
        target: { PhysicalSink: s.name } as RouteTarget,
      })),
  );

  function onchange(e: Event) {
    const key = (e.currentTarget as HTMLSelectElement).value;
    const opt = [...devices, ...sinks].find((o) => o.key === key);
    value = "";
    if (opt && !opt.disabled) onroute(opt.target);
  }
</script>

<select
  class="route-select"
  bind:value
  {disabled}
  {onchange}
  aria-label="route target"
>
  <option value="" disabled>{placeholder}</option>
  {#if devices.length > 0}
    <optgroup label="Virtual devices">
      {#each devices as opt (opt.key)}
        <option value={opt.key} disabled={opt.disabled}>
          {opt.label}{opt.hint ? ` (${opt.hint})` : ""}{opt.disabled ? " (routed)" : ""}
        </option>
      {/each}
    </optgroup>
  {/if}
  {#if sinks.length > 0}
    <optgroup label="Physical outputs">
      {#each sinks as opt (opt.key)}
        <option value={opt.key} disabled={opt.disabled}>
          {opt.label}{opt.hint ? ` (${opt.hint})` : ""}{opt.disabled ? " (routed)" : ""}
        </option>
      {/each}
    </optgroup>
  {/if}
</select>
