/**
 * In-app updates (ADR-0016 as amended by ADR-0025).
 *
 * electron-updater, because it is cross-platform and — the part that
 * matters here — it replaces the whole bundle. The Core and the Summary
 * sidecar ship *inside* that bundle, so updating the Client updates them
 * too. A Client that updated itself and left an old Core behind would be a
 * protocol-skew bug waiting for its first user, and ADR-0028's
 * additive-only rule is what makes skew survivable rather than what makes
 * it acceptable.
 *
 * **The Operator's switch is read before anything is constructed.** ADR-0034
 * makes this Sanctioned Traffic entry one, disableable, and the guarantee
 * test's final form depends on it: with updates off and models downloaded,
 * literally zero. An updater that checked the setting after building a
 * client would already have resolved a hostname, and DNS is traffic. The
 * caller reads the setting before every check, so turning it off takes
 * effect at the next one rather than the next launch.
 *
 * What the Operator sees is one state, held here and pushed to every window
 * (`update-state.ts`, DECISIONS Q324). The updater is passed in, so the
 * whole path — the install's wait and its failure included — is tested
 * without Electron (Q330).
 *
 * The Core's hold is what keeps an update from stopping a recording, and it
 * belongs to a connection: it lasts while that connection is open, then for
 * as long as the installer this Client names keeps running (Q333).
 */

import type { AppUpdater } from "electron-updater";
import { lt, valid } from "semver";

import {
  nextUpdateState,
  type HoldAnswer,
  type UpdateEvent,
  type UpdateState,
} from "./update-state.js";

/** How often a running Client checks again (Q10). A Mac app stays open for weeks. */
export const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

/** How long a waiting install waits before asking the Core again. */
export const HOLD_RETRY_MS = 10 * 1000;

/** The part of electron-updater this uses. */
export type Updater = Pick<
  AppUpdater,
  "autoDownload" | "autoInstallOnAppQuit" | "on" | "checkForUpdates" | "downloadUpdate" | "quitAndInstall"
>;

export type UpdateSteps = {
  /** Every change to the offer, for every window. */
  publish: (state: UpdateState) => void;
  /**
   * Asks the Core to hold off new work while `installers` run. Never
   * rejects: a failure is an answer.
   */
  hold: (installers: string[]) => Promise<HoldAnswer>;
  /** Ends a hold this Client was granted, by closing its connection. */
  release: () => void;
  /** Resolves when it is time to ask the Core again. */
  pause: () => Promise<void>;
};

export type Updates = ReturnType<typeof createUpdates>;

/**
 * The update offer, over `updater()` — called only once checks are on, so
 * an Operator who turned them off never has an updater constructed.
 */
export function createUpdates(updater: () => Updater, steps: UpdateSteps) {
  let configured: Updater | null = null;
  let state: UpdateState = { phase: "none" };
  // Whether this install holds the Core, so a failure knows to hand it back.
  let holding = false;
  // What the download wrote: on Windows, the installer that will stop the Core.
  let downloadedFiles: string[] = [];

  function apply(event: UpdateEvent): void {
    const next = nextUpdateState(state, event);
    if (next === state) return;
    state = next;
    steps.publish(state);
  }

  function failInstall(error: unknown): void {
    apply({ type: "installFailed", message: error instanceof Error ? error.message : String(error) });
    if (holding) {
      holding = false;
      steps.release();
    }
  }

  function configure(): Updater {
    if (configured) return configured;
    const it = updater();
    // Downloading is opt-in per update. An updater that fetched a hundred
    // megabytes in the background would be spending someone's tethered
    // connection on a decision they have not made.
    it.autoDownload = false;
    // Installing is opt-in too, and never on quit (Q11): on Windows the
    // installer stops the Core, recording or not, and on macOS it would swap
    // the bundle under a Core that is still running. Only Restart to update
    // installs, and only under the Core's hold.
    it.autoInstallOnAppQuit = false;
    // A failed check is never shown (Q17): a laptop on a plane is not a
    // problem to report. Only an install the Operator started reports here —
    // Squirrel.Mac refusing the bundle's signature arrives this way.
    it.on("error", (error: Error) => {
      if (state.phase === "installing") failInstall(error);
    });
    it.on("update-available", (info) => apply({ type: "found", version: info.version }));
    it.on("download-progress", (progress) => apply({ type: "progress", percent: progress.percent }));
    configured = it;
    return it;
  }

  return {
    /** The offer as it stands, for a window that has just opened. */
    state: (): UpdateState => state,

    /** Checks for an update. Call it only when the Operator has checks on. */
    check(): void {
      const it = configure();
      // electron-updater keeps one download per check; checking mid-download,
      // or after it, has nothing to offer the Operator that they do not have.
      if (state.phase !== "none" && state.phase !== "available" && state.phase !== "failed") return;
      void it.checkForUpdates().catch(() => {});
    },

    /** Hides the offer until the next check finds it again (Later). */
    dismiss(): void {
      apply({ type: "dismissed" });
    },

    /** Downloads the offered update, when the Operator asks for it. */
    async download(): Promise<void> {
      apply({ type: "downloadStarted" });
      if (state.phase !== "downloading") return;
      try {
        downloadedFiles = await configure().downloadUpdate();
        apply({ type: "downloaded" });
      } catch (error) {
        apply({ type: "downloadFailed", message: error instanceof Error ? error.message : String(error) });
      }
    },

    /**
     * Restart to update: installs once the Core holds off new work (Q4).
     *
     * A click while the Core records or diarizes is not refused — it waits,
     * asking again every `HOLD_RETRY_MS`, and installs once, when the Core
     * grants the hold. The hold is what makes that safe: the Core will not
     * start a recording or a Diarization while this Client is connected, nor
     * afterwards while the installer it names still runs, so nothing begins
     * between the answer and the install however late the installer gets to
     * the Core (Q333). Only no Core listening at all installs without one.
     */
    async install(): Promise<void> {
      // Waiting already, or nothing downloaded: one install at a time.
      if (state.phase !== "downloaded" && !(state.phase === "failed" && state.retry === "install")) {
        return;
      }
      let answer: HoldAnswer = await steps.hold(downloadedFiles);
      while (answer !== "held" && answer !== "absent") {
        apply({ type: "installWaiting", busy: answer });
        await steps.pause();
        answer = await steps.hold(downloadedFiles);
      }
      holding = answer === "held";
      apply({ type: "installStarted" });
      try {
        // Silent, and relaunched after: the Windows installer is a wizard
        // otherwise, which is not what Restart to update says. macOS ignores
        // both. The Core keeps running through a macOS swap — a LaunchAgent —
        // so the new Client restarts it once it is idle (Q12).
        configure().quitAndInstall(true, true);
      } catch (error) {
        failInstall(error);
      }
    },
  };
}

export type CatchUpSteps = {
  /** Asks the Core for a hold. Never rejects: a failure is an answer. */
  hold: () => Promise<HoldAnswer>;
  /** Stops the held Core and starts the one this Client shipped with. */
  restart: () => Promise<void>;
  /** Resolves when it is time to ask the Core again. */
  pause: () => Promise<void>;
};

/**
 * Restarts a Core older than this Client, once that stops nothing (Q12,
 * Q333, Q334). Returns whether it did.
 *
 * On macOS the Core is a LaunchAgent and lives through the bundle swap, so
 * after an update the new Client finds the old Core still running. It is
 * stopped only under the Core's hold: the hold is granted on an idle Core
 * and keeps it idle, so no recording can start between the look and the
 * stop — the window a status read followed by a kill would leave. A busy
 * Core is asked again until it is idle. A Core that cannot answer, one
 * older than `update/hold` among them, is left running: the next login
 * starts the new one, and stopping it without a hold could stop a
 * recording.
 *
 * Upgrade-only: an older Client — a rollback, a second copy of the app —
 * never replaces a newer Core, and a version that does not parse says
 * nothing about which side is older.
 */
export async function catchUpCore(
  coreVersion: string,
  clientVersion: string,
  steps: CatchUpSteps,
): Promise<boolean> {
  if (!isOlder(coreVersion, clientVersion)) return false;
  for (;;) {
    const answer = await steps.hold();
    if (answer === "held") {
      await steps.restart();
      return true;
    }
    if (answer === "absent" || answer === "unreadable") return false;
    await steps.pause();
  }
}

/** Whether `version` is a lower semver than `than`. Unparseable is never older. */
export function isOlder(version: string, than: string): boolean {
  return valid(version) !== null && valid(than) !== null && lt(version, than);
}
