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
 * client would already have resolved a hostname, and DNS is traffic.
 */

import { autoUpdater } from "electron-updater";

/** Whether a check has been started, so a second call is a no-op. */
let started = false;

/**
 * Starts update checks, if the Operator has them on.
 *
 * `enabled` comes from the Core's settings — the same value the trust
 * surface shows — rather than from a preference this process keeps
 * separately. Two sources for one
 * switch is how a switch ends up meaning different things in two places.
 */
export function startUpdateChecks(
  enabled: boolean,
  onAvailable: (version: string) => void,
): void {
  if (!enabled || started) return;
  started = true;

  // Found, never fetched: the offer links to the release page (Q343).
  // Squirrel.Mac refuses to install into an ad-hoc-signed app, so installing
  // from here waits for ADR-0016's Developer ID signing.
  autoUpdater.autoDownload = false;
  autoUpdater.on("update-available", (info) => onAvailable(info.version));

  // Never surfaced as an error the Operator must dismiss. A laptop on a
  // plane is not a problem to report, and nothing here may interrupt a
  // recording — which is running in the Core whether or not this window is
  // even open.
  autoUpdater.on("error", () => {});

  void autoUpdater.checkForUpdates().catch(() => {});
}
