// The small black ripple and splash the orb makes when it sinks into the
// screen edge, drawn on a canvas. The numbers match the design demo michael
// approved (version 6 of "OFA Corner Orb").

export type Side = "right" | "left" | "top";

/** How long the whole ripple lasts. */
export const RIPPLE_MS = 2000;

interface Drop {
  speed: number;
  gravity: number;
  flight: number;
  drift: number;
  radius: number;
  delay: number;
}

/** How far the water bulges out of the edge `r` px from where the orb went in. */
export function rippleDepth(r: number, ms: number, orbSize: number): number {
  const s = Math.max(orbSize, 20);
  const t = ms / 1000;
  const reach = s * 3;
  const speed = (reach / (RIPPLE_MS / 1000)) * 1.1;
  const wavelength = s * 3;
  const k = (2 * Math.PI) / wavelength;
  const front = speed * t;
  const width = s * 1.8 + s * 0.6 * t;
  const decay = Math.exp(-1.5 * t);
  const packet = Math.exp(-Math.pow((r - front) / width, 2));
  // Crests move outward a little faster than the packet, as on water.
  const surface = Math.cos(k * r - k * speed * 1.15 * t);
  const amp = s * 0.45 * decay * Math.min(1, t * 6);
  // A thin dark lip along the hit area, so troughs read as a dip in it.
  const lip = 2.5 * decay * Math.exp(-Math.pow(r / (s * 4), 2));
  // The orb's own splash where it went in.
  const splash = s * 0.3 * Math.exp(-5 * t) * Math.exp(-Math.pow(r / (s * 0.7), 2));
  return Math.max(0, lip + amp * packet * surface + splash);
}

function makeDrops(orbSize: number): Drop[] {
  const s = Math.max(orbSize, 20);
  return Array.from({ length: 7 }, (_, i) => {
    const speed = s * (2.6 + Math.random() * 1.4);
    const gravity = s * (12 + Math.random() * 4);
    return {
      speed,
      gravity,
      flight: (2 * speed) / gravity,
      drift: (i - 3) * s * (0.6 + Math.random() * 0.4),
      radius: 1.2 + Math.random() * (s * 0.07),
      delay: Math.random() * 0.06,
    };
  });
}

/**
 * Plays the ripple once on `canvas`, from the point `origin` px along `side`
 * of the canvas. Returns a function that stops it early.
 */
export function playRipple(
  canvas: HTMLCanvasElement,
  side: Side,
  origin: number,
  orbSize: number,
  color: string,
): () => void {
  const ctx = canvas.getContext("2d");
  if (!ctx || matchMedia("(prefers-reduced-motion: reduce)").matches) return () => {};
  const ratio = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  canvas.width = Math.round(w * ratio);
  canvas.height = Math.round(h * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);

  const drops = makeDrops(orbSize);
  const start = performance.now();
  let frame = 0;

  const draw = (now: number) => {
    const ms = now - start;
    ctx.clearRect(0, 0, w, h);
    if (ms >= RIPPLE_MS) return;
    const vertical = side !== "top";
    const span = Math.max(orbSize, 20) * 6;
    const from = Math.max(0, origin - span);
    const to = Math.min(vertical ? h : w, origin + span);

    ctx.beginPath();
    if (vertical) {
      const edgeX = side === "right" ? w : 0;
      const dir = side === "right" ? -1 : 1;
      ctx.moveTo(edgeX, from);
      for (let y = from; y <= to; y += 1) {
        ctx.lineTo(edgeX + dir * rippleDepth(Math.abs(y - origin), ms, orbSize), y);
      }
      ctx.lineTo(edgeX, to);
    } else {
      ctx.moveTo(from, 0);
      for (let x = from; x <= to; x += 1) ctx.lineTo(x, rippleDepth(Math.abs(x - origin), ms, orbSize));
      ctx.lineTo(to, 0);
    }
    ctx.closePath();
    ctx.fillStyle = color;
    ctx.shadowColor = "rgba(0, 0, 0, 0.3)";
    ctx.shadowBlur = 4;
    ctx.fill();

    // Splash: drops thrown out of the edge, arcing back into it.
    const t = ms / 1000;
    for (const d of drops) {
      const life = t - d.delay;
      if (life <= 0 || life > d.flight) continue;
      const out = d.speed * life - 0.5 * d.gravity * life * life;
      if (out < 0) continue;
      const along = origin + d.drift * life;
      const r = d.radius * (1 - 0.5 * (life / d.flight));
      const x = vertical ? (side === "right" ? w - out : out) : along;
      const y = vertical ? along : out;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fill();
    }
    frame = requestAnimationFrame(draw);
  };
  frame = requestAnimationFrame(draw);
  return () => {
    cancelAnimationFrame(frame);
    ctx.clearRect(0, 0, w, h);
  };
}
