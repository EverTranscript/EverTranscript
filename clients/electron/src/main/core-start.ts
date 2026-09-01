/**
 * What it means when the spawned Core stops.
 *
 * `child.on("error")` fires when a spawn *fails* — ENOENT, EACCES. It does
 * not fire when a spawn succeeds and the process is killed a moment later,
 * and that is a real case rather than a hypothetical: a macOS bundle still
 * carrying `com.apple.quarantine` has its unsigned Core SIGKILLed by
 * Gatekeeper the instant it executes, silently and with no output. Measured
 * on the real CI artifact — exit 137, empty stdout — before this existed.
 *
 * Without this the Client reported "no Core is listening", which is true and
 * useless: it names the symptom and hides the cause, and the cause has a
 * thirty-second fix the Operator can do.
 *
 * Pure and separate so it can be run by a test. The Client had no test runner
 * until Q49, and the lesson there was that a change to the start path which
 * looks obviously right is exactly the kind worth executing once.
 */

/** How soon an exit still counts as "it never came up". */
export const STARTUP_WINDOW_MS = 5_000;

export interface CoreExit {
  /** Exit code, or `null` when a signal ended it. */
  code: number | null;
  /** Signal name, or `null` for a normal exit. */
  signal: string | null;
  /** Milliseconds between spawning it and this exit. */
  msSinceSpawn: number;
  /** `process.platform`, which changes only the advice. */
  platform: string;
}

export interface CoreExitVerdict {
  /** A catalog key for the renderer, or `null` to say nothing. */
  key: string | null;
  /** Whether a later attempt should be allowed to start it again. */
  retry: boolean;
}

/**
 * Turns an exit into something to say, or nothing.
 *
 * **The exit code is not the authority; the connection is.** A verdict here
 * is only ever surfaced if the caller's retry loop also failed to connect —
 * which matters because the ordinary case of opening the Client twice exits
 * **1**, not 0, with `another EverTranscript Core is already listening`. That
 * was measured after this file first claimed it exited 0: the claim was
 * wrong, and had the caller trusted the code instead of the socket, a working
 * product would have shown an error. So a non-zero exit produces a key and
 * the key stays unread whenever a Core — this one or the one that beat it to
 * the socket — answers.
 *
 * Three cases:
 *
 * - **A signal means something killed it**, and on macOS that is
 *   overwhelmingly Gatekeeper refusing an unsigned binary out of a quarantined
 *   download. The advice differs by platform, so the key does.
 * - **A non-zero code is the Core declining to start**, which it has already
 *   explained on stderr — including the benign "already listening" case.
 * - **A clean exit says nothing worth reporting**, and nothing is known to
 *   produce one; it is handled so the set of outcomes is closed rather than
 *   because it has been observed.
 *
 * Exits after the startup window are not start failures — a Core that ran for
 * an hour and stopped is a different event — but they still clear the way for
 * a restart.
 */
export function classifyCoreExit(exit: CoreExit): CoreExitVerdict {
  if (exit.msSinceSpawn > STARTUP_WINDOW_MS) {
    return { key: null, retry: true };
  }
  if (exit.signal !== null) {
    return {
      key:
        exit.platform === "darwin"
          ? "core.start.killedQuarantine"
          : "core.start.killed",
      retry: true,
    };
  }
  if (exit.code !== 0) {
    return { key: "core.start.exited", retry: true };
  }
  return { key: null, retry: false };
}
