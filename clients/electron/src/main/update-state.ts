/**
 * What the update strip shows, and whether an install may run now.
 *
 * Pure, so it is tested without Electron; `updates.ts` wires it to
 * electron-updater and the Core (DECISIONS Q324).
 */

import type { UpdateBusy } from "@protocol/UpdateBusy";

/**
 * Why Restart to update has not restarted yet (Q4). `unreadable`: something
 * answered, or failed to, in a way that does not prove the Core idle — so
 * the install waits rather than guess.
 */
export type UpdateWait = UpdateBusy | "unreadable";

/**
 * The Core's answer to `update/hold`. `absent`: no Core is listening at all,
 * so there is nothing to interrupt.
 */
export type HoldAnswer = "held" | "absent" | UpdateWait;

/**
 * The offer, as the main process holds it and pushes it to every window.
 *
 * `failed` is only ever the Operator's own action failing — a background
 * check that fails changes nothing (Q17). `retry` names what Try again does.
 */
export type UpdateState =
  | { phase: "none" }
  | { phase: "available"; version: string }
  | { phase: "downloading"; version: string; percent: number }
  | { phase: "downloaded"; version: string }
  | { phase: "waiting"; version: string; busy: UpdateWait }
  | { phase: "installing"; version: string }
  | { phase: "failed"; version: string; retry: "download" | "install"; message: string };

export type UpdateEvent =
  | { type: "found"; version: string }
  | { type: "dismissed" }
  | { type: "downloadStarted" }
  | { type: "progress"; percent: number }
  | { type: "downloaded" }
  | { type: "downloadFailed"; message: string }
  | { type: "installWaiting"; busy: UpdateWait }
  | { type: "installStarted" }
  | { type: "installFailed"; message: string };

/** Returns `state` itself when the event changes nothing, so callers can skip a push. */
export function nextUpdateState(state: UpdateState, event: UpdateEvent): UpdateState {
  switch (event.type) {
    case "found":
      // A check finding the version already downloaded must not take the
      // Operator back a step.
      if (
        state.phase === "none" ||
        state.phase === "available" ||
        (state.phase === "failed" && state.retry === "download")
      ) {
        return state.phase === "available" && state.version === event.version
          ? state
          : { phase: "available", version: event.version };
      }
      return state;
    case "dismissed":
      return state.phase === "available" ? { phase: "none" } : state;
    case "downloadStarted":
      return state.phase === "available" ||
        (state.phase === "failed" && state.retry === "download")
        ? { phase: "downloading", version: state.version, percent: 0 }
        : state;
    case "progress": {
      if (state.phase !== "downloading") return state;
      // Whole percents: the event fires per chunk, and each push is IPC to every window.
      const percent = Math.floor(event.percent);
      return percent === state.percent ? state : { ...state, percent };
    }
    case "downloaded":
      return state.phase === "downloading" ? { phase: "downloaded", version: state.version } : state;
    case "downloadFailed":
      return state.phase === "downloading"
        ? { phase: "failed", version: state.version, retry: "download", message: event.message }
        : state;
    case "installWaiting":
      if (state.phase === "waiting") {
        return state.busy === event.busy ? state : { ...state, busy: event.busy };
      }
      return state.phase === "downloaded" ||
        (state.phase === "failed" && state.retry === "install")
        ? { phase: "waiting", version: state.version, busy: event.busy }
        : state;
    case "installStarted":
      return state.phase === "downloaded" ||
        state.phase === "waiting" ||
        (state.phase === "failed" && state.retry === "install")
        ? { phase: "installing", version: state.version }
        : state;
    case "installFailed":
      return state.phase === "installing"
        ? { phase: "failed", version: state.version, retry: "install", message: event.message }
        : state;
  }
}
