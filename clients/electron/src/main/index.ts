/**
 * The Electron main process.
 *
 * It owns the connection to the Core and exposes it to the renderer through
 * a narrow IPC surface. The window is disposable: closing or crashing it
 * never touches a recording, because the recording lives in the Core
 * (ADR-0026).
 */

import { spawn } from "node:child_process";
import { app, BrowserWindow, ipcMain } from "electron";
import { join } from "node:path";

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
  const binary = process.env.EVERTRANSCRIPT_BIN ?? "evertranscript";
  try {
    const child = spawn(binary, ["daemon"], {
      detached: true,
      stdio: "ignore",
    });
    child.unref();
  } catch {
    // Reported to the renderer by the failing connect below.
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
