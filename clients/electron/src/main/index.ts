/**
 * The Electron main process.
 *
 * It owns the connection to the Core and exposes it to the renderer through
 * a narrow IPC surface. The window is disposable: closing or crashing it
 * never touches a recording, because the recording lives in the Core
 * (ADR-0026).
 */

import { app, BrowserWindow, ipcMain } from "electron";
import { join } from "node:path";

import { CoreClient } from "./core-client.js";

let client: CoreClient | null = null;
let window: BrowserWindow | null = null;

async function coreClient(): Promise<CoreClient> {
  if (client) return client;
  const connected = await CoreClient.connect();
  await connected.initialize("evertranscript-client", app.getVersion());
  connected.onNotification((notification) => {
    window?.webContents.send("core:notification", notification);
  });
  client = connected;
  return connected;
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

void app.whenReady().then(() => {
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  // Quitting the Client must never stop the Core: it is a separate process
  // and keeps recording. On macOS the app stays resident as usual.
  client?.close();
  client = null;
  if (process.platform !== "darwin") app.quit();
});
