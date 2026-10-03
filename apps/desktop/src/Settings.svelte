<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Edge = "right" | "left" | "top";

  /** Matches UpdateStatus in src-tauri/src/updates.rs. */
  interface UpdateStatus {
    current: string;
    available: string | null;
    checking: boolean;
    installing: boolean;
    error: string | null;
  }

  interface Settings {
    exact_usage: boolean;
    island: { edge: Edge; size: number };
  }

  // Keep in step with MIN_ORB and MAX_ORB in settings.rs.
  const MIN_ORB = 16;
  const MAX_ORB = 64;
  const EDGES: { value: Edge; label: string }[] = [
    { value: "left", label: "Left" },
    { value: "top", label: "Top" },
    { value: "right", label: "Right" },
  ];

  let settings = $state<Settings | null>(null);
  let autostart = $state(false);
  let update = $state<UpdateStatus | null>(null);
  let error = $state("");
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    const unlisten = listen<UpdateStatus>("update", (e) => (update = e.payload));
    load();
    return () => void unlisten.then((stop) => stop());
  });

  async function load() {
    settings = await invoke<Settings>("get_settings");
    update = await invoke<UpdateStatus>("get_update_status");
    try {
      autostart = await invoke<boolean>("get_autostart");
    } catch (err) {
      error = String(err);
    }
  }

  // Failures show in the status line, from the "update" event.
  const checkNow = () => invoke("check_for_update").catch(() => {});
  const installNow = () => invoke("install_update").catch(() => {});

  async function setAutostart() {
    try {
      await invoke("set_autostart", { enabled: autostart });
      error = "";
    } catch (err) {
      error = String(err);
      autostart = !autostart;
    }
  }

  // Every change is saved and applied at once; the size slider waits until
  // dragging pauses so the orb isn't moved on every pixel.
  function save(delay = 0) {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      if (!settings) return;
      try {
        await invoke("save_settings", { settings: $state.snapshot(settings) });
        error = "";
      } catch (err) {
        error = String(err);
      }
    }, delay);
  }

  function setEdge(edge: Edge) {
    if (!settings || settings.island.edge === edge) return;
    settings.island.edge = edge;
    save();
  }
</script>

<main>
  <h1>Settings</h1>

  {#if settings}
    <section>
      <h2>Orb</h2>

      <div class="field">
        <span class="label" id="edge-label">Position</span>
        <div class="segments" role="radiogroup" aria-labelledby="edge-label">
          {#each EDGES as edge (edge.value)}
            <button
              role="radio"
              aria-checked={settings.island.edge === edge.value}
              class:on={settings.island.edge === edge.value}
              onclick={() => setEdge(edge.value)}
            >
              {edge.label}
            </button>
          {/each}
        </div>
      </div>

      <div class="screen" aria-hidden="true">
        <span class="mini-orb {settings.island.edge}"></span>
      </div>

      <div class="field">
        <label class="label" for="size">Size</label>
        <div class="size">
          <input
            id="size"
            type="range"
            min={MIN_ORB}
            max={MAX_ORB}
            step="2"
            bind:value={settings.island.size}
            oninput={() => save(150)}
          />
          <span class="value">{settings.island.size}px</span>
        </div>
      </div>
    </section>

    <section>
      <h2>General</h2>
      <label class="toggle">
        <span>
          <span class="label">Start with Windows</span>
          <span class="hint">Opens OFA when you sign in, so the orb is always there.</span>
        </span>
        <input type="checkbox" bind:checked={autostart} onchange={setAutostart} />
      </label>
    </section>

    <section>
      <h2>Usage</h2>
      <label class="toggle">
        <span>
          <span class="label">Exact plan usage</span>
          <span class="hint">
            Shows the percentages Claude shows, using Claude Code's saved sign-in. OFA only reads it and
            never changes it. Off: an estimate from Claude Code's logs on this PC.
          </span>
        </span>
        <input type="checkbox" bind:checked={settings.exact_usage} onchange={() => save()} />
      </label>
    </section>

    {#if update}
      <section>
        <h2>About</h2>
        <div class="field">
          <span>
            <span class="label">OFA {update.current}</span>
            <span class="hint about">
              {#if update.installing}
                Installing version {update.available}; OFA restarts when it's done.
              {:else if update.checking}
                Checking for updates…
              {:else if update.error}
                {update.error}
              {:else if update.available}
                Version {update.available} is ready.
              {:else}
                Up to date.
              {/if}
            </span>
          </span>
          {#if update.available}
            <button class="button primary" disabled={update.installing} onclick={installNow}>
              Install and restart
            </button>
          {:else}
            <button class="button" disabled={update.checking} onclick={checkNow}>Check now</button>
          {/if}
        </div>
      </section>
    {/if}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
  {/if}
</main>

<style>
  main {
    box-sizing: border-box;
    width: 100%;
    padding: 18px 22px;
    color: #e6e7ea;
    font-size: 13px;
    user-select: none;
  }

  h1 {
    margin: 0 0 14px;
    font-size: 18px;
    font-weight: 600;
  }

  h2 {
    margin: 0 0 10px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: #8b8f98;
  }

  section {
    padding: 14px 16px;
    margin-bottom: 12px;
    border-radius: 12px;
    background: #17181b;
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.06);
  }

  .field {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .label {
    font-weight: 500;
  }

  .segments {
    display: flex;
    padding: 2px;
    border-radius: 8px;
    background: #0b0c0e;
  }

  .segments button {
    padding: 5px 14px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: #a3a7b0;
    font: inherit;
    cursor: pointer;
  }

  .segments button.on {
    background: #2b2d33;
    color: #f1f2f4;
  }

  /* A little screen showing where the orb sits. */
  .screen {
    position: relative;
    width: 168px;
    height: 96px;
    margin: 12px auto;
    border-radius: 6px;
    background: linear-gradient(160deg, #2a2f3a, #1b1e25);
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.1);
  }

  .mini-orb {
    position: absolute;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #000;
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.18);
    transition:
      left 200ms ease,
      top 200ms ease;
  }

  .mini-orb::after {
    content: "";
    position: absolute;
    inset: 3px;
    border-radius: 50%;
    background: #8a8f99;
  }

  .mini-orb.right {
    left: calc(100% - 15px);
    top: calc(50% - 5px);
  }

  .mini-orb.left {
    left: 5px;
    top: calc(50% - 5px);
  }

  .mini-orb.top {
    left: calc(50% - 5px);
    top: 5px;
  }

  .size {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .size input {
    width: 170px;
    accent-color: #5b8cff;
  }

  .value {
    width: 36px;
    text-align: right;
    color: #a3a7b0;
    font-variant-numeric: tabular-nums;
  }

  .toggle {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    cursor: pointer;
  }

  .toggle > span {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .hint {
    color: #8b8f98;
    font-size: 12px;
    line-height: 1.4;
  }

  .toggle input {
    appearance: none;
    flex: none;
    position: relative;
    width: 36px;
    height: 20px;
    margin: 0;
    border-radius: 10px;
    background: #3a3d44;
    cursor: pointer;
    transition: background 150ms ease;
  }

  .toggle input::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #f1f2f4;
    transition: transform 150ms ease;
  }

  .toggle input:checked {
    background: #5b8cff;
  }

  .toggle input:checked::after {
    transform: translateX(16px);
  }

  .toggle input:focus-visible,
  .segments button:focus-visible {
    outline: 2px solid #5b8cff;
    outline-offset: 2px;
  }

  .about {
    display: block;
    margin-top: 4px;
  }

  .button {
    flex: none;
    padding: 6px 12px;
    border: 0;
    border-radius: 8px;
    background: #2b2d33;
    color: #f1f2f4;
    font: inherit;
    cursor: pointer;
  }

  .button.primary {
    background: #5b8cff;
  }

  .button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .button:focus-visible {
    outline: 2px solid #5b8cff;
    outline-offset: 2px;
  }

  .error {
    margin: 0;
    color: #ff7a7a;
  }
</style>
