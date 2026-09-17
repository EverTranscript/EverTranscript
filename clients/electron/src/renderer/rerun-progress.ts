/**
 * What the Registry's re-run block says, as two strings.
 *
 * Split out of the component because that is the only way it can be tested
 * here (see `tsconfig.test.json`), and because what it decides is worth
 * pinning: the block reports a background job nobody asked to watch, and
 * every one of these sentences is a claim about what happened to somebody's
 * meetings. The wrong one is not a cosmetic bug.
 */

import type { DiarizeRerun } from "@protocol/DiarizeRerun";

import { plural, t } from "./i18n";

export interface RerunLines {
  /** Where the re-run stands, in one sentence. */
  state: string;
  /** The counts behind it, already joined. */
  counts: string;
  /** How much of the backlog has left the line, for the bar. */
  through: number;
}

/**
 * **`done` is not "rebuilt".** The Core counts a Meeting done when it is
 * through with it, which includes one it could not process and one deleted
 * while it waited; only the two kinds of giving up are held out, in
 * `abandoned`. So this says "gone through", and never that a voice was
 * relearned.
 *
 * **Stopped is not finished.** A Meeting somebody promoted is no longer the
 * backlog's to cancel, so a stopped re-run can still have work in line — it
 * is named as the separate request it is, rather than left to read as a
 * stopped job that somehow still owes work.
 */
export function rerunLines(rerun: DiarizeRerun): RerunLines {
  const { total, done, remaining, abandoned, cancelled, pausedForRecording } = rerun;

  let state: string;
  if (cancelled) {
    state = t("registry.rerun.stopped");
  } else if (remaining === 0) {
    state = t("registry.rerun.finished");
  } else {
    state = pausedForRecording ? t("registry.rerun.paused") : t("registry.rerun.running");
  }

  const parts = [`${done} ${t("registry.rerun.gone")}`];
  if (abandoned > 0) parts.push(`${abandoned} ${t("registry.rerun.givenUp")}`);
  if (remaining > 0) {
    parts.push(
      cancelled
        ? `${remaining} ${plural("registry.meetings", remaining)} ${t("registry.rerun.stillQueued")}`
        : `${remaining} ${t("registry.rerun.left")}`,
    );
  }

  return {
    state,
    counts: parts.join(" · "),
    // Walked and given up together: what a bar can honestly measure is what
    // has left the line, and the words underneath tell the two apart.
    through: Math.max(0, total - remaining),
  };
}

/**
 * Whether the re-run actually moved between two answers.
 *
 * The Registry pulls `diarize/status` every two seconds, and a change is what
 * decides whether to pull the Speaker list with it. Two things make this
 * worth writing down rather than inlining. **Absence is a state, not a
 * change:** an installation that has never changed models answers `null`
 * forever, and calling `null` → `null` a move would refetch the whole list
 * every two seconds for as long as the Registry is open. And what counts as
 * movement is only what the rows underneath can see — a re-run standing aside
 * for a recording has changed nothing about any Speaker.
 */
export function rerunMoved(seen: DiarizeRerun | null, next: DiarizeRerun | null): boolean {
  if (seen === null || next === null) return (seen === null) !== (next === null);
  return (
    seen.done !== next.done ||
    seen.remaining !== next.remaining ||
    seen.abandoned !== next.abandoned ||
    seen.cancelled !== next.cancelled ||
    seen.model !== next.model ||
    seen.modelVersion !== next.modelVersion
  );
}
