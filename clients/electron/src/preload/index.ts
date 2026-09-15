/**
 * The preload bridge: the only surface the renderer can reach.
 *
 * Deliberately narrow — the renderer gets protocol calls and notifications,
 * never a socket, a file path, or the record itself.
 */

import { contextBridge, ipcRenderer } from "electron";

import type { JsonRpcNotification } from "@protocol/JsonRpcNotification";
import type { StatusResponse } from "@protocol/StatusResponse";

/** What the menu bar needs from the page: its words, and what can be done now. */
export type MenuState = {
  available: boolean;
  recording: boolean;
  labels: Record<
    | "about"
    | "services"
    | "hide"
    | "hideOthers"
    | "showAll"
    | "quit"
    | "file"
    | "close"
    | "edit"
    | "undo"
    | "redo"
    | "cut"
    | "copy"
    | "paste"
    | "pasteAndMatchStyle"
    | "delete"
    | "selectAll"
    | "substitutions"
    | "showSubstitutions"
    | "smartQuotes"
    | "smartDashes"
    | "textReplacement"
    | "speech"
    | "startSpeaking"
    | "stopSpeaking"
    | "view"
    | "window"
    | "minimize"
    | "zoom"
    | "front"
    | "settings"
    | "record"
    | "stop"
    | "registry"
    | "posture"
    | "enterFullScreen"
    | "exitFullScreen",
    string
  >;
};

/** A destructive confirmation: the question, what it costs, and the words on its buttons. */
export type ConfirmRequest = { message: string; detail: string; action: string; cancel: string };

// A command can reach a window before its page has mounted anything to
// handle it — the main window reopened only to run setup — so it waits here
// until something has.
let commandHandler: ((command: string) => void) | null = null;
const waiting: string[] = [];
ipcRenderer.on("menu:command", (_event, command: string) => {
  if (commandHandler) commandHandler(command);
  else waiting.push(command);
});

const api = {
  status: (): Promise<StatusResponse> => ipcRenderer.invoke("core:status"),
  request: <T>(method: string, params?: unknown): Promise<T> =>
    ipcRenderer.invoke("core:request", method, params) as Promise<T>,
  onNotification: (handler: (notification: JsonRpcNotification) => void) => {
    const listener = (_event: unknown, notification: JsonRpcNotification) =>
      handler(notification);
    ipcRenderer.on("core:notification", listener);
    return () => ipcRenderer.off("core:notification", listener);
  },
  updateMenu: (state: MenuState): void => ipcRenderer.send("menu:update", state),
  onMenuCommand: (handler: (command: string) => void) => {
    commandHandler = handler;
    for (const command of waiting.splice(0)) handler(command);
    return (): void => {
      commandHandler = null;
    };
  },
  /** Settings, from the Settings window, asking the main window to run setup. */
  rerunSetup: (): void => ipcRenderer.send("setup:rerun"),
  confirm: (request: ConfirmRequest): Promise<boolean> =>
    ipcRenderer.invoke("dialog:confirm", request),
};

contextBridge.exposeInMainWorld("evertranscript", api);

export type EverTranscriptApi = typeof api;
