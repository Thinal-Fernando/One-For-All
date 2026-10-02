// Fake sessions for milestone 2, so every island state and animation can be
// checked before real events exist. Removed once the session store lands.

import type { Session, SessionState } from "./sessions";

const STEP_MS = 4000;

const start: Session[] = [
  { id: "a", source: "Claude Code", title: "one-for-all", state: "idle", detail: "" },
  { id: "b", source: "Codex", title: "api-platform", state: "idle", detail: "" },
  { id: "c", source: "Terminal", title: "cargo test", state: "idle", detail: "" },
];

type Change = [id: string, state: SessionState, detail: string];

// Walks the island through idle, working, needs you, failed, done and back.
const script: Change[][] = [
  [["a", "working", "Editing src/island.rs"]],
  [["b", "needs-you", "Wants to run npm test"]],
  [["b", "working", "Running npm test"]],
  [["c", "failed", "Exited with code 101 after 42 s"]],
  [
    ["c", "idle", ""],
    ["b", "idle", ""],
    ["a", "done", "Finished in 3 min"],
  ],
  [["a", "idle", ""]],
];

/** Calls `onChange` with the fake sessions every few seconds, forever. */
export function runDemo(onChange: (sessions: Session[]) => void): () => void {
  let sessions = start.map((s) => ({ ...s }));
  let step = 0;
  onChange(sessions);

  const timer = setInterval(() => {
    if (step === script.length) {
      sessions = start.map((s) => ({ ...s }));
      step = 0;
    } else {
      for (const [id, state, detail] of script[step]) {
        sessions = sessions.map((s) => (s.id === id ? { ...s, state, detail } : s));
      }
      step += 1;
    }
    onChange(sessions);
  }, STEP_MS);

  return () => clearInterval(timer);
}
