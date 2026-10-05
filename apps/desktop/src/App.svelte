<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { playRipple, type Side } from "./lib/ripple";
  import { LABELS, islandState, requestVerb, type Session } from "./lib/sessions";
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
  /** Gap between the pop-up and the request panel beside it, in px. */
  const PEEK_GAP = 10;
  /** Width of the request panel, and the most it grows to when enlarged, in px. */
  const PEEK_WIDTH = 420;
  const PEEK_BIG_WIDTH = 680;
  /** How long the request panel stays after the cursor leaves its row, so
   *  the cursor can cross the gap to it. */
  const PEEK_LINGER = 220;

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

  // The session whose permission request is shown in full beside the
  // pop-up, while its row or the panel itself is hovered.
  let peekId = $state<string | null>(null);
  let peekRow = $state<HTMLElement | null>(null);
  let big = $state(false);
  let peekTimer: ReturnType<typeof setTimeout> | undefined;

  let orb: HTMLButtonElement;
  let pop: HTMLDivElement;
  let peekEl: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let stopRipple = () => {};
  let timers: ReturnType<typeof setTimeout>[] = [];

  const current = $derived(islandState(sessions));
  const active = $derived(sessions.filter((s) => s.state !== "idle"));
  const peek = $derived(
    phase === "open" ? active.find((s) => s.id === peekId && s.request) : undefined,
  );

  // What the panel shows: the hovered request, or the last one while the
  // panel fades out, so it doesn't go blank as it goes.
  let lastPeek = $state<Session | undefined>(undefined);
  $effect(() => {
    if (peek) lastPeek = peek;
  });
  const shown = $derived(peek ?? lastPeek);

  function showPeek(session: Session, row: HTMLElement) {
    clearTimeout(peekTimer);
    if (!session.request) return hidePeek();
    if (peekId !== session.id) big = false;
    peekId = session.id;
    peekRow = row;
  }

  function stayPeek() {
    clearTimeout(peekTimer);
  }

  function hidePeek() {
    clearTimeout(peekTimer);
    peekTimer = setTimeout(() => {
      peekId = null;
      big = false;
    }, PEEK_LINGER);
  }

  /** A diff's lines with how each one should look. */
  function diffLines(body: string) {
    return body.split("\n").map((line) => ({
      line,
      kind: line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : line === "@@" ? "gap" : "",
    }));
  }

  /** "island.rs" from "C:\code\src\island.rs". */
  function fileName(path: string) {
    return path.split(/[\\/]/).pop() || path;
  }

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
    clearTimeout(peekTimer);
    peekId = null;
    big = false;
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

  // The request panel sits beside the pop-up, on the side away from the
  // screen edge, level with the hovered row. Enlarged, it takes all the room
  // on that side.
  let peekStyle = $state("");
  function placePeek() {
    if (!pop || !peekEl || !peek) return;
    const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(lo, v), hi);
    const popLeft = pop.offsetLeft;
    const popRight = popLeft + pop.offsetWidth;
    const onLeft = layout.edge === "right";
    const room = onLeft ? popLeft - PEEK_GAP - POP_MARGIN : width - popRight - PEEK_GAP - POP_MARGIN;
    const w = Math.max(0, Math.min(big ? PEEK_BIG_WIDTH : PEEK_WIDTH, room));
    const x = onLeft ? popLeft - PEEK_GAP - w : popRight + PEEK_GAP;
    let vertical: string;
    if (big) {
      vertical = `top: ${POP_MARGIN}px; height: ${height - POP_MARGIN * 2}px;`;
    } else {
      // The window fills the stage, so on-screen coordinates are window ones.
      const rowTop = peekRow ? peekRow.getBoundingClientRect().top - 12 : POP_MARGIN;
      const top = clamp(rowTop, POP_MARGIN, height - peekEl.offsetHeight - POP_MARGIN);
      vertical = `top: ${top}px; max-height: ${height - POP_MARGIN * 2}px;`;
    }
    peekStyle = `left: ${x}px; width: ${w}px; ${vertical} transform-origin: ${onLeft ? "right" : "left"} center;`;
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
    // Once it starts closing only the orb counts again, so passing back over
    // where the pop-up was doesn't bring it back.
    if ((phase === "sinking" || phase === "open") && pop) {
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
      // The request panel too, and the gap between it and the pop-up, so
      // the cursor can move across to it and scroll it.
      if (peek && peekEl) {
        const q = { x: peekEl.offsetLeft, y: peekEl.offsetTop, width: peekEl.offsetWidth, height: peekEl.offsetHeight };
        const x = Math.min(box.x, q.x);
        const y = Math.min(box.y, q.y);
        box = {
          x,
          y,
          width: Math.max(box.x + box.width, q.x + q.width) - x,
          height: Math.max(box.y + box.height, q.y + q.height) - y,
        };
      }
    }
    invoke("set_hit_area", box);
  }

  $effect(() => {
    // Re-run whenever any of these change.
    void [phase, layout.edge, layout.size, width, height, active.length, usage, peek, peekRow, big];
    placePop();
    placePeek();
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
      placePeek();
      reportHitArea();
    });
    popObserver.observe(pop);
    popObserver.observe(peekEl);

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
      clearTimeout(peekTimer);
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
    <!-- The card has a round bite out of its bottom-right corner, and the
         settings button sits in that bite as its own little piece. -->
    <div class="card">
      <div class="pop-head">
        <b>Claude Code</b>
        <span>
          {active.length ? `${active.length} session${active.length > 1 ? "s" : ""}` : "Nothing running"}
        </span>
      </div>
      <ul class="rows">
        {#each active as session (session.id)}
          <li
            class="item"
            class:peeking={peek?.id === session.id}
            onmouseenter={(e) => showPeek(session, e.currentTarget)}
            onmouseleave={hidePeek}
          >
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
            {:else if session.state === "failed" || session.state === "lost"}
              <button
                class="dismiss"
                title="Clear"
                aria-label="Clear {session.title}"
                onclick={() => invoke("dismiss_session", { id: session.id })}
              >
                <svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true">
                  <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
                </svg>
              </button>
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
    <button class="gear" title="Settings" aria-label="Settings" onclick={() => invoke("open_settings")}>
      <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
        <path
          fill="currentColor"
          fill-rule="evenodd"
          d="M5.98 3.21 L6.61 1.14 L9.39 1.14 L10.02 3.21 L11.14 3.85 L13.25 3.37 L14.64 5.77 L13.16 7.36 L13.16 8.64 L14.64 10.23 L13.25 12.63 L11.14 12.15 L10.02 12.79 L9.39 14.86 L6.61 14.86 L5.98 12.79 L4.86 12.15 L2.75 12.63 L1.36 10.23 L2.84 8.64 L2.84 7.36 L1.36 5.77 L2.75 3.37 L4.86 3.85 Z M8 5.6 A2.4 2.4 0 1 0 8 10.4 A2.4 2.4 0 1 0 8 5.6 Z"
        />
      </svg>
    </button>
  </div>

  <!-- The waiting request in full, with Claude's reason, beside the pop-up. -->
  <div
    class="peek"
    class:open={!!peek}
    class:big
    style={peekStyle}
    bind:this={peekEl}
    role="dialog"
    aria-label="Permission request"
    tabindex="-1"
    onmouseenter={stayPeek}
    onmouseleave={hidePeek}
  >
    {#if shown?.request}
      {@const s = shown}
      {@const request = shown.request}
      <div class="peek-head">
        <span class="peek-what">
          <b>{requestVerb(request)}</b>
          <span>{s.title}</span>
        </span>
        <button
          class="grow"
          title={big ? "Make smaller" : "Make bigger"}
          aria-label={big ? "Make smaller" : "Make bigger"}
          onclick={() => (big = !big)}
        >
          <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
            {#if big}
              <path d="M5 1v4H1M7 11V7h4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            {:else}
              <path d="M1 5V1h4M11 7v4H7" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            {/if}
          </svg>
        </button>
      </div>
      {#if request.file}
        <div class="peek-file" title={request.file}>{fileName(request.file)}</div>
      {/if}
      <pre class="code {request.format}">{#if request.format === "diff"}{#each diffLines(request.body) as { line, kind }}<span class="ln {kind}">{line || " "}</span>{/each}{:else}{request.body}{/if}</pre>
      {#if request.truncated}
        <p class="cut">Cut short here. The terminal shows the whole request.</p>
      {/if}
      {#if request.reason}
        <div class="why">
          <span class="why-label">Why it asked:</span>
          {request.reason}
        </div>
      {/if}
      {#if s.answerable}
        <div class="peek-answers">
          <button class="answer deny" onclick={() => answer(s, false)}>Deny</button>
          <button class="answer allow" onclick={() => answer(s, true)}>Allow</button>
        </div>
      {/if}
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
    animation: rest-breathe 4s ease-in-out infinite;
    box-shadow: none;
  }
  .orb.working .dot {
    animation: breathe 2.4s ease-in-out infinite;
  }
  .orb.needs-you .dot {
    animation: breathe 1.8s ease-in-out infinite;
  }

  /* Idle: a slow, soft fade in and out, like breathing while asleep. */
  @keyframes rest-breathe {
    0%,
    100% {
      transform: scale(0.8);
      opacity: 0.35;
    }
    50% {
      transform: scale(1);
      opacity: 0.9;
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
  /* The pop-up is two pieces, the card and the settings button, drawn with
     one outline and shadow that follow the bite in the card. */
  .pop {
    --gear: 26px;
    --gap: 3px;
    position: absolute;
    width: 340px;
    max-height: calc(100% - 24px);
    display: flex;
    flex-direction: column;
    filter: drop-shadow(0 0 0.6px rgba(255, 255, 255, 0.4)) drop-shadow(0 18px 20px rgba(0, 0, 0, 0.45));
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

  .card {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    background: #0b0c0e;
    border-radius: 20px;
    /* The bite: a circle the button's size plus a gap, centred on the button. */
    mask: radial-gradient(
      circle calc(var(--gear) / 2 + var(--gap)) at right calc(var(--gear) / 2) bottom calc(var(--gear) / 2),
      transparent calc(var(--gear) / 2 + var(--gap) - 0.5px),
      #000 calc(var(--gear) / 2 + var(--gap))
    );
  }

  .gear {
    position: absolute;
    right: 0;
    bottom: 0;
    display: grid;
    place-items: center;
    width: var(--gear);
    height: var(--gear);
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: #0b0c0e;
    color: var(--pop-muted);
    cursor: pointer;
    transition:
      color 150ms ease,
      background 150ms ease;
  }

  .gear:hover {
    background: #1b1d21;
    color: #f1f2f4;
  }

  .gear svg {
    transition: transform 400ms ease;
  }

  .gear:hover svg {
    transform: rotate(60deg);
  }

  /* Keep content out of the bite. */
  .rows:last-child {
    padding-bottom: calc(var(--gear) + var(--gap));
  }

  .bar,
  .plan-week {
    margin-right: calc(var(--gear) - 10px);
  }

  .card .usage {
    padding-right: calc(var(--gear) + var(--gap) + 8px);
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

  .dismiss {
    flex: none;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    margin-right: 4px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--pop-muted);
    cursor: pointer;
  }

  .dismiss:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #f1f2f4;
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

  .item.peeking .row {
    background: rgba(255, 255, 255, 0.07);
  }

  /* ---------- the request panel ---------- */
  .peek {
    position: absolute;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-sizing: border-box;
    padding: 12px 14px 14px;
    background: #0b0c0e;
    border-radius: 20px;
    filter: drop-shadow(0 0 0.6px rgba(255, 255, 255, 0.4)) drop-shadow(0 18px 20px rgba(0, 0, 0, 0.45));
    opacity: 0;
    transform: scale(0.96);
    pointer-events: none;
    transition:
      transform 200ms cubic-bezier(0.2, 0.9, 0.3, 1.12),
      opacity 140ms ease;
  }

  .peek.open {
    opacity: 1;
    transform: scale(1);
    pointer-events: auto;
  }

  .peek-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .peek-what {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    font-size: 12px;
    color: var(--pop-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .peek-what b {
    color: var(--amber);
    font-size: 13px;
  }

  .grow {
    flex: none;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--pop-muted);
    cursor: pointer;
  }

  .grow:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #f1f2f4;
  }

  .peek-file {
    font: 11.5px ui-monospace, "Cascadia Mono", Consolas, monospace;
    color: #c4cad4;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .code {
    margin: 0;
    padding: 10px 12px;
    max-height: 300px;
    min-height: 0;
    overflow: auto;
    border-radius: 10px;
    background: #16181c;
    border: 1px solid rgba(255, 255, 255, 0.07);
    font: 12px/1.5 ui-monospace, "Cascadia Mono", Consolas, monospace;
    color: #e6e8eb;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }

  .code,
  .why {
    scrollbar-width: thin;
    scrollbar-color: rgba(255, 255, 255, 0.22) transparent;
  }

  .peek.big .code {
    flex: 1;
    max-height: none;
  }

  .ln {
    display: block;
    margin: 0 -12px;
    padding: 0 12px;
  }

  .ln.add {
    background: rgba(74, 222, 128, 0.1);
    color: #b9f5cf;
  }

  .ln.del {
    background: rgba(248, 113, 113, 0.1);
    color: #fbc4c4;
  }

  .ln.gap {
    color: var(--pop-muted);
  }

  .cut {
    margin: 0;
    font-size: 11px;
    color: var(--pop-muted);
  }

  .why {
    flex: none;
    max-height: 120px;
    overflow: auto;
    font-size: 12px;
    line-height: 1.45;
    color: #d6dae0;
    white-space: pre-wrap;
    user-select: text;
  }

  .peek.big .why {
    max-height: 30%;
  }

  .why-label {
    font-weight: 600;
    color: #f1f2f4;
  }

  .peek-answers {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .orb,
    .orb.rising,
    .pop,
    .peek {
      transition-duration: 1ms;
    }
    .orb .dot,
    .orb.needs-you,
    .mark.working {
      animation: none;
    }
  }
</style>
