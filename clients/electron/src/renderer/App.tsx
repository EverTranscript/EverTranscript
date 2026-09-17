import { useEffect, useMemo, useRef, useState } from "react";

import type { AudioLegReport } from "@protocol/AudioLegReport";
import type { Meeting } from "@protocol/Meeting";
import type { TranscriptSegment } from "@protocol/TranscriptSegment";

import { isMessageKey, plural, t } from "./i18n";
import { parseSpans, parseSummary } from "./summary-markdown";
import { rerunLines } from "./rerun-progress";
import type { Speaker } from "@protocol/Speaker";
import type { SpeakerMeeting } from "@protocol/SpeakerMeeting";
import type { SpeakerJoinPreview } from "@protocol/SpeakerJoinPreview";
import type { DiarizeRerun } from "@protocol/DiarizeRerun";
import {
  useAudioCheck,
  useCalendarAccess,
  useCore,
  useMeetingWriting,
  useRegistry,
  useSettings,
  useBriefing,
  useModelDownload,
  usePosture,
  useSummaryBackends,
  useTranscript,
} from "./useCore";

export function App(): React.JSX.Element {
  const core = useCore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showingSettings, setShowingSettings] = useState(false);
  const [showingRegistry, setShowingRegistry] = useState(false);
  const [showingPosture, setShowingPosture] = useState(false);
  const [showingOnboarding, setShowingOnboarding] = useState(false);
  const { briefing } = useBriefing();

  const recording = useMemo(
    () => core.meetings.find((meeting) => !meeting.endedAt) ?? null,
    [core.meetings],
  );
  // Follow the live meeting unless the Operator has picked another one.
  const activeId = selectedId ?? recording?.id ?? core.meetings[0]?.id ?? null;
  const active = core.meetings.find((meeting) => meeting.id === activeId) ?? null;
  const isLive = active !== null && active.id === recording?.id;

  // One set of handlers behind the toolbar and the menu bar alike, so a
  // command cannot come to mean one thing in one and another in the other.
  const commands = {
    record: () => void core.startRecording(),
    stop: () => void core.stopRecording(),
    settings: () => {
      setShowingRegistry(false);
      setShowingSettings((open) => !open);
    },
    registry: () => {
      setShowingSettings(false);
      setShowingPosture(false);
      setShowingRegistry((open) => !open);
    },
    posture: () => {
      setShowingSettings(false);
      setShowingRegistry(false);
      setShowingPosture((open) => !open);
    },
    // Asked for from Settings, which on a Mac is a window of its own.
    onboarding: () => {
      setShowingSettings(false);
      setShowingOnboarding(true);
    },
  };
  const latestCommands = useRef(commands);
  latestCommands.current = commands;
  useEffect(
    () =>
      window.evertranscript.onMenuCommand((command) =>
        latestCommands.current[command as keyof typeof commands]?.(),
      ),
    [],
  );

  // The menu bar is where a Mac looks for every command a window offers,
  // so the toolbar's are there too, in the catalog's words. They are
  // offered only once the window is showing the toolbar they mirror.
  const available =
    !core.error && Boolean(core.status) && Boolean(briefing?.acknowledged) && !showingOnboarding;
  const isRecording = recording !== null;
  useEffect(() => {
    window.evertranscript.updateMenu({
      available,
      recording: isRecording,
      labels: {
        about: t("menu.about"),
        services: t("menu.services"),
        hide: t("menu.hide"),
        hideOthers: t("menu.hideOthers"),
        showAll: t("menu.showAll"),
        quit: t("menu.quit"),
        file: t("menu.file"),
        close: t("menu.close"),
        edit: t("menu.edit"),
        undo: t("menu.undo"),
        redo: t("menu.redo"),
        cut: t("menu.cut"),
        copy: t("menu.copy"),
        paste: t("menu.paste"),
        pasteAndMatchStyle: t("menu.pasteAndMatchStyle"),
        delete: t("menu.delete"),
        selectAll: t("menu.selectAll"),
        substitutions: t("menu.substitutions"),
        showSubstitutions: t("menu.showSubstitutions"),
        smartQuotes: t("menu.smartQuotes"),
        smartDashes: t("menu.smartDashes"),
        textReplacement: t("menu.textReplacement"),
        speech: t("menu.speech"),
        startSpeaking: t("menu.startSpeaking"),
        stopSpeaking: t("menu.stopSpeaking"),
        view: t("menu.view"),
        window: t("menu.window"),
        minimize: t("menu.minimize"),
        zoom: t("menu.zoom"),
        front: t("menu.front"),
        settings: t("settings.open"),
        record: t("action.record"),
        stop: t("action.stop"),
        registry: t("registry.open"),
        posture: t("posture.open"),
        enterFullScreen: t("menu.enterFullScreen"),
        exitFullScreen: t("menu.exitFullScreen"),
      },
    });
  }, [available, isRecording]);

  if (core.error) {
    return (
      <Frame>
        <CoreUnreachable message={core.error} onRetry={core.refresh} />
      </Frame>
    );
  }
  if (!core.status) {
    return (
      <Frame>
        <div className="grid h-full place-items-center bg-surface text-ink-muted">
          {t("core.connecting")}
        </div>
      </Frame>
    );
  }

  // Setup takes the whole window until the Briefing is acknowledged.
  // Nothing is captured before that (ADR-0023), so a sidebar offering a
  // Record button would be offering an action that will be refused.
  if (!briefing?.acknowledged || showingOnboarding) {
    return (
      <Frame>
        <Onboarding onDone={() => setShowingOnboarding(false)} />
      </Frame>
    );
  }

  return (
    <div className="grid h-full grid-cols-[280px_1fr]">
      <Sidebar
        meetings={core.meetings}
        activeId={activeId}
        recordingId={recording?.id ?? null}
        onSelect={setSelectedId}
        onRecord={commands.record}
        onStop={commands.stop}
      />
      <main className="flex h-full min-w-0 flex-col overflow-hidden bg-surface">
        {/* The window's toolbar, in the strip a title bar would have taken —
            where a Mac app keeps its toolbar, and where Windows puts
            commands in a tall title bar. It stops short of the caption
            buttons Windows draws over its far end. */}
        <div
          className="titlebar toolbar"
          style={{ paddingInlineEnd: "calc(var(--titlebar-inset-end) + 9px)" }}
        >
          <button
            type="button"
            onClick={commands.registry}
            title={t("registry.title")}
            className="tool"
          >
            {t("registry.open")}
          </button>
          <button
            type="button"
            onClick={commands.posture}
            title={t("posture.title")}
            className="tool"
          >
            {t("posture.open")}
          </button>
          {/* A Mac keeps Settings in the App menu, under ⌘, — not in the
              toolbar, where it would take room from what is used every day.
              Windows has no menu bar to keep it in. */}
          {document.documentElement.dataset.platform !== "darwin" && (
            <button type="button" onClick={commands.settings} className="tool">
              {t("settings.open")}
            </button>
          )}
        </div>
        {showingSettings ? (
          <SettingsPanel
            onClose={() => setShowingSettings(false)}
            onRerunSetup={commands.onboarding}
          />
        ) : showingRegistry ? (
          <RegistryPanel
            onClose={() => setShowingRegistry(false)}
            onOpenMeeting={(id) => {
              setSelectedId(id);
              setShowingRegistry(false);
            }}
          />
        ) : showingPosture ? (
          <PosturePanel onClose={() => setShowingPosture(false)} />
        ) : active ? (
          <MeetingView
            meeting={active}
            live={isLive}
            onRetitle={(title) => void core.retitle(active.id, title)}
            onDelete={() => {
              void core.remove(active.id);
              setSelectedId(null);
            }}
          />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}

/**
 * The window edge, for the states that fill the window without a toolbar.
 *
 * There is no title bar: macOS keeps its traffic lights at the left of this
 * strip, Windows draws its caption buttons over the right of it, and both
 * expect the app to leave the room and to accept a drag there. The main
 * window puts its toolbar in the same strip instead.
 */
function Frame({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="relative h-full pt-[var(--titlebar-height)]">
      <div className="titlebar absolute inset-x-0 top-0 h-[var(--titlebar-height)]" />
      {children}
    </div>
  );
}

function CoreUnreachable({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}): React.JSX.Element {
  return (
    <div className="grid h-full place-items-center bg-surface p-8">
      <div className="max-w-md text-center">
        <h1 className="font-display text-xl font-semibold text-recording">
          {t("core.unreachable.title")}
        </h1>
        {/* The recording is not lost when this window cannot reach the Core —
            the Core is a separate process and keeps going (ADR-0026). */}
        <p className="mt-2 text-sm text-ink-muted">
          {t("core.unreachable.hint")}
        </p>
        {/* The Core reports why by catalog key where it has one to give,
            so the sentence the Operator reads is translated rather than
            whatever English the main process happened to build. Anything
            else — an OS error, a socket path — is shown as it came. */}
        <pre className="mt-4 overflow-x-auto rounded bg-surface-raised p-3 text-left text-xs text-ink-muted">
          {isMessageKey(message) ? t(message) : message}
        </pre>
        <button
          type="button"
          onClick={onRetry}
          className="push-default mt-4"
        >
          {t("core.retry")}
        </button>
      </div>
    </div>
  );
}

function Sidebar({
  meetings,
  activeId,
  recordingId,
  onSelect,
  onRecord,
  onStop,
}: {
  meetings: Meeting[];
  activeId: string | null;
  recordingId: string | null;
  onSelect: (id: string) => void;
  onRecord: () => void;
  onStop: () => void;
}): React.JSX.Element {
  // The list's one Tab stop is the selected row, or the first row when the
  // selection is a Meeting the list does not show — deleted elsewhere, or
  // older than the page of Meetings it holds.
  const tabStop = meetings.some((meeting) => meeting.id === activeId)
    ? activeId
    : meetings[0]?.id;
  return (
    <aside className="flex h-full flex-col border-r border-line">
      {/* The sidebar's share of the toolbar strip. The traffic lights take
          its leading end on macOS, so Record goes at the other. */}
      <div className="titlebar toolbar">
        {recordingId ? (
          <button type="button" onClick={onStop} className="tool">
            <span aria-hidden className="size-2 bg-recording" />
            {t("action.stop")}
          </button>
        ) : (
          <button type="button" onClick={onRecord} className="tool">
            <span aria-hidden className="size-2 rounded-full bg-recording" />
            {t("action.record")}
          </button>
        )}
      </div>

      {/* One stop for Tab, with the arrow keys moving the selection and the
          keyboard with it — how a list behaves on both platforms, and what
          lets the highlight double as the focus indicator. */}
      <ul
        className="min-h-0 flex-1 overflow-y-auto pb-2.5"
        onKeyDown={(event) => {
          if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
          const index = meetings.findIndex((meeting) => meeting.id === activeId);
          const next = index + (event.key === "ArrowDown" ? 1 : -1);
          const target = meetings[next];
          if (!target) return;
          event.preventDefault();
          onSelect(target.id);
          event.currentTarget.children[next]?.querySelector("button")?.focus();
        }}
      >
        {meetings.length === 0 ? (
          <li className="px-4 py-6 text-sm text-ink-muted">
            {t("meetings.empty")}
            <span className="mt-1 block text-xs">{t("meetings.emptyHint")}</span>
          </li>
        ) : (
          meetings.map((meeting) => (
            <li key={meeting.id}>
              <button
                type="button"
                onClick={() => onSelect(meeting.id)}
                aria-current={meeting.id === activeId ? "true" : undefined}
                tabIndex={meeting.id === tabStop ? 0 : -1}
                className="row"
              >
                <span className="block truncate text-sm">
                  {displayTitle(meeting)}
                </span>
                <span className="mt-0.5 flex items-center gap-1.5 text-xs text-ink-muted">
                  {meeting.id === recordingId ? (
                    <>
                      <span className="inline-block size-1.5 rounded-full bg-recording" />
                      {t("meeting.recordingNow")}
                    </>
                  ) : (
                    formatStarted(meeting.startedAt)
                  )}
                </span>
              </button>
            </li>
          ))
        )}
      </ul>
    </aside>
  );
}

function MeetingView({
  meeting,
  live,
  onRetitle,
  onDelete,
}: {
  meeting: Meeting;
  live: boolean;
  onRetitle: (title: string) => void;
  onDelete: () => void;
}): React.JSX.Element {
  const { segments, dropped } = useTranscript(meeting.id, live);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  return (
    <>
      <header className="border-b border-line px-6 py-4">
        {editing ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (draft.trim()) onRetitle(draft.trim());
              setEditing(false);
            }}
            className="flex items-center gap-2"
          >
            <input
              autoFocus
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              className="field min-w-0 flex-1 font-display text-xl"
            />
            <button type="submit" className="push-default">
              {t("action.save")}
            </button>
            <button
              type="button"
              onClick={() => setEditing(false)}
              className="push"
            >
              {t("action.cancel")}
            </button>
          </form>
        ) : (
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h1 className="truncate font-display text-xl font-semibold">
                {displayTitle(meeting)}
              </h1>
              <p className="mt-0.5 text-xs text-ink-muted">
                {formatStarted(meeting.startedAt)}
                {meeting.durationSeconds !== undefined
                  ? ` · ${formatDuration(meeting.durationSeconds)}`
                  : ""}
              </p>
            </div>
            <div className="flex shrink-0 gap-2">
              <button
                type="button"
                onClick={() => {
                  setDraft(meeting.title ?? "");
                  setEditing(true);
                }}
                className="push"
              >
                {t("action.rename")}
              </button>
              <button
                type="button"
                onClick={() => {
                  // Deleting removes the audio too and cannot be undone, so
                  // it is asked the way the OS asks it: a short question, the
                  // act named on its own button, and Escape as the way out.
                  void window.evertranscript
                    .confirm({
                      message: t("meeting.deleteConfirm"),
                      detail: t("meeting.deleteConfirm.detail"),
                      action: t("action.delete"),
                      cancel: t("action.cancel"),
                    })
                    .then((confirmed) => confirmed && onDelete());
                }}
                className="push text-recording"
              >
                {t("action.delete")}
              </button>
            </div>
          </div>
        )}
      </header>

      {/* Above the transcript, because the transcript is only as complete
          as the capture was. Without this, a meeting that recorded one side
          of a conversation looks like one where nobody else spoke. */}
      {meeting.audioNotes && meeting.audioNotes.length > 0 ? (
        <div className="border-b border-line bg-surface-raised px-6 py-2 text-xs">
          <p className="font-medium text-recording">
            {t("meeting.incomplete")}
          </p>
          <ul className="mt-1 list-disc pl-4 text-ink-muted">
            {meeting.audioNotes.map((note) => (
              <li key={note}>{note}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {dropped > 0 ? (
        <p className="border-b border-line bg-surface-raised px-6 py-2 text-xs text-ink-muted">
          {t("transcript.dropped")}
        </p>
      ) : null}

      <section className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
        {segments.length === 0 ? (
          <p className="text-sm text-ink-muted">
            {live ? t("transcript.listening") : t("transcript.empty")}
          </p>
        ) : (
          <ol className="max-w-[72ch] space-y-3 text-lg">
            {segments.map((segment) => (
              <Segment key={segment.id} segment={segment} />
            ))}
          </ol>
        )}
      </section>

      <WritingPanel meetingId={meeting.id} />
    </>
  );
}

function Segment({ segment }: { segment: TranscriptSegment }): React.JSX.Element {
  // Until Diarization lands (M3) the channel is the attribution we honestly
  // have: the mic leg is where the Operator is (ADR-0029 as amended).
  const speaker =
    segment.channel === "mic" ? t("speaker.you") : t("speaker.participants");
  return (
    <li className="grid grid-cols-[auto_1fr] items-baseline gap-3">
      <span className="font-mono text-xs text-ink-muted tabular-nums">
        {formatTimestamp(segment.startMs)}
      </span>
      <span>
        <span className="mr-2 font-medium">{speaker}</span>
        {segment.text}
      </span>
    </li>
  );
}

function EmptyState(): React.JSX.Element {
  return (
    <div className="grid h-full place-content-center justify-items-center gap-3 text-sm text-ink-muted">
      <Mark />
      {t("meeting.selectPrompt")}
    </div>
  );
}

/**
 * The seahorse, drawn from `brand/src/mark.svg`.
 *
 * Once, in the one place with nothing else to look at: everywhere else the
 * window belongs to the record and to the OS around it.
 */
function Mark(): React.JSX.Element {
  return (
    <svg
      aria-hidden
      viewBox="0 0 256 256"
      className="size-16 opacity-50"
      fill="none"
      stroke="currentColor"
      strokeWidth={17}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M116,54 C104,56 92,62 82,74 C66,92 56,112 56,134 C56,162 60,187 72,202 C84,217 105,222 123,218 C142,214 156,205 159,191 C163,176 158,163 146,158 C136,154 125,158 122,168 C121,173 122,178 125,181" />
      <path d="M116,54 C133,56 149,63 159,74 C166,81 169,87 170,93 C178,95 191,98 197,103 C202,108 202,115 197,119 C190,124 120,121 96,121 C84,121 78,132 74,150" />
      <circle cx="135" cy="80" r="8" fill="currentColor" stroke="none" />
      <circle cx="116" cy="39" r="9.5" fill="currentColor" stroke="none" />
    </svg>
  );
}

function displayTitle(
  meeting: Pick<Meeting, "title" | "startedAt" | "detectedApp">,
): string {
  if (meeting.title && meeting.title.trim()) return meeting.title;
  const date = meeting.startedAt.slice(0, 10);
  return meeting.detectedApp
    ? `${meeting.detectedApp}, ${date}`
    : `${t("meeting.untitled")}, ${date}`;
}

function formatStarted(startedAt: string): string {
  const parsed = new Date(startedAt);
  return Number.isNaN(parsed.getTime())
    ? startedAt
    : parsed.toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      });
}

function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m`;
  return `${seconds}s`;
}

function formatTimestamp(milliseconds: number): string {
  const total = Math.floor(Math.max(0, milliseconds) / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number) => String(value).padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${pad(minutes)}:${pad(seconds)}`;
}

/**
 * Settings in a window of their own, which is where a Mac keeps them
 * (`settings.md › macOS`). The window's title names them, a change applies
 * as it is made, and ⌘W closes it — so the pane drops its heading and its
 * Done button here.
 */
export function SettingsWindow(): React.JSX.Element {
  return (
    <div className="h-full bg-surface">
      <SettingsPanel onRerunSetup={() => window.evertranscript.rerunSetup()} />
    </div>
  );
}

/**
 * Settings: the single Auto-Record switch, and the Watchlist it governs.
 *
 * The Client had no settings surface until now, so this is also where the
 * settings only the CLI could reach come to live — a settings screen that
 * hides settings is worse than none.
 */
function SettingsPanel({
  onClose,
  onRerunSetup,
}: {
  /** Absent in the Mac's Settings window, which closes the way windows do. */
  onClose?: () => void;
  onRerunSetup: () => void;
}): React.JSX.Element {
  const { settings, watchlist, error, update, addWatched, removeWatched } =
    useSettings();
  const [draft, setDraft] = useState("");

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      {onClose ? (
        <header className="mb-6 flex items-center justify-between">
          <h1 className="font-display text-xl font-semibold">{t("settings.title")}</h1>
          <button
            type="button"
            onClick={onClose}
            className="push"
          >
            {t("settings.close")}
          </button>
        </header>
      ) : null}

      {error ? (
        <p className="mb-4 text-sm text-recording">{error}</p>
      ) : null}

      <section className="mb-8">
        <label className="flex items-start gap-3">
          <input
            type="checkbox"
            className="mt-1"
            checked={settings?.autoRecord ?? false}
            onChange={(changed) =>
              void update({ autoRecord: changed.target.checked })
            }
          />
          <span>
            <span className="block text-sm font-medium">
              {t("settings.autoRecord")}
            </span>
            <span className="block text-xs text-ink-muted">
              {t("settings.autoRecord.hint")}
            </span>
          </span>
        </label>
      </section>

      <section className="mb-8">
        <span className="block text-sm font-medium">
          {t("settings.chineseScript")}
        </span>
        <span className="mb-2 block text-xs text-ink-muted">
          {t("settings.chineseScript.hint")}
        </span>
        <select
          value={settings?.chineseScript ?? "simplified"}
          onChange={(changed) =>
            void update({
              chineseScript: changed.target.value as "simplified" | "traditional",
            })
          }
          className="push"
        >
          <option value="simplified">
            {t("settings.chineseScript.simplified")}
          </option>
          <option value="traditional">
            {t("settings.chineseScript.traditional")}
          </option>
        </select>
      </section>

      <section>
        <h2 className="text-sm font-medium">{t("watchlist.title")}</h2>
        <p className="mb-3 text-xs text-ink-muted">
          {t("watchlist.hint")}
        </p>

        {watchlist && watchlist.entries.length === 0 ? (
          <p className="mb-3 text-xs text-recording">
            {t("watchlist.empty")}
          </p>
        ) : null}

        <ul className="mb-4">
          {watchlist?.entries.map((entry) => (
            <li
              key={entry.id}
              className="flex items-center justify-between border-b border-line py-2"
            >
              <span>
                <span className="block text-sm">{entry.name}</span>
                <span className="block font-mono text-xs text-ink-muted">
                  {entry.kind === "browserMeetings"
                    ? t("watchlist.browserMeetings")
                    : entry.id}
                </span>
              </span>
              <button
                type="button"
                onClick={() => void removeWatched(entry.id)}
                className="push"
              >
                {t("watchlist.remove")}
              </button>
            </li>
          ))}
        </ul>

        <form
          className="mb-4 flex gap-2"
          onSubmit={(submitted) => {
            submitted.preventDefault();
            if (!draft.trim()) return;
            void addWatched(draft.trim());
            setDraft("");
          }}
        >
          <input
            value={draft}
            onChange={(changed) => setDraft(changed.target.value)}
            placeholder={t("watchlist.addPlaceholder")}
            className="field min-w-0 flex-1"
          />
          <button
            type="submit"
            className="push"
          >
            {t("watchlist.add")}
          </button>
        </form>

        {watchlist && watchlist.suggestions.length > 0 ? (
          <>
            <h3 className="text-xs font-medium text-ink-muted">
              {t("watchlist.suggested")}
            </h3>
            <ul>
              {watchlist.suggestions.map((entry) => (
                <li key={entry.id} className="flex items-center justify-between py-2">
                  <span className="text-sm text-ink-muted">
                    {entry.name}
                  </span>
                  <button
                    type="button"
                    onClick={() => void addWatched(entry.id)}
                    className="push"
                  >
                    {t("watchlist.add")}
                  </button>
                </li>
              ))}
            </ul>
          </>
        ) : null}
      </section>

      <BackendPanel />

      <section className="mt-8">
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings?.checkForUpdates ?? true}
            onChange={(changed) =>
              void update({ checkForUpdates: changed.target.checked })
            }
          />
          <span>
            {t("updates.title")}
            <span className="mt-0.5 block text-xs text-ink-muted">
              {t("updates.hint")}
            </span>
          </span>
        </label>

        {/* An Operator who skipped a step needs a way back that is not
            reinstalling. */}
        <button
          type="button"
          onClick={onRerunSetup}
          className="push mt-4"
        >
          {t("onboarding.reopen")}
        </button>
      </section>
    </div>
  );
}

/**
 * What the bulk re-run is doing, drawn only while there is one.
 *
 * A multi-hour background job that reprocesses every meeting is, from
 * outside, indistinguishable from the product misbehaving — so it says so,
 * in the screen that describes what the installation holds.
 *
 * **It reports and it stops; it does not start anything.** There is no
 * begin here and no endpoint for one: a re-run is what a model change
 * causes, not a button.
 */
function RerunProgress({
  rerun,
  stopping,
  onStop,
}: {
  rerun: DiarizeRerun;
  stopping: boolean;
  onStop: () => void;
}): React.JSX.Element {
  const { state, counts, through } = rerunLines(rerun);

  return (
    <section className="mb-6 rounded border border-line p-4">
      <div className="mb-2 flex items-center justify-between gap-4">
        <h2 className="font-display text-sm font-semibold">{t("registry.rerun.title")}</h2>
        <button
          type="button"
          onClick={onStop}
          // Stop is for work still in line. Already stopped is nothing to
          // stop; a second click while the first is in flight asks again for
          // what is already happening; and an emptied line has nothing left
          // to give up, so a click there would turn a re-run that finished
          // into one the Operator is told they stopped.
          disabled={rerun.cancelled || stopping || rerun.remaining === 0}
          className="push"
        >
          {t("registry.rerun.stop")}
        </button>
      </div>

      <p className="mb-2 text-xs text-ink-muted">{t("registry.rerun.hint")}</p>

      {rerun.total > 0 ? (
        <progress
          value={through}
          max={rerun.total}
          aria-label={t("registry.rerun.progress")}
          // Labelled for what it measures: rows that have left the line,
          // walked and given up alike. "Been through" would overclaim the
          // given-up ones.
          className="mb-2 w-full"
        />
      ) : null}

      <p className="text-sm">{state}</p>

      <p className="mt-1 text-xs text-ink-muted">{counts}</p>

      <p className="mt-2 text-xs text-ink-muted">{t("registry.rerun.names")}</p>
      {/* The same sentence a row without a Voiceprint carries, so the two
          places agree word for word. */}
      <p className="mt-1 text-xs text-ink-muted">{t("registry.voiceprint.cleared")}</p>
    </section>
  );
}

/**
 * The Voice Registry (stories 30-32).
 *
 * ADR-0008 accepted storing biometric identifiers for people who never
 * consented, and named the price: the inventory must be fully inspectable
 * and each Voiceprint individually deletable. Those are acceptance criteria
 * of this milestone rather than polish — a build that clusters voices
 * without this screen has taken the exposure and skipped the controls.
 *
 * It opens without a Meeting selected, because it describes what the
 * installation holds rather than anything about one recording.
 */
function RegistryPanel({
  onClose,
  onOpenMeeting,
}: {
  onClose: () => void;
  onOpenMeeting: (meetingId: string) => void;
}): React.JSX.Element {
  const {
    speakers,
    error,
    rename,
    forgetVoice,
    meetingsFor,
    sampleFor,
    rerun,
    rerunError,
    stopping,
    stopRerun,
  } = useRegistry();
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  // A name someone else already holds. Held until the Operator answers,
  // because folding two identities together should be a decision rather
  // than a side effect of typing (ADR-0037).
  const [joining, setJoining] = useState<{
    id: string;
    name: string;
    preview: SpeakerJoinPreview;
  } | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<SpeakerMeeting[] | null>(null);
  // One clip at a time: the row whose voice is loaded, and the audio once
  // it arrives. Fetched on the click rather than with the list — a clip is
  // a decode and an encode on the Core, and a Registry of forty rows should
  // not do forty of them to draw itself.
  const [playingId, setPlayingId] = useState<string | null>(null);
  const [clip, setClip] = useState<string | null>(null);
  const [clipError, setClipError] = useState<string | null>(null);

  const togglePlay = async (id: string): Promise<void> => {
    if (playingId === id) {
      setPlayingId(null);
      setClip(null);
      return;
    }
    setPlayingId(id);
    setClip(null);
    setClipError(null);
    try {
      const loaded = await sampleFor(id);
      if (loaded === null) {
        setClipError(t("registry.sample.gone"));
        return;
      }
      setClip(loaded);
    } catch (cause) {
      setClipError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  // One row's list at a time, discarded on collapse. Caching every Speaker's
  // Meetings would mean deciding when a rename or a deletion staled them, and
  // the fetch is one indexed query against a database on this machine.
  const toggleMeetings = async (id: string): Promise<void> => {
    if (expandedId === id) {
      setExpandedId(null);
      setExpanded(null);
      return;
    }
    setExpandedId(id);
    setExpanded(null);
    setExpanded(await meetingsFor(id));
  };

  const nameOf = (speaker: Speaker): string => {
    if (speaker.displayName) return speaker.displayName;
    return speaker.isOperator ? t("registry.you") : t("registry.unnamed");
  };

  const voiceprintLabel = (speaker: Speaker): string => {
    // A name with nothing behind it reads as data loss unless the Registry
    // says why. The model that made the old vector outlives the vector for
    // exactly this sentence (ADR-0037).
    if (!speaker.hasVoiceprint) {
      // Three states wear "no Voiceprint" and mean different things: one the
      // Operator chose, one a model change caused, and one that is simply a
      // voice never enrolled. Saying so is the point of the Registry.
      if (speaker.forgotten) return t("registry.voiceprint.forgotten");
      return speaker.voiceprintModel
        ? t("registry.voiceprint.cleared")
        : t("registry.voiceprint.none");
    }
    return speaker.confirmed
      ? t("registry.voiceprint.confirmed")
      : t("registry.voiceprint.unconfirmed");
  };

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="font-display text-xl font-semibold">{t("registry.title")}</h1>
        <button
          type="button"
          onClick={onClose}
          className="push"
        >
          {t("registry.close")}
        </button>
      </header>

      <p className="mb-4 text-xs text-ink-muted">
        {t("registry.hint")}
      </p>

      {/* Absent for every installation that has never had a re-run, and
          absent is how it stays hidden — there is no flag to consult. */}
      {rerun ? (
        <RerunProgress
          rerun={rerun}
          stopping={stopping}
          onStop={() => {
            void stopRerun();
          }}
        />
      ) : null}

      {/* Outside the block, because the first read failing is exactly the
          case where there is no block: `rerun` is still null, and an error
          drawn inside one would be an error nobody ever sees. */}
      {rerunError ? (
        <p className="mb-4 text-sm text-recording">{rerunError}</p>
      ) : null}

      {error ? (
        <p className="mb-4 text-sm text-recording">{error}</p>
      ) : null}

      {speakers && speakers.speakers.length === 0 ? (
        <p className="text-xs text-ink-muted">{t("registry.empty")}</p>
      ) : null}

      <ul>
        {speakers?.speakers.map((speaker) => (
          <li
            key={speaker.id}
            className="border-b border-line py-3"
          >
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                {editingId === speaker.id ? (
                  <form
                    onSubmit={(submitted) => {
                      submitted.preventDefault();
                      const name = draft.trim();
                      setEditingId(null);
                      if (!name) return;
                      void rename(speaker.id, name).then((response) => {
                        if (response.joinRequired) {
                          setJoining({
                            id: speaker.id,
                            name,
                            preview: response.joinRequired,
                          });
                        }
                      });
                    }}
                    className="flex gap-2"
                  >
                    <input
                      autoFocus
                      value={draft}
                      onChange={(changed) => setDraft(changed.target.value)}
                      className="field min-w-0"
                    />
                    <button type="submit" className="push-default">
                      {t("action.save")}
                    </button>
                    <button
                      type="button"
                      onClick={() => setEditingId(null)}
                      className="push"
                    >
                      {t("action.cancel")}
                    </button>
                  </form>
                ) : (
                  <span className="block truncate text-sm">{nameOf(speaker)}</span>
                )}
                <span className="mt-0.5 block text-xs text-ink-muted">
                  {voiceprintLabel(speaker)} ·{" "}
                  {/* The count answers "how many"; the Operator's next
                      question is "which ones", and it is the only fact on
                      this row that still had no way to ask. */}
                  {speaker.meetingsSeenIn > 0 ? (
                    <button
                      type="button"
                      onClick={() => void toggleMeetings(speaker.id)}
                      title={t("registry.showMeetings")}
                      aria-expanded={expandedId === speaker.id}
                      className="underline decoration-dotted underline-offset-2 hover:text-ink"
                      data-testid="registry-meeting-count"
                    >
                      {speaker.meetingsSeenIn}{" "}
                      {plural("registry.meetings", speaker.meetingsSeenIn)}
                    </button>
                  ) : (
                    <>
                      {speaker.meetingsSeenIn}{" "}
                      {plural("registry.meetings", speaker.meetingsSeenIn)}
                    </>
                  )}
                </span>
                {/* Where the voice came from, said on the row itself. A
                    biometric inventory the Operator cannot trace back to a
                    recording is inspectable in name only — ADR-0008 bought
                    the storage with legibility, and "which meeting was this
                    taken from" is the first question anyone asks of it. */}
                {speaker.firstSeenAt ? (
                  <HeardIn
                    label={`${t("registry.firstSeen")} ${formatStarted(
                      speaker.firstSeenAt,
                    )} · ${displayTitle({
                      title: speaker.firstMeetingTitle,
                      detectedApp: speaker.firstMeetingApp,
                      startedAt: speaker.firstSeenAt,
                    })}`}
                    meetingId={speaker.firstMeetingId}
                    tooltip={t("registry.openMeeting")}
                    testId="registry-first-seen"
                    onOpenMeeting={onOpenMeeting}
                  />
                ) : null}
                {/* Only when it says something the line above did not: for a
                    voice heard in one Meeting the last time is the first
                    time, and two identical timestamps read as a bug. */}
                {speaker.lastHeardAt &&
                speaker.lastHeardAt !== speaker.firstSeenAt ? (
                  <HeardIn
                    label={`${t("registry.lastHeard")} ${formatStarted(
                      speaker.lastHeardAt,
                    )}`}
                    meetingId={speaker.lastMeetingId}
                    tooltip={t("registry.openLastMeeting")}
                    testId="registry-last-heard"
                    onOpenMeeting={onOpenMeeting}
                  />
                ) : null}
              </div>

              <div className="flex shrink-0 gap-2">
                {/* The voice itself. A biometric inventory that names a
                    stranger and cannot play them is inspectable only by
                    trusting the label; this is what lets the Operator check
                    a row against their own ears before naming or deleting
                    it. Absent where there is nothing to play, rather than
                    a button that fails. */}
                {speaker.hasSample ? (
                  <button
                    type="button"
                    onClick={() => void togglePlay(speaker.id)}
                    aria-pressed={playingId === speaker.id}
                    className="push"
                    data-testid="registry-play"
                  >
                    {playingId === speaker.id
                      ? t("registry.sample.hide")
                      : t("registry.sample.play")}
                  </button>
                ) : null}
                <button
                  type="button"
                  onClick={() => {
                    setDraft(speaker.displayName ?? "");
                    setEditingId(speaker.id);
                  }}
                  className="push"
                >
                  {t("registry.rename")}
                </button>
                {speaker.hasVoiceprint ? (
                  <button
                    type="button"
                    onClick={() => setConfirmingId(speaker.id)}
                    className="push"
                  >
                    {t("registry.forget")}
                  </button>
                ) : null}
              </div>
            </div>

            {playingId === speaker.id ? (
              <div className="mt-2" data-testid="registry-sample">
                {clip ? (
                  // The browser's own control, autoplaying: the Operator
                  // asked to hear it, so the second click a custom player
                  // would need is a step with no purpose.
                  <audio controls autoPlay src={clip} className="h-8 w-full" />
                ) : (
                  <p className="text-xs text-ink-muted">
                    {clipError ?? t("registry.sample.loading")}
                  </p>
                )}
              </div>
            ) : null}

            {expandedId === speaker.id ? (
              <ul className="mt-2 border-l border-line pl-3">
                {expanded === null ? (
                  <li className="py-1 text-xs text-ink-muted">
                    {t("core.connecting")}
                  </li>
                ) : (
                  expanded.map((meeting) => (
                    <li key={meeting.id}>
                      <button
                        type="button"
                        onClick={() => onOpenMeeting(meeting.id)}
                        className="flex w-full items-baseline justify-between gap-3 py-1 text-left text-xs text-ink-muted hover:text-ink"
                        data-testid="registry-meeting-entry"
                      >
                        <span className="min-w-0 truncate">
                          {displayTitle({
                            title: meeting.title,
                            detectedApp: meeting.detectedApp,
                            startedAt: meeting.startedAt,
                          })}
                        </span>
                        <span className="shrink-0">
                          {formatStarted(meeting.startedAt)}
                        </span>
                      </button>
                    </li>
                  ))
                )}
              </ul>
            ) : null}

            {editingId === speaker.id ? (
              <p className="mt-2 text-xs text-ink-muted">
                {t("registry.rename.hint")}
              </p>
            ) : null}

            {/* Said before it happens, not after: a biometric deletion is a
                legible act, and the Operator has to know it costs
                recognition and costs the record nothing. */}
            {confirmingId === speaker.id ? (
              <div className="mt-3 rounded border border-line p-3">
                <p className="mb-2 text-xs text-ink-muted">
                  {t("registry.forget.hint")}
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => {
                      void forgetVoice(speaker.id);
                      setConfirmingId(null);
                    }}
                    className="push text-recording"
                  >
                    {t("registry.forget.confirm")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmingId(null)}
                    className="push"
                  >
                    {t("action.cancel")}
                  </button>
                </div>
              </div>
            ) : null}

            {joining?.id === speaker.id ? (
              <div className="mt-3 rounded border border-line p-3">
                <p className="mb-2 text-xs text-ink-muted">
                  {joining.name} {t("registry.join.held")}{" "}
                  {joining.preview.intoMeetings}{" "}
                  {plural("registry.meetings", joining.preview.intoMeetings)}.{" "}
                  {t("registry.join.hint")} {joining.preview.fromMeetings}{" "}
                  {plural("registry.meetings", joining.preview.fromMeetings)}{" "}
                  {t("registry.join.hint.tail")}
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => {
                      void rename(joining.id, joining.name, true);
                      setJoining(null);
                    }}
                    className="push-default"
                  >
                    {t("registry.join.confirm")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setJoining(null)}
                    className="push"
                  >
                    {t("action.cancel")}
                  </button>
                </div>
              </div>
            ) : null}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * One end of a voice's history, and a way back to the recording.
 *
 * A button rather than a label when the Meeting is still here: naming the
 * recording and then making the Operator go find it in the sidebar is the
 * legibility ADR-0008 promised stopping one step short. The Meeting can be
 * gone — Voiceprints outlive the recordings they came from (ADR-0009) — and
 * then this is text, because a control that leads nowhere is worse than none.
 */
function HeardIn({
  label,
  meetingId,
  tooltip,
  testId,
  onOpenMeeting,
}: {
  label: string;
  meetingId: string | undefined;
  tooltip: string;
  testId: string;
  onOpenMeeting: (meetingId: string) => void;
}): React.JSX.Element {
  if (!meetingId) {
    return (
      <span
        className="mt-0.5 block text-xs text-ink-muted"
        data-testid={testId}
      >
        {label}
      </span>
    );
  }

  return (
    <button
      type="button"
      onClick={() => onOpenMeeting(meetingId)}
      title={tooltip}
      className="mt-0.5 block max-w-full truncate text-left text-xs text-ink-muted underline decoration-dotted underline-offset-2 hover:text-ink"
      data-testid={testId}
    >
      {label}
    </button>
  );
}

/**
 * Notes and Summary for the open Meeting.
 *
 * Notes save on a debounce rather than behind a Save button: they are
 * written *during* a meeting, and a button is a thing to forget while
 * listening to somebody.
 */
function WritingPanel({ meetingId }: { meetingId: string }): React.JSX.Element {
  const { meeting, generating, error, saveNotes, generate } =
    useMeetingWriting(meetingId);
  const [draft, setDraft] = useState<string | null>(null);

  // Adopt the stored notes once, then leave the field alone — re-syncing on
  // every refresh would overwrite what someone is in the middle of typing.
  useEffect(() => {
    setDraft(null);
  }, [meetingId]);

  const notes = draft ?? meeting?.notes ?? "";

  useEffect(() => {
    if (draft === null) return;
    const timer = setTimeout(() => void saveNotes(draft), 600);
    return () => clearTimeout(timer);
  }, [draft, saveNotes]);

  return (
    <div className="border-t border-line px-6 py-4">
      {error ? (
        <p className="mb-3 text-xs text-recording">{error}</p>
      ) : null}

      <section className="mb-5">
        <h2 className="text-sm font-medium">{t("summary.title")}</h2>
        {meeting?.summary ? (
          <>
            {meeting.summaryGaps ? (
              /* Above the Summary, not below it: what follows is only as
                 complete as the run that produced it, and a partial Summary
                 must not be read as a complete one first. */
              <p className="mt-2 rounded border border-line px-2 py-1 text-xs text-ink-muted">
                <strong>{t("summary.incomplete")}</strong> {meeting.summaryGaps}
              </p>
            ) : null}
            <SummaryProse className="mt-2 max-w-[68ch] text-sm" text={meeting.summary} />
            {meeting.summaryBackend ? (
              /* Story 38: which Backend actually ran, beside the thing it
                 produced rather than buried in Settings. */
              <p className="mt-2 text-xs text-ink-muted">
                {t("summary.generatedBy")} {meeting.summaryBackend}
              </p>
            ) : null}
          </>
        ) : (
          <p className="mt-1 text-xs text-ink-muted">
            {t("summary.none")}
          </p>
        )}
        <button
          type="button"
          disabled={generating}
          onClick={() => void generate()}
          className="push mt-3"
        >
          {generating ? t("summary.generating") : t("summary.generate")}
        </button>
      </section>

      <section>
        <h2 className="text-sm font-medium">{t("notes.title")}</h2>
        <p className="mb-2 text-xs text-ink-muted">{t("notes.hint")}</p>
        <textarea
          value={notes}
          onChange={(changed) => setDraft(changed.target.value)}
          placeholder={t("notes.placeholder")}
          rows={5}
          className="field w-full"
        />
      </section>
    </div>
  );
}

/**
 * The Summary Backend picker (ADR-0013, ADR-0010).
 *
 * Two things this screen must not do: preselect, and gate. Nothing is chosen
 * until the Operator chooses, and a provider's data-handling label informs
 * without ever blocking — the product cannot verify provider-side retention,
 * so refusing on a label would be false hardness dressed as a guarantee.
 */
function BackendPanel(): React.JSX.Element {
  const { backends, error, choose, setStrict, setKey } = useSummaryBackends();
  const [confirming, setConfirming] = useState<string | null>(null);
  const [keyDraft, setKeyDraft] = useState<Record<string, string>>({});

  return (
    <section className="mt-8">
      <h2 className="text-sm font-medium">{t("backend.title")}</h2>
      <p className="mb-3 text-xs text-ink-muted">{t("backend.hint")}</p>

      {error ? (
        <p className="mb-3 text-sm text-recording">{error}</p>
      ) : null}

      {backends && !backends.chosen ? (
        <p className="mb-3 text-xs text-recording">
          {t("summary.unchosen")}
        </p>
      ) : null}

      <ul className="mb-4">
        {backends?.options.map((option) => (
          <li key={option.id} className="border-b border-line py-3">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <span className="block text-sm">
                  {option.displayName}
                  {option.id === "local" ? (
                    <span className="ml-2 rounded bg-surface-raised px-1.5 py-0.5 text-xs">
                      {t("backend.recommended")}
                    </span>
                  ) : null}
                </span>
                <span className="mt-0.5 block text-xs text-ink-muted">
                  {option.leavesTheMachine
                    ? t("backend.leaves")
                    : t("backend.staysHere")}
                </span>
              </div>
              <button
                type="button"
                disabled={backends.chosen === option.id}
                onClick={() =>
                  option.leavesTheMachine && !backends.cloudWarningAccepted
                    ? setConfirming(option.id)
                    : void choose(option.id, false)
                }
                className="push shrink-0"
              >
                {backends.chosen === option.id ? "✓" : "Use"}
              </button>
            </div>

            {option.dataHandling ? (
              <p className="mt-2 text-xs text-ink-muted">
                {t("backend.trains")}: {String(option.dataHandling.trainsOnInputs)} ·{" "}
                {t("backend.retention")}: {option.dataHandling.retention} ·{" "}
                {t("backend.zeroRetention")}:{" "}
                {String(option.dataHandling.zeroRetentionAvailable)} ·{" "}
                {t("backend.verified")}:{" "}
                {/* An unverifiable label is worse than none, so an unverified
                    one says so rather than implying a check that did not
                    happen. */}
                {option.dataHandling.verifiedOn === "unverified"
                  ? t("backend.unverified")
                  : option.dataHandling.verifiedOn}
              </p>
            ) : null}

            {option.leavesTheMachine ? (
              <div className="mt-2">
                <p className="text-xs text-ink-muted">
                  {option.hasKey ? t("backend.key.stored") : t("backend.key.none")}
                </p>
                <div className="mt-1 flex gap-2">
                  <input
                    type="password"
                    value={keyDraft[option.id] ?? ""}
                    onChange={(changed) =>
                      setKeyDraft({ ...keyDraft, [option.id]: changed.target.value })
                    }
                    placeholder={t("backend.key")}
                    className="field min-w-0 flex-1"
                  />
                  <button
                    type="button"
                    onClick={() => {
                      void setKey(option.id, keyDraft[option.id] ?? "");
                      setKeyDraft({ ...keyDraft, [option.id]: "" });
                    }}
                    className="push"
                  >
                    {t("backend.key.save")}
                  </button>
                  {option.hasKey ? (
                    <button
                      type="button"
                      onClick={() => void setKey(option.id, null)}
                      className="push"
                    >
                      {t("backend.key.clear")}
                    </button>
                  ) : null}
                </div>
              </div>
            ) : null}

            {/* The hard one-time warning (story 36), stated before the act
                rather than after it. */}
            {confirming === option.id ? (
              <div className="mt-3 rounded border border-recording p-3">
                <p className="text-sm font-medium">{t("backend.warning.title")}</p>
                <p className="mt-1 text-xs text-ink-muted">
                  {t("backend.warning.body")}
                </p>
                <div className="mt-2 flex gap-2">
                  <button
                    type="button"
                    onClick={() => {
                      void choose(option.id, true);
                      setConfirming(null);
                    }}
                    className="push text-recording"
                  >
                    {t("backend.warning.accept")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirming(null)}
                    className="push"
                  >
                    {t("backend.warning.cancel")}
                  </button>
                </div>
              </div>
            ) : null}
          </li>
        ))}
      </ul>

      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          checked={backends?.strict ?? false}
          onChange={(changed) => void setStrict(changed.target.checked)}
        />
        <span>
          {t("backend.strict")}
          <span className="mt-0.5 block text-xs text-ink-muted">
            {t("backend.strict.hint")}
          </span>
        </span>
      </label>
    </section>
  );
}

/**
 * What this installation knows, holds, and may say (stories 46, 47).
 *
 * Enumeration, not assurance. Every line here is a fact with a source, and
 * the Core recomputes them on each open — a stale privacy page is a false
 * one, and this is the surface an evaluator uses to decide.
 */
function PosturePanel({ onClose }: { onClose: () => void }): React.JSX.Element {
  const { posture, error, refresh } = usePosture();

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="font-display text-xl font-semibold">{t("posture.title")}</h1>
        <button
          type="button"
          onClick={onClose}
          className="push"
        >
          {t("registry.close")}
        </button>
      </header>

      {error ? (
        <p className="mb-4 text-sm text-recording">{error}</p>
      ) : null}

      {posture ? (
        <>
          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.holds")}</h2>
            <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
              <dt className="text-ink-muted">{t("posture.meetings")}</dt>
              <dd>{posture.meetings}</dd>
              <dt className="text-ink-muted">{t("posture.speakers")}</dt>
              <dd>{posture.speakers}</dd>
              {/* The biometric count, as a number rather than a category. */}
              <dt className="text-ink-muted">
                {t("posture.voiceprints")}
              </dt>
              <dd>{posture.voiceprints}</dd>
              <dt className="text-ink-muted">{t("posture.models")}</dt>
              <dd>{posture.models.join(", ") || "—"}</dd>
              <dt className="text-ink-muted">{t("posture.folder")}</dt>
              <dd className="truncate font-mono text-xs">{posture.historyDir}</dd>
            </dl>
            <p className="mt-2 text-xs text-ink-muted">
              {posture.calendarGranted
                ? t("posture.calendar.granted")
                : t("posture.calendar.withheld")}
            </p>
            {posture.calendarGranted ? null : <CalendarAccessPanel onChange={refresh} />}
          </section>

          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.wire")}</h2>
            <p
              className={`mt-1 text-xs ${
                posture.currentlySilent ? "" : "text-recording"
              }`}
            >
              {posture.currentlySilent
                ? t("posture.silent")
                : t("posture.notSilent")}
            </p>
            <ul className="mt-2">
              {posture.traffic.map((entry) => (
                <li key={entry.name} className="border-b border-line py-2">
                  <span className="block text-sm">
                    {entry.name} —{" "}
                    <span className="text-ink-muted">
                      {entry.enabled ? t("posture.enabled") : t("posture.disabled")}
                    </span>
                  </span>
                  <span className="block font-mono text-xs text-ink-muted">
                    {entry.host}
                  </span>
                  <span className="mt-0.5 block text-xs text-ink-muted">
                    {entry.whatItSends}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.cannot")}</h2>
            <ul className="mt-2">
              {posture.foreclosed.map((claim) => (
                <li key={claim.capability} className="py-1.5">
                  <span className="block text-sm">{claim.capability}</span>
                  <span className="block text-xs text-ink-muted">
                    {claim.proof}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          {/* Shown rather than absorbed: a guarantees page reflecting only
              the current state, with no sign the promise had moved, is what
              an evaluator finds first and trusts least. */}
          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.amended")}</h2>
            <ul className="mt-2">
              {posture.amended.map((claim) => (
                <li key={claim.capability} className="py-1.5">
                  <span className="block text-sm">{claim.capability}</span>
                  <span className="block text-xs text-ink-muted">
                    {claim.proof}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          <a
            href={posture.source}
            className="text-sm underline"
            target="_blank"
            rel="noreferrer"
          >
            {t("posture.source")}
          </a>
        </>
      ) : null}
    </div>
  );
}

/**
 * Linear setup (story 44).
 *
 * Each step explains its requirement where the requirement is made, not in
 * a help page. Skippable steps say what skipping costs, in the step — and
 * the Backend step is not skippable, because ADR-0013 requires an explicit
 * choice and "decide later" is a preselection with better manners.
 */
function Onboarding({ onDone }: { onDone: () => void }): React.JSX.Element {
  const { briefing, acknowledge } = useBriefing();
  const [step, setStep] = useState(0);
  const { backends } = useSummaryBackends();

  const steps = [
    "briefing",
    "permissions",
    "models",
    "folder",
    "backend",
    "calendar",
  ] as const;
  const current = steps[step];

  // The Briefing is not a step that can be walked past, and the Backend is
  // not one that can be deferred. Everything else can.
  const blocked =
    (current === "briefing" && !briefing?.acknowledged) ||
    (current === "backend" && !backends?.chosen);

  const advance = () => (step + 1 < steps.length ? setStep(step + 1) : onDone());

  return (
    <div className="flex h-full flex-col overflow-y-auto bg-surface p-6">
      <header className="mb-4">
        <h1 className="font-display text-xl font-semibold">{t("onboarding.title")}</h1>
        <p className="text-xs text-ink-muted">
          {t("onboarding.step")} {step + 1} {t("onboarding.of")} {steps.length}
        </p>
      </header>

      <div className="min-h-0 flex-1">
        {current === "briefing" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.briefing.title")}</h2>
            <pre className="mt-2 max-h-[50vh] overflow-y-auto whitespace-pre-wrap rounded border border-line bg-surface-raised p-3 font-sans text-sm">
              {briefing?.text ?? ""}
            </pre>
            {briefing?.acknowledged ? null : (
              <button
                type="button"
                onClick={() => void acknowledge()}
                className="push mt-3"
              >
                {t("onboarding.briefing.accept")}
              </button>
            )}
          </section>
        ) : null}

        {current === "permissions" ? (
          <section>
            <h2 className="text-sm font-medium">
              {t("onboarding.permissions.title")}
            </h2>
            <p className="mt-2 text-sm text-ink-muted">
              {t("onboarding.permissions.body")}
            </p>
            <AudioCheckPanel />
          </section>
        ) : null}

        {current === "models" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.models.title")}</h2>
            <p className="mt-2 text-sm text-ink-muted">
              {t("onboarding.models.body")}
            </p>
            <ModelDownload />
          </section>
        ) : null}

        {current === "folder" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.folder.title")}</h2>
            <p className="mt-2 text-sm text-ink-muted">
              {t("onboarding.folder.body")}
            </p>
          </section>
        ) : null}

        {current === "backend" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.backend.title")}</h2>
            <p className="mt-2 text-sm text-ink-muted">
              {t("onboarding.backend.body")}
            </p>
            <BackendPanel />
          </section>
        ) : null}

        {current === "calendar" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.calendar.title")}</h2>
            <p className="mt-2 text-sm text-ink-muted">
              {t("onboarding.calendar.body")}
            </p>
            <CalendarAccessPanel />
            <p className="mt-2 text-xs text-ink-muted">
              {t("onboarding.calendar.skipCost")}
            </p>
          </section>
        ) : null}
      </div>

      <footer className="mt-4 flex gap-2">
        <button
          type="button"
          disabled={blocked}
          onClick={advance}
          className="push-default"
        >
          {step + 1 === steps.length ? t("onboarding.done") : t("onboarding.next")}
        </button>
        {!blocked && current !== "briefing" && current !== "backend" ? (
          <button
            type="button"
            onClick={advance}
            className="push"
          >
            {t("onboarding.skip")}
          </button>
        ) : null}
      </footer>
    </div>
  );
}

/**
 * What a fetch is doing, and the way to stop it.
 *
 * Silent while nothing is downloading: this is the surface for work the
 * product started on its own, and a permanent empty progress bar would be
 * its own kind of noise.
 */
/**
 * The check the permissions step describes.
 *
 * Its copy has always said "this checks by recording, not by asking" — and
 * the step rendered a paragraph and a Next button. The strings for this
 * button existed, in both locales, referenced by nothing. What that cost is
 * specific: a refused system-audio tap delivers silence and never errors, so
 * the first sign of it was a Meeting that came out empty, hours later, with
 * nothing to say why.
 *
 * Never blocking. A machine can legitimately fail this step — no microphone,
 * a permission the Operator wants to grant later — and refusing to let them
 * past a truthful "no" would be worse than the silence it replaces. The
 * verdict is information, not a gate.
 */
/**
 * The one button that makes the Calendars prompt appear (ADR-0036).
 *
 * Until the app has asked, macOS does not list it under Privacy &
 * Security, so there is nowhere else an Operator could say yes. Asking is
 * the whole job; the answer is theirs, and a refusal is changed in System
 * Settings rather than by asking again — a second ask returns the refusal
 * silently.
 */
function CalendarAccessPanel({ onChange }: { onChange?: () => void } = {}): React.ReactElement {
  const { granted, refused, asking, error, request } = useCalendarAccess();

  useEffect(() => {
    if (granted) onChange?.();
  }, [granted, onChange]);

  if (granted) {
    return (
      <p className="mt-3 text-xs text-ink-muted">{t("onboarding.calendar.granted")}</p>
    );
  }
  return (
    <div className="mt-3">
      <button
        type="button"
        disabled={asking}
        onClick={request}
        className="push"
      >
        {asking ? t("onboarding.calendar.asking") : t("onboarding.calendar.allow")}
      </button>
      {refused && !asking ? (
        <p className="mt-2 text-xs text-ink-muted">{t("onboarding.calendar.withheld")}</p>
      ) : null}
      {error ? <p className="mt-2 text-xs text-recording">{error}</p> : null}
    </div>
  );
}

function AudioCheckPanel(): React.ReactElement {
  const { report, checking, error, check } = useAudioCheck();

  const legLabel = (channel: AudioLegReport["channel"]) =>
    channel === "mic"
      ? t("onboarding.permissions.leg.mic")
      : t("onboarding.permissions.leg.system");

  return (
    <div className="mt-3">
      <button
        type="button"
        disabled={checking}
        onClick={() => check()}
        className="push"
      >
        {checking
          ? t("onboarding.permissions.checking")
          : report
            ? t("onboarding.permissions.recheck")
            : t("onboarding.permissions.check")}
      </button>

      {checking ? (
        <p className="mt-2 text-xs text-ink-muted">
          {t("onboarding.permissions.playSomething")}
        </p>
      ) : null}

      {error ? (
        <p className="mt-2 text-xs text-recording">{error}</p>
      ) : null}

      {report ? (
        <div className="mt-3 rounded border border-line px-3 py-2 text-xs">
          {report.couldNotStart ? (
            <p className="mb-2">
              {t("onboarding.permissions.couldNotStart")} {report.couldNotStart}
            </p>
          ) : null}
          <dl className="space-y-1">
            {report.legs.map((leg) => (
              <div key={leg.channel} className="flex justify-between gap-3">
                <dt className="text-ink-muted">
                  {legLabel(leg.channel)}
                </dt>
                <dd className="text-right">
                  {t(`onboarding.permissions.state.${leg.state}`)}
                  {leg.reason ? ` — ${leg.reason}` : ""}
                </dd>
              </div>
            ))}
          </dl>
          <p className="mt-2">
            {t(`onboarding.permissions.verdict.${report.verdict}`)}
          </p>
        </div>
      ) : null}
    </div>
  );
}

/**
 * The Summary, as the model wrote it.
 *
 * Summaries come back as Markdown — a heading, bold labels, and an action
 * items table — and were rendered in a `<pre>`, so the Operator read
 * `**Discussion:**` and a row of pipe characters. This is the product's
 * headline output; showing it as syntax undersells it badly.
 *
 * The parsing lives in `summary-markdown.ts` so it can be run by a test
 * rather than reasoned about; this is only the rendering. Deliberately not
 * `dangerouslySetInnerHTML`: Summary text is written by a language model, so
 * it is untrusted input, and building React elements makes injection
 * impossible by construction rather than by sanitising.
 */
function SummaryProse({
  text,
  className,
}: {
  text: string;
  className?: string;
}): React.JSX.Element {
  return (
    <div className={className}>
      {parseSummary(text).map((block, index) => {
        switch (block.kind) {
          case "heading":
            return (
              <p key={index} className="mt-3 font-semibold first:mt-0">
                <Inline text={block.text} />
              </p>
            );
          case "bullets":
            return (
              <ul key={index} className="mt-2 list-disc pl-5">
                {block.items.map((item, itemIndex) => (
                  <li key={itemIndex}>
                    <Inline text={item} />
                  </li>
                ))}
              </ul>
            );
          case "table":
            return (
              <div key={index} className="mt-2 overflow-x-auto">
                <table className="w-full border-collapse text-left text-xs">
                  {block.header ? (
                    <thead>
                      <tr>
                        {block.header.map((cell, column) => (
                          <th
                            key={column}
                            className="border-b border-line px-2 py-1 font-medium"
                          >
                            <Inline text={cell} />
                          </th>
                        ))}
                      </tr>
                    </thead>
                  ) : null}
                  <tbody>
                    {block.rows.map((row, rowIndex) => (
                      <tr key={rowIndex}>
                        {row.map((cell, column) => (
                          <td
                            key={column}
                            className="border-b border-line px-2 py-1 align-top"
                          >
                            <Inline text={cell} />
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          default:
            return (
              <p key={index} className="mt-2 first:mt-0">
                <Inline text={block.text} />
              </p>
            );
        }
      })}
    </div>
  );
}

function Inline({ text }: { text: string }): React.JSX.Element {
  return (
    <>
      {parseSpans(text).map((span, index) => {
        if (span.kind === "strong") {
          return <strong key={index}>{span.text}</strong>;
        }
        if (span.kind === "code") {
          return (
            <code key={index} className="font-mono text-xs">
              {span.text}
            </code>
          );
        }
        return <span key={index}>{span.text}</span>;
      })}
    </>
  );
}

function ModelDownload(): React.ReactElement | null {
  const { active, cancel } = useModelDownload();
  if (!active) return null;
  const percent =
    active.totalBytes > 0
      ? Math.floor((active.doneBytes / active.totalBytes) * 100)
      : 0;
  const megabytes = (bytes: number) => Math.round(bytes / 1_048_576);
  return (
    <div className="mt-3 rounded border border-line px-3 py-2 text-xs">
      <div className="flex items-center justify-between gap-3">
        <span>
          {t("models.downloading")} {active.displayName} — {percent}% (
          {megabytes(active.doneBytes)} / {megabytes(active.totalBytes)} MB)
        </span>
        <button
          type="button"
          onClick={cancel}
          className="push"
        >
          {t("models.stop")}
        </button>
      </div>
    </div>
  );
}
