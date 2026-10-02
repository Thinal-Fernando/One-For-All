<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { LABELS, islandState, type Session } from "./lib/sessions";

  // The Rust side polls the cursor and tells us when to open, because a
  // click-through window never receives hover events of its own.
  let open = $state(false);
  let sessions = $state<Session[]>([]);
  let island: HTMLDivElement;

  const current = $derived(islandState(sessions));
  const active = $derived(sessions.filter((s) => s.state !== "idle"));

  onMount(() => {
    // Rust only treats the painted island as solid, so report its box
    // whenever it changes size, including every frame of an animation.
    const report = () => {
      const box = island.getBoundingClientRect();
      invoke("set_hit_area", {
        x: box.x,
        y: box.y,
        width: box.width,
        height: box.height,
      });
    };
    const observer = new ResizeObserver(report);
    observer.observe(island);

    const unlisten = listen<boolean>("island-hover", (event) => {
      open = event.payload;
    });

    // Listen first, then fetch, so no change can slip in between.
    const unlistenSessions = listen<Session[]>("sessions", (event) => {
      sessions = event.payload;
    });
    unlistenSessions.then(() =>
      invoke<Session[]>("get_sessions").then((current) => (sessions = current)),
    );

    return () => {
      observer.disconnect();
      unlisten.then((stop) => stop());
      unlistenSessions.then((stop) => stop());
    };
  });
</script>

<div
  class="island {current}"
  class:open
  class:busy={current !== "idle"}
  bind:this={island}
  role="status"
  aria-label="OFA: {LABELS[current]}"
>
  {#if open}
    <ul class="panel">
      {#each active as session (session.id)}
        <li>
          <button
            class="row"
            title="Show this session's terminal"
            onclick={() => invoke("focus_session", { id: session.id })}
          >
            <span class="mark {session.state}"></span>
            <span class="text">
              <span class="title">{session.title}</span>
              <span class="detail">{session.source} · {session.detail}</span>
            </span>
            <span class="state {session.state}">{LABELS[session.state]}</span>
          </button>
        </li>
      {:else}
        <li class="empty">Nothing running</li>
      {/each}
    </ul>
  {:else if current !== "idle"}
    <div class="compact">
      <span class="mark {current}"></span>
      <span class="label {current}">{LABELS[current]}</span>
    </div>
  {/if}
</div>

<style>
  .island {
    --amber: #f5b83d;
    --red: #f87171;
    --blue: #6ea0ff;
    --green: #4ade80;
    --ease: cubic-bezier(0.2, 0.9, 0.3, 1.1);

    box-sizing: border-box;
    width: 160px;
    height: 32px;
    margin-top: 6px;
    border-radius: 16px;
    background: #000;
    color: #f2f2f2;
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.08);
    overflow: hidden;
    transition:
      width 260ms var(--ease),
      height 260ms var(--ease),
      border-radius 260ms ease,
      box-shadow 260ms ease;
  }

  .island.busy {
    width: 200px;
  }

  .island.open {
    width: 360px;
    height: 150px;
    border-radius: 24px;
  }

  /* Only a permission request pulses, so it stands out from everything else. */
  .island.needs-you:not(.open) {
    animation: pulse 1.6s ease-in-out infinite;
  }

  .island.failed:not(.open) {
    box-shadow: 0 0 0 1.5px var(--red);
  }

  @keyframes pulse {
    0%,
    100% {
      box-shadow: 0 0 0 1.5px var(--amber);
    }
    50% {
      box-shadow:
        0 0 0 1.5px var(--amber),
        0 0 14px 2px rgba(245, 184, 61, 0.55);
    }
  }

  .compact {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 100%;
    padding: 0 14px;
    font-size: 12.5px;
    font-weight: 600;
    animation: appear 200ms ease both;
  }

  .label {
    flex: 1;
    text-align: right;
  }

  .panel {
    list-style: none;
    margin: 0;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    animation: appear 180ms 80ms ease both;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-width: 0;
    margin: -4px -6px;
    padding: 4px 6px;
    box-sizing: content-box;
    border: 0;
    border-radius: 10px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .title {
    font-size: 13px;
    font-weight: 600;
  }

  .detail {
    font-size: 11.5px;
    color: #9aa3b2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .state {
    font-size: 11.5px;
    font-weight: 600;
  }

  .empty {
    font-size: 12.5px;
    color: #9aa3b2;
    text-align: center;
    padding-top: 36px;
  }

  /* One mark per state: spinner, pulsing dot, cross-like dot, check-like dot. */
  .mark {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }

  .mark.working {
    border: 2px solid rgba(110, 160, 255, 0.25);
    border-top-color: var(--blue);
    box-sizing: border-box;
    animation: spin 0.9s linear infinite;
  }

  .mark.needs-you {
    background: var(--amber);
    animation: blink 1.6s ease-in-out infinite;
  }

  .mark.failed {
    background: var(--red);
  }

  .mark.done {
    background: var(--green);
  }

  .mark.lost {
    background: #6b7280;
  }

  .needs-you.label,
  .needs-you.state {
    color: var(--amber);
  }
  .failed.label,
  .failed.state {
    color: var(--red);
  }
  .working.label,
  .working.state {
    color: var(--blue);
  }
  .done.label,
  .done.state {
    color: var(--green);
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes blink {
    50% {
      opacity: 0.35;
    }
  }

  @keyframes appear {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .island,
    .compact,
    .panel {
      transition: none;
      animation: none;
    }
    .island.needs-you:not(.open) {
      box-shadow: 0 0 0 1.5px var(--amber);
    }
    .mark.working {
      animation-duration: 2.5s;
    }
  }
</style>
