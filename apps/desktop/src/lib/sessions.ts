// What the island knows about each agent session. Matches
// SessionView in src-tauri/src/sessions.rs, which sends the full list.

export type SessionState = "needs-you" | "failed" | "working" | "done" | "idle" | "lost";

/** A permission request in full. Matches `Request` in ofa-protocol. */
export interface Request {
  /** The tool that wants to run, such as "Bash" or "Edit". */
  tool: string;
  /** The file it would change, for tools that change files. */
  file?: string;
  /** command: a shell command; diff: lines starting with + or -; text: anything else. */
  format: "command" | "diff" | "text";
  body: string;
  /** Whether the body was cut short; the terminal shows all of it. */
  truncated: boolean;
  /** Why Claude asked, in its own words. */
  reason?: string;
}

/** "Wants to run" and the like, for the top of a request. */
export function requestVerb(request: Request): string {
  switch (request.tool) {
    case "Bash":
    case "PowerShell":
      return "Wants to run";
    case "Write":
      return "Wants to write";
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return "Wants to edit";
    default:
      return `Wants to use ${request.tool}`;
  }
}

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
  /** The whole permission request, while one is waiting. */
  request: Request | null;
  /** How long the turn took, in seconds, once it has finished or failed. */
  took_secs: number | null;
  /** Tokens the finished or failed turn used, if known. */
  tokens: number | null;
}

/** "14 s", "2 min 14 s", "1 h 5 min". */
export function took(seconds: number): string {
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? `${minutes} min ${seconds % 60} s` : `${minutes} min`;
  const rest = minutes % 60;
  return rest ? `${Math.floor(minutes / 60)} h ${rest} min` : `${Math.floor(minutes / 60)} h`;
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
