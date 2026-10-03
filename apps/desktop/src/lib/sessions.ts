// What the island knows about each agent session. Matches
// SessionView in src-tauri/src/sessions.rs, which sends the full list.

export type SessionState = "needs-you" | "failed" | "working" | "done" | "idle" | "lost";

export interface Session {
  id: string;
  /** Which agent: "Claude Code". */
  source: string;
  title: string;
  state: SessionState;
  /** One short line about what is happening, such as the waiting command. */
  detail: string;
  /** Which permission prompt is showing; sent back with an answer. */
  prompt: number;
  /** Whether that prompt can be answered from the island. */
  answerable: boolean;
}

/** Most urgent first, the same order as ofa-core's IslandState. */
const PRIORITY: SessionState[] = ["needs-you", "failed", "working", "done", "idle"];

/**
 * The state the island shows: the most urgent across all sessions. A lost
 * session is listed in the panel but doesn't light up the island.
 */
export function islandState(sessions: Session[]): SessionState {
  let best = PRIORITY.length - 1;
  for (const session of sessions) {
    if (session.state !== "lost") {
      best = Math.min(best, PRIORITY.indexOf(session.state));
    }
  }
  return PRIORITY[best];
}

export const LABELS: Record<SessionState, string> = {
  "needs-you": "Needs you",
  failed: "Failed",
  working: "Working",
  done: "Done",
  idle: "Idle",
  lost: "Lost",
};
