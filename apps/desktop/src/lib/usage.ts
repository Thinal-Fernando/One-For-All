// Plan usage: an estimate from Claude Code's local logs, plus the exact
// figures from Claude when that opt-in is on. Matches UsageView in
// src-tauri/src/usage.rs.

export interface Limit {
  /** 0 to 100. */
  percent: number;
  /** Unix seconds, or null if Claude didn't say. */
  resets_at: number | null;
}

export interface Usage {
  window_tokens: number;
  /** When the open 5-hour window resets, in Unix seconds; null if none is open. */
  window_resets_at: number | null;
  week_tokens: number;
  /** Exact usage from Claude; null when the opt-in is off or Claude didn't answer. */
  plan: { session: Limit; weekly: Limit | null } | null;
}

/** "17:00" today, or "Thu 11:30" for another day, in local time. */
export function clockTime(unixSeconds: number, nowMs: number): string {
  const at = new Date(unixSeconds * 1000);
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  const sameDay = at.toDateString() === new Date(nowMs).toDateString();
  return sameDay ? time : `${at.toLocaleDateString([], { weekday: "short" })} ${time}`;
}

/** How the bar is coloured: calm, nearly full, or full. */
export function level(percent: number): "normal" | "warning" | "full" {
  return percent >= 95 ? "full" : percent >= 80 ? "warning" : "normal";
}

/** "850", "12K", "1.4M". */
export function tokens(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1_000_000) return `${Math.round(count / 1000)}K`;
  return `${(count / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}

/** "resets in 2 h 13 min", "resets in 4 min". */
export function resetsIn(resetsAt: number, nowMs: number): string {
  const minutes = Math.max(0, Math.ceil((resetsAt * 1000 - nowMs) / 60_000));
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return `resets in ${rest} min`;
  return rest === 0 ? `resets in ${hours} h` : `resets in ${hours} h ${rest} min`;
}
