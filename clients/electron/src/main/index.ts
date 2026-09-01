/**
 * The Electron main process.
 *
 * It owns the connection to the Core and exposes it to the renderer through
 * a narrow IPC surface. The window is disposable: closing or crashing it
 * never touches a recording, because the recording lives in the Core
 * (ADR-0026).
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { app, BrowserWindow, ipcMain } from "electron";
import { join } from "node:path";

import { CoreClient } from "./core-client.js";
import { locateCore } from "./core-location.js";
import { classifyCoreExit } from "./core-start.js";
import { downloadUpdate, installUpdate, startUpdateChecks } from "./updates.js";

/**
 * The app icon, for the surfaces packaging does not cover yet.
 *
 * `resources/` sits beside `src/` and `dist/`, so from the compiled
 * `dist/main/index.js` it is two levels up. electron-builder (M5) will bake
 * the same files into the bundle; until then the Dock and the window frame
 * would show Electron's own icon, which is not this app.
 */
const RESOURCES = join(__dirname, "../../resources");
const ICON_PNG = join(RESOURCES, "icon.png");
const ICON_ICO = join(RESOURCES, "icon.ico");

/**
 * The in-flight or established connection.
 *
 * The *promise* is cached rather than the resolved client: the renderer
 * fires several requests at once on first paint, and caching only the result
 * lets each of them start its own connection before the first completes.
 */
let connecting: Promise<CoreClient> | null = null;
let client: CoreClient | null = null;
let window: BrowserWindow | null = null;
let startAttempted = false;
/** Why starting the Core failed, when it did. */
let startFailure: string | null = null;

/**
 * Where the Core binary is, or `null` if it cannot be found.
 *
 * The search itself is in `core-location.ts` and is tested there. This
 * supplies the real process's values — the part that cannot be unit-tested,
 * kept as small as possible for that reason.
 */
function coreBinary(): string | null {
  return locateCore({
    explicit: process.env.EVERTRANSCRIPT_BIN,
    platform: process.platform,
    // In a checkout this points inside Electron's own dist, where no Core
    // will be, so the search falls through to `target/` as it always did.
    resourcesPath: process.resourcesPath,
    searchPath: process.env.PATH ?? "",
    repoRoot: join(__dirname, "../../../.."),
    exists: existsSync,
  });
}

/**
 * Starts the Core if it is not already running.
 *
 * Opening the app must work even after the Operator quit the Core from the
 * tray. The Core is detached on purpose: it outlives this window, because a
 * recording must not end when someone closes a UI (ADR-0026).
 */
function startCore(): void {
  if (startAttempted) return;
  startAttempted = true;
  const binary = coreBinary();
  if (binary === null) {
    // Nothing was started, so a later attempt is not a duplicate.
    startAttempted = false;
    // A catalog key, not a sentence: the renderer owns the wording, so
    // the Operator reads it in their own language (ticket 10 — every
    // user-facing string externalized).
    startFailure = "core.start.binaryMissing";
    return;
  }
  try {
    const spawnedAt = Date.now();
    const child = spawn(binary, ["daemon"], {
      detached: true,
      stdio: "ignore",
    });
    // **A spawn that succeeds and is killed a moment later is not an
    // `error`.** A macOS bundle still carrying `com.apple.quarantine` has its
    // unsigned Core SIGKILLed by Gatekeeper the instant it executes —
    // measured on the real CI artifact, exit 137 with no output — and without
    // this the Client said only "no Core is listening", which names the
    // symptom and hides a cause the Operator can fix in thirty seconds.
    child.on("exit", (code, signal) => {
      const verdict = classifyCoreExit({
        code,
        signal,
        msSinceSpawn: Date.now() - spawnedAt,
        platform: process.platform,
      });
      if (verdict.retry) startAttempted = false;
      if (verdict.key !== null) startFailure = verdict.key;
    });
    // A binary that cannot be executed reports ENOENT asynchronously rather
    // than throwing, so the `catch` below never sees it. Unhandled, that
    // becomes an uncaught exception in the main process and Electron
    // replaces the whole Client with a crash dialog — failing to find the
    // Core would take the window down with it.
    child.on("error", (error: Error) => {
      startAttempted = false;
      startFailure = `could not start the Core at ${binary}: ${error.message}`;
    });
    child.unref();
  } catch (error) {
    startAttempted = false;
    startFailure = `could not start the Core at ${binary}: ${String(error)}`;
  }
}

async function connectWithRetry(): Promise<CoreClient> {
  try {
    return await CoreClient.connect();
  } catch (first) {
    startCore();
    // Give the daemon a moment to bind its socket, then try again.
    for (let attempt = 0; attempt < 10; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 200));
      try {
        return await CoreClient.connect();
      } catch {
        continue;
      }
    }
    // Say why the Core is not there, when the reason is known. "No Core is
    // listening" is true and useless if the reason is that the binary was
    // never found — or that macOS killed it for being quarantined.
    //
    // Reached only after every connection attempt failed, which is what makes
    // it safe for `classifyCoreExit` to produce a key for any non-zero exit:
    // opening the Client twice exits 1, and the connection to the Core that
    // already holds the socket succeeds long before this line.
    if (startFailure !== null) throw new Error(startFailure);
    throw first;
  }
}

async function coreClient(): Promise<CoreClient> {
  if (client) return client;
  if (!connecting) {
    connecting = (async () => {
      const connected = await connectWithRetry();
      await connected.initialize("evertranscript-client", app.getVersion());
      connected.onNotification((notification) => {
        window?.webContents.send("core:notification", notification);
      });
      client = connected;
      return connected;
    })().catch((error) => {
      // Let the next request try again rather than caching the failure.
      connecting = null;
      throw error;
    });
  }
  return connecting;
}

function createWindow(): void {
  window = new BrowserWindow({
    width: 1100,
    height: 760,
    title: "EverTranscript",
    // Read on Windows and Linux; macOS takes the Dock icon from the bundle,
    // or from `app.dock.setIcon` below while there is no bundle.
    icon: process.platform === "win32" ? ICON_ICO : ICON_PNG,
    webPreferences: {
      preload: join(__dirname, "../preload/index.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  const devServer = process.env.VITE_DEV_SERVER_URL;
  if (devServer) {
    void window.loadURL(devServer);
  } else {
    void window.loadFile(join(__dirname, "../renderer/index.html"));
  }

  window.on("closed", () => {
    window = null;
  });
}

ipcMain.handle("core:status", async () => {
  const connected = await coreClient();
  return connected.status();
});

ipcMain.handle("core:request", async (_event, method: string, params?: unknown) => {
  const connected = await coreClient();
  return connected.request(method, params);
});

// A dropped connection must not leave the Client permanently broken: forget
// it so the next request reconnects (and restarts the Core if needed).
function forgetClient(): void {
  client?.close();
  client = null;
  connecting = null;
  startAttempted = false;
}


/**
 * Starts update checks only if the Operator has them on.
 *
 * The setting is read from the Core rather than kept here, because it is
 * the same switch the trust surface shows and the Core's own check reads —
 * two sources for one switch is how a switch ends up meaning different
 * things in two places. A Core that is not up yet simply means no check
 * this launch, which is the safe direction: the failure mode of asking
 * later is a missed update, and of assuming yes is traffic the Operator
 * turned off.
 */
async function maybeCheckForUpdates(): Promise<void> {
  // Unpackaged builds have no update to install and no signature to check.
  if (!app.isPackaged) return;
  try {
    const core = await connectWithRetry();
    const settings = (await core.request("settings/get", {})) as {
      checkForUpdates?: boolean;
    };
    startUpdateChecks(settings.checkForUpdates === true);
  } catch {
    // No Core, no check.
  }
}

ipcMain.handle("updates:download", async () => {
  await downloadUpdate();
});

ipcMain.handle("updates:install", () => {
  installUpdate();
});

void app.whenReady().then(() => {
  // An unpackaged Client has no bundle for the Dock to read an icon from.
  if (!app.isPackaged && app.dock) app.dock.setIcon(ICON_PNG);
  createWindow();
  void maybeCheckForUpdates();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  // Quitting the Client must never stop the Core: it is a separate process
  // and keeps recording. On macOS the app stays resident as usual.
  forgetClient();
  if (process.platform !== "darwin") app.quit();
});
