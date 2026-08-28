/**
 * String catalog.
 *
 * Externalized from the first commit on purpose: retrofitting extraction
 * after the UI exists is the classic regret, and v1 ships English and
 * Simplified Chinese together. A real i18n runtime (lingui-style) replaces
 * this shim in M5; the call sites do not change.
 */

const en = {
  "app.title": "EverTranscript",
  "core.connecting": "Connecting to the Core…",
  "core.unreachable.title": "The Core isn't running",
  "core.unreachable.hint": "Start it with: evertranscript daemon",
  "core.retry": "Try again",
  "state.idle": "Idle",
  "state.recording": "Recording",
  "action.record": "Record",
  "action.stop": "Stop",
  "action.rename": "Rename",
  "action.delete": "Delete",
  "action.cancel": "Cancel",
  "action.save": "Save",
  "meetings.empty": "No meetings yet",
  "meetings.emptyHint": "Press Record to capture one.",
  "meeting.untitled": "Untitled",
  "meeting.recordingNow": "Recording now",
  "meeting.selectPrompt": "Select a meeting",
  "meeting.incomplete": "This recording is incomplete",
  "meeting.deleteConfirm":
    "Delete this meeting? Its transcript, notes file, and audio are removed permanently.",
  "transcript.empty": "No transcript yet.",
  "transcript.listening": "Listening…",
  "transcript.dropped": "Some captions were dropped — this window fell behind.",
  "speaker.you": "You",
  "speaker.participants": "Participants",
  "field.version": "Version",
  "field.uptime": "Uptime",
  "field.history": "History folder",
  "field.duration": "Duration",
  "field.started": "Started",
} as const;

export type MessageKey = keyof typeof en;

const catalogs: Record<string, Partial<Record<MessageKey, string>>> = {
  en,
  "zh-CN": {
    "core.connecting": "正在连接 Core…",
    "core.unreachable.title": "Core 未运行",
    "core.unreachable.hint": "请运行：evertranscript daemon",
    "core.retry": "重试",
    "state.idle": "空闲",
    "state.recording": "录制中",
    "action.record": "录制",
    "action.stop": "停止",
    "action.rename": "重命名",
    "action.delete": "删除",
    "action.cancel": "取消",
    "action.save": "保存",
    "meetings.empty": "还没有会议",
    "meetings.emptyHint": "点击「录制」开始。",
    "meeting.untitled": "未命名",
    "meeting.recordingNow": "正在录制",
    "meeting.selectPrompt": "请选择一个会议",
    "meeting.incomplete": "这次录音不完整",
    "meeting.deleteConfirm": "确定删除这个会议？转录、笔记文件和音频都会被永久删除。",
    "transcript.empty": "暂无转录内容。",
    "transcript.listening": "正在聆听…",
    "transcript.dropped": "部分字幕已丢弃 — 此窗口处理不及时。",
    "speaker.you": "我",
    "speaker.participants": "其他与会者",
    "field.version": "版本",
    "field.uptime": "运行时长",
    "field.history": "History 文件夹",
    "field.duration": "时长",
    "field.started": "开始时间",
  },
};

function activeLocale(): string {
  const requested = typeof navigator !== "undefined" ? navigator.language : "en";
  return requested in catalogs ? requested : "en";
}

export function t(key: MessageKey): string {
  return catalogs[activeLocale()]?.[key] ?? en[key];
}
