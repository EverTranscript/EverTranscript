import type { EverTranscriptApi } from "../preload/index";

declare global {
  interface Window {
    evertranscript: EverTranscriptApi;
  }
}

export {};
