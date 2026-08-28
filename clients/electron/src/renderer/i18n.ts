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
  "core.start.binaryMissing":
    "EverTranscript could not find the Core to start it. Put `evertranscript` on your PATH, or set EVERTRANSCRIPT_BIN to it.",
  "core.retry": "Try again",
  "settings.title": "Settings",
  "settings.open": "Settings",
  "settings.close": "Done",
  "settings.autoRecord": "Auto-Record",
  "settings.autoRecord.hint":
    "Record by itself when a watched app is in a call. Turning this off is the one act that stops it.",
  "settings.chineseScript": "Chinese script",
  "settings.chineseScript.simplified": "Simplified",
  "settings.chineseScript.traditional": "Traditional",
  "settings.chineseScript.hint":
    "Which script Mandarin is written in. The words are the same either way.",
  "watchlist.title": "Watchlist",
  "watchlist.hint": "What Meeting Detection watches. Adding an app is all it takes to watch it.",
  "watchlist.empty": "Nothing is watched. Nothing will record by itself.",
  "watchlist.browserMeetings": "any browser in a call",
  "watchlist.remove": "Remove",
  "watchlist.add": "Add",
  "watchlist.suggested": "Suggested",
  "watchlist.addPlaceholder": "Bundle id or executable name",
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
    "core.start.binaryMissing":
      "EverTranscript 找不到 Core，无法启动它。请将 `evertranscript` 加入 PATH，或将 EVERTRANSCRIPT_BIN 指向它。",
    "core.retry": "重试",
    "settings.title": "设置",
    "settings.open": "设置",
    "settings.close": "完成",
    "settings.autoRecord": "自动录制",
    "settings.autoRecord.hint": "当受监视的应用进入通话时自动录制。关闭此项即可停止该行为。",
    "settings.chineseScript": "中文字形",
    "settings.chineseScript.simplified": "简体",
    "settings.chineseScript.traditional": "繁體",
    "settings.chineseScript.hint": "中文记录使用的字形。两者内容相同。",
    "watchlist.title": "监视列表",
    "watchlist.hint": "Meeting Detection 监视的应用。加入列表即表示监视。",
    "watchlist.empty": "列表为空，不会自动录制任何内容。",
    "watchlist.browserMeetings": "任意浏览器通话",
    "watchlist.remove": "移除",
    "watchlist.add": "添加",
    "watchlist.suggested": "建议",
    "watchlist.addPlaceholder": "Bundle id 或可执行文件名",
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

/// Whether a string the Core sent is a catalog key rather than prose.
///
/// The Core reports a reason by key where it has one, so the sentence the
/// Operator reads comes from here and not from whatever English the main
/// process assembled. Everything else — an OS error, a socket path — is
/// shown as it arrived.
export function isMessageKey(value: string): value is MessageKey {
  return Object.hasOwn(en, value);
}
