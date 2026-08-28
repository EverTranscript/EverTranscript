/**
 * A Client of the Core, speaking newline-delimited JSON over the local
 * socket (ADR-0028).
 *
 * This lives in the Electron main process on purpose: the renderer never
 * touches the socket, and it never touches storage at all — the Core is the
 * record's only writer (ADR-0026).
 */

import { connect, type Socket } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";

import type { InitializeResponse } from "@protocol/InitializeResponse";
import type { JsonRpcMessage } from "@protocol/JsonRpcMessage";
import type { JsonRpcNotification } from "@protocol/JsonRpcNotification";
import type { StatusResponse } from "@protocol/StatusResponse";

/** Mirrors `evertranscript_core::paths` — keep the two in step. */
export function coreAddress(): string {
  const override = process.env.EVERTRANSCRIPT_RUNTIME_DIR;
  if (process.platform === "win32") {
    const user = process.env.USERNAME ?? "default";
    return `\\\\.\\pipe\\evertranscript-${user}`;
  }
  const runtimeDir =
    override ??
    join(homedir(), "Library", "Application Support", "EverTranscript", "run");
  return join(runtimeDir, "evertranscript.sock");
}

type Pending = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

export type NotificationHandler = (notification: JsonRpcNotification) => void;

export class CoreClient {
  #socket: Socket;
  #buffer = "";
  #nextId = 1;
  #pending = new Map<number, Pending>();
  #handlers = new Set<NotificationHandler>();
  #closed = false;

  private constructor(socket: Socket) {
    this.#socket = socket;
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => this.#onData(chunk));
    socket.on("close", () => this.#onClose());
    socket.on("error", () => this.#onClose());
  }

  static async connect(address = coreAddress()): Promise<CoreClient> {
    const socket = await new Promise<Socket>((resolve, reject) => {
      const candidate = connect(address);
      const onError = (error: Error) => {
        candidate.destroy();
        reject(
          new Error(
            `no Core is listening at ${address} — start it with \`evertranscript daemon\` (${error.message})`,
          ),
        );
      };
      candidate.once("error", onError);
      candidate.once("connect", () => {
        candidate.off("error", onError);
        resolve(candidate);
      });
    });
    return new CoreClient(socket);
  }

  /** Opens the connection. Every other request is refused before this. */
  async initialize(name: string, version: string): Promise<InitializeResponse> {
    return this.request<InitializeResponse>("initialize", {
      clientInfo: { name, version },
      capabilities: { experimentalApi: false },
    });
  }

  async status(): Promise<StatusResponse> {
    return this.request<StatusResponse>("status");
  }

  /** Subscribes to Core notifications; returns an unsubscribe function. */
  onNotification(handler: NotificationHandler): () => void {
    this.#handlers.add(handler);
    return () => this.#handlers.delete(handler);
  }

  request<T>(method: string, params?: unknown): Promise<T> {
    if (this.#closed) {
      return Promise.reject(new Error("the connection to the Core is closed"));
    }
    const id = this.#nextId++;
    const line = JSON.stringify({ id, method, params: params ?? null }) + "\n";
    return new Promise<T>((resolve, reject) => {
      this.#pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
      });
      this.#socket.write(line, (error) => {
        if (error) {
          this.#pending.delete(id);
          reject(error);
        }
      });
    });
  }

  close(): void {
    this.#socket.destroy();
    this.#onClose();
  }

  #onData(chunk: string): void {
    this.#buffer += chunk;
    let newline: number;
    while ((newline = this.#buffer.indexOf("\n")) !== -1) {
      const line = this.#buffer.slice(0, newline).trim();
      this.#buffer = this.#buffer.slice(newline + 1);
      if (line.length === 0) continue;
      this.#dispatch(line);
    }
  }

  #dispatch(line: string): void {
    let message: JsonRpcMessage;
    try {
      message = JSON.parse(line) as JsonRpcMessage;
    } catch {
      return; // an unparseable line has no id to answer
    }

    if ("id" in message && "result" in message) {
      const pending = this.#pending.get(Number(message.id));
      if (pending) {
        this.#pending.delete(Number(message.id));
        pending.resolve(message.result);
      }
      return;
    }
    if ("id" in message && "error" in message) {
      const pending = this.#pending.get(Number(message.id));
      if (pending) {
        this.#pending.delete(Number(message.id));
        pending.reject(
          new Error(`${message.error.message} (${message.error.code})`),
        );
      }
      return;
    }
    if ("method" in message && !("id" in message)) {
      for (const handler of this.#handlers) {
        handler(message as JsonRpcNotification);
      }
    }
  }

  #onClose(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const pending of this.#pending.values()) {
      pending.reject(new Error("the Core closed the connection"));
    }
    this.#pending.clear();
  }
}
