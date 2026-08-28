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
import { delimiter, join } from "node:path";

import { CoreClient } from "./core-client.js";

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
 * `EVERTRANSCRIPT_BIN` wins, so an unusual install can say outright. `PATH`
 * is searched by hand rather than left to `spawn`, because a GUI app
 * inherits a much smaller `PATH` than a shell — a Core installed in
 * `/opt/homebrew/bin` is invisible to an app launched from Finder — and
 * because knowing the name resolved to nothing is what lets this say so.
 * A checkout's own build is the last resort, for a Client run from source
 * before there is anything installed at all.
 *
 * Both platforms, because ADR-0025 makes Windows a gate rather than a
 * follow-up: `PATH` is `;`-separated there and the file carries `.exe`, and
 * searching by hand means neither is inherited from `spawn` any more.
 */
function coreBinary(): string | null {
  const explicit = process.env.EVERTRANSCRIPT_BIN;
  if (explicit) return explicit;
  const name = process.platform === "win32" ? "evertranscript.exe" : "evertranscript";
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, name);
    if (existsSync(candidate)) return candidate;
  }
  const repo = join(__dirname, "../../../..");
  for (const profile of ["release", "debug"]) {
    const candidate = join(repo, "target", profile, name);
    if (existsSync(candidate)) return candidate;
  }
  return null;
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
    const child = spawn(binary, ["daemon"], {
      detached: true,
      stdio: "ignore",
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
    // never found.
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

void app.whenReady().then(() => {
  createWindow();
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
