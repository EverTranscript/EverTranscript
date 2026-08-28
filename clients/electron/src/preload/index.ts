/**
 * The preload bridge: the only surface the renderer can reach.
 *
 * Deliberately narrow — the renderer gets protocol calls and notifications,
 * never a socket, a file path, or the record itself.
 */

import { contextBridge, ipcRenderer } from "electron";

import type { JsonRpcNotification } from "@protocol/JsonRpcNotification";
import type { StatusResponse } from "@protocol/StatusResponse";

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
};

contextBridge.exposeInMainWorld("evertranscript", api);

export type EverTranscriptApi = typeof api;
