/**
 * The Electron main process.
 *
 * It owns the connection to the Core and exposes it to the renderer through
 * a narrow IPC surface. The window is disposable: closing or crashing it
 * never touches a recording, because the recording lives in the Core
 * (ADR-0026).
 */

import { execFile, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import {
  app,
  BrowserWindow,
  dialog,
  ipcMain,
  Menu,
  nativeTheme,
  systemPreferences,
  type MenuItemConstructorOptions,
} from "electron";
import { join } from "node:path";

import type { ConfirmRequest, MenuState } from "../preload/index";

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
 * Chromium's profile — caches, nothing the Client reads back — in a folder
 * of its own.
 *
 * Left alone, Electron puts it in `<appData>/EverTranscript`, which on macOS
 * and Windows is the Core's Application Support (ADR-0035): the two would
 * share one folder, and an instance isolated with the Core's override would
 * still write a profile into the real one. So it goes a level down, under
 * whichever folder the Core is using. Paths can only move before `ready`,
 * which is why this runs as the module loads.
 */
app.setPath(
  "userData",
  join(process.env.EVERTRANSCRIPT_APP_SUPPORT_DIR ?? join(app.getPath("appData"), app.name), "Client"),
);

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
/** The Mac's Settings window, while it is open. */
let settingsWindow: BrowserWindow | null = null;
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
      // Every window, not just the main one: Settings on a Mac is a window of
      // its own, and it shows the Core's state too.
      connected.onNotification((notification) => {
        for (const target of BrowserWindow.getAllWindows()) {
          target.webContents.send("core:notification", notification);
        }
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

const DARWIN = process.platform === "darwin";
const WINDOWS = process.platform === "win32";

/**
 * Whether this Windows can draw Mica behind the window.
 *
 * Electron sets it through the backdrop API Windows 11 22H2 (build 22621)
 * introduced — not 11 itself, which is 22000. Asking an older build for it
 * gets a window with no background at all rather than a refusal, so the
 * check has to happen here and not be left to the platform.
 */
const MICA =
  WINDOWS && Number(process.getSystemVersion().split(".")[2] ?? 0) >= 22621;

/**
 * The Windows title bar, which the caption buttons are drawn to fill.
 *
 * Fluent's tall variant, because the window's toolbar lives in this strip
 * and 48px is the height Windows gives a title bar that holds controls. The
 * renderer reads it back through `env(titlebar-area-height)`.
 */
const TITLEBAR_HEIGHT = 48;

/**
 * Hands the page the system's own colours.
 *
 * Apple is explicit that a Mac app should draw with the semantic colours
 * AppKit publishes rather than values copied out of them: they are what
 * adapt to the appearance, to Increase Contrast, and to the accent the
 * Operator chose, which is a colour this app does not get to pick. CSS has
 * keywords for a few of them and this Chromium resolves `AccentColor` to
 * nothing, so they arrive as one stylesheet, re-read whenever the system
 * says something changed. Windows publishes only the accent; there the
 * page's own palette stands in for the rest.
 */
const MAC_COLORS = {
  "--color-surface": "text-background",
  "--color-ink": "label",
  "--color-ink-muted": "secondary-label",
  "--color-line": "separator",
  "--color-selected": "selected-content-background",
  "--color-selected-unemphasized": "unemphasized-selected-content-background",
  "--color-focus": "keyboard-focus-indicator",
  "--color-placeholder": "placeholder-text",
} as const;

/** Each window's copy of the system colours, so the next copy can replace it. */
const paletteKeys = new WeakMap<BrowserWindow, string>();

/**
 * Windows' own shades of the accent, as Fluent fills a control with it.
 *
 * A checked box, a selection mark and a focused field take Dark1 in the
 * light theme and Light2 in the dark one; the raw accent is neither. (The
 * filled button keeps the default blue's shades instead, since white text
 * on a light accent's Dark1, yellow gold's among them, cannot be read.)
 * Windows works the shades out when the accent is chosen and keeps them in
 * the registry — the same values `UISettings.GetColorValue` answers with,
 * compared on a Windows 11 machine — so they are read rather than guessed
 * at. Null when there is nothing to read.
 */
function windowsAccentFill(): Promise<string | null> {
  return new Promise((resolve) => {
    execFile(
      "reg",
      ["query", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent", "/v", "AccentPalette"],
      { windowsHide: true },
      (error, stdout) => {
        // Eight RGBA swatches: Light3, Light2, Light1, the accent, Dark1,
        // Dark2, Dark3 and one more.
        const palette = /REG_BINARY\s+([0-9A-F]{64})/i.exec(stdout)?.[1];
        if (error || !palette) return resolve(null);
        const swatch = (index: number): string => `#${palette.slice(index * 8, index * 8 + 6)}`;
        resolve(`light-dark(${swatch(4)},${swatch(1)})`);
      },
    );
  });
}

async function applySystemColors(): Promise<void> {
  let css = "";
  try {
    // RRGGBBAA; the alpha is dropped because the page mixes its own.
    const accent = systemPreferences.getAccentColor();
    const fill = (WINDOWS ? await windowsAccentFill() : null) ?? (accent ? `#${accent.slice(0, 6)}` : null);
    if (fill) css += `:root{--color-accent:${fill}}`;
    if (DARWIN) {
      const palette = Object.entries(MAC_COLORS).map(
        ([property, name]) => `${property}:${systemPreferences.getColor(name)}`,
      );
      palette.push(`--color-recording:${systemPreferences.getSystemColor("red")}`);
      // AppKit resolves these in the appearance the process started in, and
      // a switch afterwards never reaches them: `NSAppearance.current` only
      // moves while a view draws, and this is not drawing. So they hold for
      // that appearance alone — which the text background gives away — and
      // the other one keeps the stylesheet's copy of AppKit's values.
      // ponytail: that copy is standard contrast with a blue-derived selection
      // until relaunch; resolving in both appearances needs a native addon.
      const dark = parseInt(systemPreferences.getColor("text-background").slice(1, 3), 16) < 0x80;
      css += `@media (prefers-color-scheme:${dark ? "dark" : "light"}){:root{${palette.join(";")}}}`;
    }
  } catch {
    // Keep whatever is already applied rather than half a palette.
    return;
  }
  if (!css) return;
  for (const target of BrowserWindow.getAllWindows()) {
    if (target.isDestroyed()) continue;
    const previous = paletteKeys.get(target);
    paletteKeys.set(target, await target.webContents.insertCSS(css));
    if (previous) await target.webContents.removeInsertedCSS(previous);
  }
}

/** The ground behind the page where no system material is drawn over it. */
function ground(): string {
  return nativeTheme.shouldUseDarkColors ? "#1F1D1B" : "#FFFFFF";
}

/**
 * The caption buttons Windows draws over the page.
 *
 * Transparent, so the Mica behind them is continuous with the rest of the
 * strip, and the glyphs take the ink colour of the appearance in force.
 */
function captionButtons(): Electron.TitleBarOverlay {
  return {
    color: "#00000000",
    symbolColor: nativeTheme.shouldUseDarkColors ? "#F5F1E8" : "#1F1D1B",
    height: TITLEBAR_HEIGHT,
  };
}

const WEB_PREFERENCES: Electron.WebPreferences = {
  preload: join(__dirname, "../preload/index.js"),
  contextIsolation: true,
  nodeIntegration: false,
  sandbox: true,
};

/** Puts the renderer in a window: the main view, or the one `view` names. */
function load(target: BrowserWindow, view?: string): void {
  const devServer = process.env.VITE_DEV_SERVER_URL;
  if (devServer) {
    void target.loadURL(view ? `${devServer}#${view}` : devServer);
  } else {
    void target.loadFile(join(__dirname, "../renderer/index.html"), { hash: view });
  }
  // A stylesheet inserted before the load does not survive it.
  target.webContents.on("did-finish-load", () => void applySystemColors());
}

function createWindow(): void {
  window = new BrowserWindow({
    width: 1100,
    height: 760,
    title: "EverTranscript",
    // Read on Windows and Linux; macOS takes the Dock icon from the bundle,
    // or from `app.dock.setIcon` below while there is no bundle.
    icon: WINDOWS ? ICON_ICO : ICON_PNG,

    // The window keeps its frame and loses its title bar, so the page runs
    // to the edge and the system's own material shows through it. What each
    // OS still owns of that strip, it keeps and places: the traffic lights
    // where AppKit puts them for a window with a unified toolbar, the caption
    // buttons on the right on Windows. The renderer leaves
    // `--titlebar-height` clear for both and drags by it.
    titleBarStyle: DARWIN || WINDOWS ? "hidden" : "default",
    ...(DARWIN
      ? {
          // Measured from an NSWindow with a unified toolbar on macOS 26:
          // 14pt lights centred in the 52pt strip, so they share a centre
          // line with the toolbar items. `hiddenInset` sits them ~8pt higher,
          // where a toolbar-less title bar would have them.
          trafficLightPosition: { x: 19, y: 19 },
          // Sidebar, not a heavier material: the Client sits open beside
          // whatever the meeting is in, and the vibrancy is what makes it
          // read as part of the desktop rather than a window on top of it.
          vibrancy: "sidebar" as const,
        }
      : {}),
    ...(WINDOWS ? { titleBarOverlay: captionButtons() } : {}),
    ...(MICA ? { backgroundMaterial: "mica" as const } : {}),
    // Transparent where a material is drawn, because the material is what
    // should be seen. Where none is, this is the ground the page sits on.
    backgroundColor: DARWIN || MICA ? "#00000000" : ground(),

    webPreferences: WEB_PREFERENCES,
  });
  load(window);

  window.on("enter-full-screen", buildMenu);
  window.on("leave-full-screen", buildMenu);

  window.on("closed", () => {
    window = null;
    // The commands act on the page, so with no page they are unavailable.
    buildMenu();
  });
}

/**
 * Settings, in the window a Mac keeps them in (`settings.md › macOS`).
 *
 * Its own window, brought back rather than duplicated, with minimize and
 * zoom dimmed: ⌘, reopens it faster than the Dock would, and there is
 * nothing more to see at a larger size. Matched against the window a SwiftUI
 * `Settings` scene opens on macOS 26 — not miniaturizable, not resizable, no
 * full screen, no toolbar for a single pane, titled "EverTranscript Settings",
 * which the page sets. The ground is AppKit's window background, the same
 * white or #1E1E1E as the text background there.
 * ponytail: one fixed-size pane that scrolls; panes with a toolbar, each
 * sized to fit, when the settings outgrow it.
 */
function openSettings(): void {
  if (settingsWindow) {
    settingsWindow.show();
    return;
  }
  const opened = new BrowserWindow({
    width: 600,
    height: 640,
    show: false,
    resizable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    backgroundColor: nativeTheme.shouldUseDarkColors ? "#1E1E1E" : "#FFFFFF",
    webPreferences: WEB_PREFERENCES,
  });
  settingsWindow = opened;
  opened.once("ready-to-show", () => opened.show());
  opened.on("closed", () => {
    settingsWindow = null;
  });
  load(opened, "settings");
}

// Setup runs in the main window, so Settings asks for it there: that window
// comes forward — reopened, if it was closed — and Settings closes, because
// setup is what the Operator turns to next.
ipcMain.on("setup:rerun", () => {
  settingsWindow?.close();
  if (!window) createWindow();
  window?.show();
  window?.webContents.send("menu:command", "onboarding");
});

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

/**
 * A destructive confirmation, asked the way the OS asks it.
 *
 * `window.confirm` gets a native alert too, but one whose buttons say OK and
 * Cancel. Apple's alert guidance wants the button to name the act, Cancel
 * never to be the default, and no caution symbol for an act whose whole
 * purpose is to remove something — so the act the Operator already chose
 * is what Return does, and Escape or ⌘. is the way out.
 */
ipcMain.handle("dialog:confirm", async (_event, request: ConfirmRequest) => {
  if (!window) return false;
  const { response } = await dialog.showMessageBox(window, {
    message: request.message,
    detail: request.detail,
    buttons: [request.action, request.cancel],
    defaultId: 0,
    cancelId: 1,
    // Windows otherwise turns any label it does not recognise as OK, Cancel
    // and the like into a command link.
    noLink: true,
  });
  return response === 0;
});

/**
 * The Mac menu bar, built from what the page says it can do.
 *
 * A Mac app lists every command in its menu bar, whether or not a toolbar
 * shows it too, and Settings is always in the app menu behind ⌘,. The page
 * owns the words — the catalog is there — and what is possible right now,
 * so it sends both and the menu is rebuilt from them. Until it does, the
 * menu is Electron's default. The system's own items are named in the
 * catalog's words too, as Apple's localizations name them, so the menu bar
 * does not switch languages halfway along. Windows has no menu bar to fill:
 * its commands are in the window.
 */
let menuState: MenuState | null = null;

ipcMain.on("menu:update", (_event, state: MenuState) => {
  menuState = state;
  buildMenu();
});

function buildMenu(): void {
  if (!DARWIN || !menuState) return;
  const { recording, labels } = menuState;
  const available = menuState.available && window !== null;
  const command = (id: string): MenuItemConstructorOptions["click"] =>
    () => window?.webContents.send("menu:command", id);
  Menu.setApplicationMenu(
    Menu.buildFromTemplate([
      {
        label: app.name,
        submenu: [
          { role: "about", label: labels.about },
          { type: "separator" },
          {
            label: `${labels.settings}…`,
            accelerator: "Command+,",
            // Settings has a window of its own, so it needs the page to be
            // past setup, not the main window to be open.
            enabled: menuState.available,
            click: () => openSettings(),
          },
          { type: "separator" },
          { role: "services", label: labels.services },
          { type: "separator" },
          { role: "hide", label: labels.hide },
          { role: "hideOthers", label: labels.hideOthers },
          { role: "unhide", label: labels.showAll },
          { type: "separator" },
          { role: "quit", label: labels.quit },
        ],
      },
      {
        label: labels.file,
        submenu: [
          {
            label: labels.record,
            accelerator: "Command+R",
            enabled: available && !recording,
            click: command("record"),
          },
          {
            // No shortcut: ⌘. is the system's Cancel, and Stop is not a
            // cancel — it keeps what was recorded.
            label: labels.stop,
            enabled: available && recording,
            click: command("stop"),
          },
          { type: "separator" },
          // "Close", not Electron's "Close Window": a window without tabs
          // (`the-menu-bar.md › File menu`).
          { role: "close", label: labels.close },
        ],
      },
      {
        label: labels.edit,
        submenu: [
          { role: "undo", label: labels.undo },
          { role: "redo", label: labels.redo },
          { type: "separator" },
          { role: "cut", label: labels.cut },
          { role: "copy", label: labels.copy },
          { role: "paste", label: labels.paste },
          { role: "pasteAndMatchStyle", label: labels.pasteAndMatchStyle },
          { role: "delete", label: labels.delete },
          { role: "selectAll", label: labels.selectAll },
          { type: "separator" },
          {
            label: labels.substitutions,
            submenu: [
              { role: "showSubstitutions", label: labels.showSubstitutions },
              { type: "separator" },
              { role: "toggleSmartQuotes", label: labels.smartQuotes },
              { role: "toggleSmartDashes", label: labels.smartDashes },
              { role: "toggleTextReplacement", label: labels.textReplacement },
            ],
          },
          {
            label: labels.speech,
            submenu: [
              { role: "startSpeaking", label: labels.startSpeaking },
              { role: "stopSpeaking", label: labels.stopSpeaking },
            ],
          },
        ],
      },
      {
        label: labels.view,
        submenu: [
          { label: labels.registry, enabled: available, click: command("registry") },
          { label: labels.posture, enabled: available, click: command("posture") },
          { type: "separator" },
          ...(app.isPackaged ? [] : [{ role: "toggleDevTools" } as const]),
          {
            role: "togglefullscreen",
            // Electron's label is "Toggle Full Screen"; a Mac names the way
            // it will go, and the window rebuilds this when it gets there.
            label: window?.isFullScreen() ? labels.exitFullScreen : labels.enterFullScreen,
          },
        ],
      },
      {
        role: "windowMenu",
        label: labels.window,
        submenu: [
          { role: "minimize", label: labels.minimize },
          { role: "zoom", label: labels.zoom },
          { type: "separator" },
          { role: "front", label: labels.front },
        ],
      },
    ]),
  );
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

  // The page follows the appearance by itself, through `light-dark()`. The
  // two things it cannot reach are the caption glyphs Windows draws over
  // it, and the ground behind it on a machine with no material — so those
  // are re-stated here, once, rather than per window.
  nativeTheme.on("updated", () => {
    if (window && WINDOWS) window.setTitleBarOverlay(captionButtons());
    if (window && !DARWIN && !MICA) window.setBackgroundColor(ground());
    // Appearance and Increase Contrast both change what AppKit hands back.
    void applySystemColors();
  });
  // The accent changes by a different road on each platform: an event on
  // Windows, a distributed notification on macOS.
  if (WINDOWS) {
    systemPreferences.on("accent-color-changed", () => void applySystemColors());
  }
  if (DARWIN) {
    systemPreferences.subscribeNotification(
      "AppleColorPreferencesChangedNotification",
      () => void applySystemColors(),
    );
  }

  createWindow();
  void maybeCheckForUpdates();
  // The Dock icon brings back the main window, even with Settings still open.
  app.on("activate", () => {
    if (!window) createWindow();
  });
});

app.on("window-all-closed", () => {
  // Quitting the Client must never stop the Core: it is a separate process
  // and keeps recording. On macOS the app stays resident as usual.
  forgetClient();
  if (process.platform !== "darwin") app.quit();
});
