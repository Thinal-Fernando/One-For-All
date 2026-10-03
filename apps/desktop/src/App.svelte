<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { playRipple, type Side } from "./lib/ripple";
  import { LABELS, islandState, type Session } from "./lib/sessions";
  import { clockTime, level, resetsIn, tokens, type Usage } from "./lib/usage";

  interface Layout {
    edge: Side;
    size: number;
  }

  /** Gap between the orb and the screen edge, in px. */
  const MARGIN = 10;
  /** Gap between the pop-up and the screen edge, in px. */
  const POP_MARGIN = 12;
  /** Extra room around the orb that still counts as hovering it, in px. */
  const HIT_SLACK = 8;

  // The Rust side polls the cursor and tells us when to open, because a
  // click-through window never receives hover events of its own.
  let open = $state(false);
  let sessions = $state<Session[]>([]);
  let usage = $state<Usage | null>(null);
  let layout = $state<Layout>({ edge: "right", size: 20 });
  // Ticks once a minute so reset times stay current.
  let now = $state(Date.now());
  let width = $state(window.innerWidth);
  let height = $state(window.innerHeight);
  /** rest: orb on the edge; sinking: going in; open: pop-up shown; rising: coming back. */
  let phase = $state<"rest" | "sinking" | "open" | "rising">("rest");
  let landed = $state(false);

  let orb: HTMLButtonElement;
  let pop: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let stopRipple = () => {};
  let timers: ReturnType<typeof setTimeout>[] = [];

  const current = $derived(islandState(sessions));
  const active = $derived(sessions.filter((s) => s.state !== "idle"));

  // Where the orb rests, and where it goes when it sinks into its edge.
  const rest = $derived.by(() => {
    const s = layout.size;
    if (layout.edge === "right") return { x: width - s - MARGIN, y: (height - s) / 2 };
    if (layout.edge === "left") return { x: MARGIN, y: (height - s) / 2 };
    return { x: (width - s) / 2, y: MARGIN };
  });
  const sunk = $derived.by(() => {
    const s = layout.size;
    if (layout.edge === "right") return { x: width - s * 0.35, y: rest.y };
    if (layout.edge === "left") return { x: -s * 0.65, y: rest.y };
    return { x: rest.x, y: -s * 0.65 };
  });
  const orbAt = $derived(phase === "rest" || phase === "rising" ? rest : sunk);

  // Answers go back with the prompt's number, so a click that arrives after
  // the prompt was answered in the terminal is ignored rather than misfiring.
  function answer(session: Session, allow: boolean) {
    invoke("answer_prompt", { id: session.id, prompt: session.prompt, allow }).catch(
      (err) => console.warn("OFA: answer not sent:", err),
    );
  }

  function later(ms: number, fn: () => void) {
    timers.push(setTimeout(fn, ms));
  }

  function clearTimers() {
    timers.forEach(clearTimeout);
    timers = [];
  }

  function sinkAndOpen() {
    if (phase === "sinking" || phase === "open") return;
    clearTimers();
    phase = "sinking";
    later(260, () => {
      landed = true;
      const entry = layout.edge === "top" ? rest.x + layout.size / 2 : rest.y + layout.size / 2;
      stopRipple();
      stopRipple = playRipple(canvas, layout.edge, entry, layout.size, "#050506");
    });
    later(520, () => (phase = "open"));
  }

  function closeAndRise() {
    if (phase === "rest" || phase === "rising") return;
    clearTimers();
    phase = "rising";
    later(140, () => (landed = false));
    later(560, () => (phase = "rest"));
  }

  $effect(() => {
    if (open) sinkAndOpen();
    else closeAndRise();
  });

  // The pop-up opens from where the orb went in, kept inside the window.
  let popStyle = $state("");
  function placePop() {
    if (!pop) return;
    const pw = pop.offsetWidth;
    const ph = pop.offsetHeight;
    const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(lo, v), hi);
    if (layout.edge === "top") {
      const left = clamp(rest.x + layout.size / 2 - pw / 2, POP_MARGIN, width - pw - POP_MARGIN);
      popStyle = `top: ${POP_MARGIN}px; left: ${left}px; transform-origin: center top;`;
    } else {
      const top = clamp(rest.y + layout.size / 2 - ph / 2, POP_MARGIN, height - ph - POP_MARGIN);
      popStyle = `${layout.edge}: ${POP_MARGIN}px; top: ${top}px; transform-origin: ${layout.edge} center;`;
    }
  }

  // Rust only treats what is painted as solid: the orb at rest, and while
  // open the pop-up together with the orb's spot, so the cursor can travel
  // from one to the other without the island closing. The orb's spot runs
  // out to the screen edge with a little slack around it, because people
  // throw the cursor against the edge rather than onto a 20px dot.
  function reportHitArea() {
    const s = layout.size + HIT_SLACK * 2;
    const x = rest.x - HIT_SLACK;
    const y = rest.y - HIT_SLACK;
    let box =
      layout.edge === "right"
        ? { x, y, width: width - x, height: s }
        : layout.edge === "left"
          ? { x: 0, y, width: x + s, height: s }
          : { x, y: 0, width: s, height: y + s };
    if (phase !== "rest" && pop) {
      // The pop-up's laid-out box, not its on-screen one: it grows from a
      // small scale as it opens, and a box measured mid-grow would leave
      // most of it outside the hit area, so hovering it would close it.
      const p = { x: pop.offsetLeft, y: pop.offsetTop, width: pop.offsetWidth, height: pop.offsetHeight };
      const x = Math.min(box.x, p.x);
      const y = Math.min(box.y, p.y);
      box = {
        x,
        y,
        width: Math.max(box.x + box.width, p.x + p.width) - x,
        height: Math.max(box.y + box.height, p.y + p.height) - y,
      };
    }
    invoke("set_hit_area", box);
  }

  $effect(() => {
    // Re-run whenever any of these change.
    void [phase, layout.edge, layout.size, width, height, active.length, usage];
    placePop();
    queueMicrotask(reportHitArea);
  });

  onMount(() => {
    const onResize = () => {
      width = window.innerWidth;
      height = window.innerHeight;
    };
    window.addEventListener("resize", onResize);
    const popObserver = new ResizeObserver(() => {
      placePop();
      reportHitArea();
    });
    popObserver.observe(pop);

    const unlisten = listen<boolean>("island-hover", (event) => {
      open = event.payload;
    });
    const unlistenLayout = listen<Layout>("island-layout", (event) => {
      layout = event.payload;
    });
    unlistenLayout.then(() =>
      invoke<Layout>("get_island_layout").then((current) => (layout = current)),
    );

    // Listen first, then fetch, so no change can slip in between.
    const unlistenSessions = listen<Session[]>("sessions", (event) => {
      sessions = event.payload;
    });
    unlistenSessions.then(() =>
      invoke<Session[]>("get_sessions").then((current) => (sessions = current)),
    );

    const unlistenUsage = listen<Usage>("usage", (event) => {
      usage = event.payload;
    });
    unlistenUsage.then(() => invoke<Usage>("get_usage").then((current) => (usage = current)));
    const clock = setInterval(() => (now = Date.now()), 60_000);

    return () => {
      window.removeEventListener("resize", onResize);
      popObserver.disconnect();
      clearTimers();
      stopRipple();
      unlisten.then((stop) => stop());
      unlistenLayout.then((stop) => stop());
      unlistenSessions.then((stop) => stop());
      unlistenUsage.then((stop) => stop());
      clearInterval(clock);
    };
  });
</script>

<div class="stage" style="--size: {layout.size}px">
  <canvas class="ripple" bind:this={canvas}></canvas>

  <button
    class="orb {current}"
    class:landed
    class:rising={phase === "rising" || phase === "rest"}
    style="left: {orbAt.x}px; top: {orbAt.y}px"
    bind:this={orb}
    aria-label="OFA: {LABELS[current]}"
    tabindex="-1"
  >
    <span class="dot"></span>
  </button>

  <div
    class="pop"
    class:open={phase === "open"}
    style={popStyle}
    bind:this={pop}
    role="dialog"
    aria-label="OFA details"
  >
    <div class="pop-head">
      <b>Claude Code</b>
      <span>
        {active.length ? `${active.length} session${active.length > 1 ? "s" : ""}` : "Nothing running"}
      </span>
    </div>
    <ul class="rows">
      {#each active as session (session.id)}
        <li class="item">
          <button
            class="row"
            title={session.answerable ? undefined : "Show this session's terminal"}
            onclick={() => invoke("focus_session", { id: session.id })}
          >
            <span class="mark {session.state}"></span>
            <span class="text">
              <span class="title">{session.title}</span>
              <span class="detail">{session.source} · {session.detail}</span>
            </span>
            {#if !session.answerable}
              <span class="state {session.state}">{LABELS[session.state]}</span>
            {/if}
          </button>
          {#if session.answerable}
            <span class="answers">
              <button class="answer deny" onclick={() => answer(session, false)}>Deny</button>
              <button class="answer allow" onclick={() => answer(session, true)}>Allow</button>
            </span>
          {/if}
        </li>
      {:else}
        <li class="empty">No Claude Code sessions right now</li>
      {/each}
    </ul>
    {#if usage?.plan}
      {@const session = usage.plan.session}
      {@const weekly = usage.plan.weekly}
      <div class="plan">
        <div class="plan-head">
          <span>Claude plan · 5-hour limit</span>
          <span>
            {Math.round(session.percent)}% used{session.resets_at
              ? ` · resets ${clockTime(session.resets_at, now)}`
              : ""}
          </span>
        </div>
        <div class="bar">
          <div
            class="fill {level(session.percent)}"
            style="width: {Math.min(100, session.percent)}%"
          ></div>
        </div>
        {#if weekly}
          <div class="plan-week">
            Weekly {Math.round(weekly.percent)}%{weekly.resets_at
              ? ` · resets ${clockTime(weekly.resets_at, now)}`
              : ""}
          </div>
        {/if}
      </div>
    {:else if usage && (usage.window_tokens > 0 || usage.week_tokens > 0)}
      <p class="usage" title="Estimated from Claude Code's logs on this PC">
        {#if usage.window_resets_at}
          {tokens(usage.window_tokens)} tokens this window · {resetsIn(usage.window_resets_at, now)}
        {:else}
          No usage window open
        {/if}
        · {tokens(usage.week_tokens)} in 7 days
      </p>
    {/if}
  </div>
</div>

<style>
  .stage {
    --orb: #050506;
    --idle: #8b919b;
    --amber: #f5b83d;
    --red: #f87171;
    --blue: #6ea0ff;
    --green: #4ade80;
    --pop-muted: #9aa1ad;

    position: fixed;
    inset: 0;
    overflow: hidden;
    color: #f1f2f4;
  }

  .ripple {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

  /* ---------- the orb ---------- */
  .orb {
    position: absolute;
    width: var(--size);
    height: var(--size);
    border-radius: 50%;
    border: 0;
    padding: 0;
    margin: 0;
    line-height: 0;
    font-size: 0;
    appearance: none;
    background: var(--orb);
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.1),
      0 6px 18px rgba(0, 0, 0, 0.35);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: default;
    transition:
      left 300ms cubic-bezier(0.55, 0, 0.75, 0.2),
      top 300ms cubic-bezier(0.55, 0, 0.75, 0.2),
      transform 220ms ease,
      opacity 200ms ease;
  }

  .orb.rising {
    transition:
      left 380ms cubic-bezier(0.2, 0.8, 0.3, 1.15),
      top 380ms cubic-bezier(0.2, 0.8, 0.3, 1.15),
      transform 220ms ease,
      opacity 200ms ease;
  }

  .orb.landed {
    transform: scale(0.5);
    opacity: 0;
  }

  .dot {
    display: block;
    flex: none;
    width: max(6px, calc(var(--size) * 0.3));
    height: max(6px, calc(var(--size) * 0.3));
    border-radius: 50%;
    background: var(--c);
    box-shadow: 0 0 calc(var(--size) * 0.2) var(--c);
  }

  .orb.idle {
    --c: var(--idle);
  }
  .orb.working {
    --c: var(--blue);
  }
  .orb.needs-you {
    --c: var(--amber);
    animation: halo 2.6s ease-in-out infinite;
  }
  .orb.done {
    --c: var(--green);
  }
  .orb.failed {
    --c: var(--red);
    box-shadow:
      0 0 0 1.5px var(--red),
      0 6px 18px rgba(0, 0, 0, 0.35);
  }
  .orb.idle .dot {
    animation: flicker 3.2s steps(1, end) infinite;
    box-shadow: none;
  }
  .orb.working .dot {
    animation: breathe 2.4s ease-in-out infinite;
  }
  .orb.needs-you .dot {
    animation: breathe 1.8s ease-in-out infinite;
  }

  @keyframes flicker {
    0%,
    100% {
      opacity: 0.85;
    }
    8% {
      opacity: 0.35;
    }
    11% {
      opacity: 0.85;
    }
    46% {
      opacity: 0.6;
    }
    49% {
      opacity: 0.85;
    }
    72% {
      opacity: 0.3;
    }
    74% {
      opacity: 0.8;
    }
  }

  @keyframes breathe {
    0%,
    100% {
      transform: scale(0.85);
      opacity: 0.75;
    }
    50% {
      transform: scale(1.1);
      opacity: 1;
    }
  }

  @keyframes halo {
    0%,
    100% {
      box-shadow:
        0 0 0 1.5px var(--amber),
        0 6px 18px rgba(0, 0, 0, 0.35);
    }
    50% {
      box-shadow:
        0 0 0 1.5px var(--amber),
        0 0 22px 4px rgba(245, 184, 61, 0.55);
    }
  }

  /* ---------- the pop-up ---------- */
  .pop {
    position: absolute;
    width: 340px;
    max-height: calc(100% - 24px);
    background: #0b0c0e;
    border-radius: 20px;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.09),
      0 18px 40px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    opacity: 0;
    transform: scale(0.15);
    pointer-events: none;
    transition:
      transform 360ms cubic-bezier(0.2, 0.9, 0.3, 1.12),
      opacity 160ms ease;
  }

  .pop.open {
    opacity: 1;
    transform: scale(1);
    pointer-events: auto;
  }

  .pop-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 16px 6px;
    font-size: 12px;
    color: var(--pop-muted);
  }

  .pop-head b {
    color: #f1f2f4;
    font-size: 13px;
  }

  .rows {
    list-style: none;
    margin: 0;
    padding: 4px 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: auto;
    min-height: 0;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 7px 6px;
    border: 0;
    border-radius: 10px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .row:hover {
    background: rgba(255, 255, 255, 0.07);
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
    color: var(--pop-muted);
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
    color: var(--pop-muted);
    padding: 10px 6px;
  }

  .mark {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 50%;
  }

  .mark.working {
    width: 11px;
    height: 11px;
    box-sizing: border-box;
    border: 2px solid rgba(110, 160, 255, 0.25);
    border-top-color: var(--blue);
    animation: spin 0.9s linear infinite;
  }
  .mark.needs-you {
    background: var(--amber);
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

  .needs-you.state {
    color: var(--amber);
  }
  .failed.state {
    color: var(--red);
  }
  .working.state {
    color: var(--blue);
  }
  .done.state {
    color: var(--green);
  }
  .lost.state {
    color: var(--pop-muted);
  }

  .answers {
    flex: none;
    display: flex;
    gap: 6px;
  }

  .answer {
    border: 0;
    border-radius: 8px;
    padding: 5px 10px;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .answer.allow {
    background: var(--amber);
    color: #000;
  }

  .answer.deny {
    background: rgba(255, 255, 255, 0.12);
    color: #f2f2f2;
  }

  .answer:hover {
    filter: brightness(1.15);
  }

  .plan,
  .usage {
    margin: 0;
    padding: 8px 16px 12px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }

  .usage {
    font-size: 11px;
    color: var(--pop-muted);
  }

  .plan-head,
  .plan-week {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    font-size: 11.5px;
    color: #c4cad4;
    white-space: nowrap;
  }

  .plan-week {
    margin-top: 5px;
    font-size: 11px;
    color: var(--pop-muted);
  }

  .bar {
    margin-top: 6px;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    border-radius: 3px;
    background: var(--blue);
    transition: width 400ms ease;
  }

  .fill.warning {
    background: var(--amber);
  }

  .fill.full {
    background: var(--red);
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .orb,
    .orb.rising,
    .pop {
      transition-duration: 1ms;
    }
    .orb .dot,
    .orb.needs-you,
    .mark.working {
      animation: none;
    }
  }
</style>
