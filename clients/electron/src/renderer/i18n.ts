/**
 * String catalog.
 *
 * Externalized from the first commit on purpose: retrofitting extraction
 * after the UI exists is the classic regret, and v1 ships English and
 * Simplified Chinese together. A real i18n runtime (lingui-style) replaces
 * this shim in M5; the call sites do not change.
 */

const en = {
  "core.connecting": "Connecting to the Core…",
  "core.unreachable.title": "The Core isn't running",
  "state.idle": "Idle",
  "state.recording": "Recording",
  "field.version": "Version",
  "field.uptime": "Uptime",
  "field.history": "History folder",
} as const;

export type MessageKey = keyof typeof en;

const catalogs: Record<string, Partial<Record<MessageKey, string>>> = {
  en,
  "zh-CN": {
    "core.connecting": "正在连接 Core…",
    "core.unreachable.title": "Core 未运行",
    "state.idle": "空闲",
    "state.recording": "录制中",
    "field.version": "版本",
    "field.uptime": "运行时长",
    "field.history": "History 文件夹",
  },
};

function activeLocale(): string {
  const requested =
    typeof navigator !== "undefined" ? navigator.language : "en";
  return requested in catalogs ? requested : "en";
}

export function t(key: MessageKey): string {
  return catalogs[activeLocale()]?.[key] ?? en[key];
}
