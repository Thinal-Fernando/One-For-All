// What the island knows about each agent session or terminal job.
// Mirrors ofa-core's states; the real list comes from the Rust session store
// in milestone 3.

export type SessionState = "needs-you" | "failed" | "working" | "done" | "idle";

export interface Session {
  id: string;
  /** Where it runs: "Claude Code", "Codex" or "Terminal". */
  source: string;
  title: string;
  state: SessionState;
  /** One short line about what is happening, such as the waiting command. */
  detail: string;
}

/** Most urgent first, the same order as ofa-core's IslandState. */
const PRIORITY: SessionState[] = ["needs-you", "failed", "working", "done", "idle"];

/** The state the island shows: the most urgent across all sessions. */
export function islandState(sessions: Session[]): SessionState {
  let best = PRIORITY.length - 1;
  for (const session of sessions) {
    best = Math.min(best, PRIORITY.indexOf(session.state));
  }
  return PRIORITY[best];
}

export const LABELS: Record<SessionState, string> = {
  "needs-you": "Needs you",
  failed: "Failed",
  working: "Working",
  done: "Done",
  idle: "Idle",
};
