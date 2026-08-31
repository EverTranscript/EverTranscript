import { useEffect, useMemo, useState } from "react";

import type { Meeting } from "@protocol/Meeting";
import type { TranscriptSegment } from "@protocol/TranscriptSegment";

import { isMessageKey, t } from "./i18n";
import type { Speaker } from "@protocol/Speaker";
import {
  useCore,
  useMeetingWriting,
  useRegistry,
  useSettings,
  useBriefing,
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

  if (core.error) {
    return <CoreUnreachable message={core.error} onRetry={core.refresh} />;
  }
  if (!core.status) {
    return (
      <div className="grid h-full place-items-center text-[--color-ink-muted]">
        {t("core.connecting")}
      </div>
    );
  }

  // Setup takes the whole window until the Briefing is acknowledged.
  // Nothing is captured before that (ADR-0023), so a sidebar offering a
  // Record button would be offering an action that will be refused.
  if (!briefing?.acknowledged || showingOnboarding) {
    return <Onboarding onDone={() => setShowingOnboarding(false)} />;
  }

  return (
    <div className="grid h-full grid-cols-[280px_1fr]">
      <Sidebar
        meetings={core.meetings}
        activeId={activeId}
        recordingId={recording?.id ?? null}
        onSelect={setSelectedId}
        onRecord={() => void core.startRecording()}
        onStop={() => void core.stopRecording()}
        onSettings={() => {
          setShowingRegistry(false);
          setShowingSettings((open) => !open);
        }}
        onRegistry={() => {
          setShowingSettings(false);
          setShowingPosture(false);
          setShowingRegistry((open) => !open);
        }}
        onPosture={() => {
          setShowingSettings(false);
          setShowingRegistry(false);
          setShowingPosture((open) => !open);
        }}
      />
      <main className="flex h-full min-w-0 flex-col overflow-hidden">
        {showingSettings ? (
          <SettingsPanel
            onClose={() => setShowingSettings(false)}
            onRerunSetup={() => {
              setShowingSettings(false);
              setShowingOnboarding(true);
            }}
          />
        ) : showingRegistry ? (
          <RegistryPanel onClose={() => setShowingRegistry(false)} />
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

function CoreUnreachable({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}): React.JSX.Element {
  return (
    <div className="grid h-full place-items-center p-8">
      <div className="max-w-md text-center">
        <h1 className="text-lg font-semibold text-[--color-recording]">
          {t("core.unreachable.title")}
        </h1>
        {/* The recording is not lost when this window cannot reach the Core —
            the Core is a separate process and keeps going (ADR-0026). */}
        <p className="mt-2 text-sm text-[--color-ink-muted]">
          {t("core.unreachable.hint")}
        </p>
        {/* The Core reports why by catalog key where it has one to give,
            so the sentence the Operator reads is translated rather than
            whatever English the main process happened to build. Anything
            else — an OS error, a socket path — is shown as it came. */}
        <pre className="mt-4 overflow-x-auto rounded bg-[--color-surface-raised] p-3 text-left text-xs text-[--color-ink-muted]">
          {isMessageKey(message) ? t(message) : message}
        </pre>
        <button
          type="button"
          onClick={onRetry}
          className="mt-4 rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
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
  onSettings,
  onRegistry,
  onPosture,
}: {
  meetings: Meeting[];
  activeId: string | null;
  recordingId: string | null;
  onSelect: (id: string) => void;
  onRecord: () => void;
  onStop: () => void;
  onSettings: () => void;
  onRegistry: () => void;
  onPosture: () => void;
}): React.JSX.Element {
  return (
    <aside className="flex h-full flex-col border-r border-[--color-line] bg-[--color-surface-raised]">
      <header className="flex items-center justify-between border-b border-[--color-line] px-4 py-3">
        <button
          type="button"
          onClick={onSettings}
          title={t("settings.open")}
          className="text-sm font-semibold hover:text-[--color-ink-muted]"
        >
          {t("app.title")}
        </button>
        <button
          type="button"
          onClick={onRegistry}
          title={t("registry.title")}
          className="text-xs text-[--color-ink-muted] hover:text-[--color-ink]"
        >
          {t("registry.open")}
        </button>
        <button
          type="button"
          onClick={onPosture}
          title={t("posture.title")}
          className="text-xs text-[--color-ink-muted] hover:text-[--color-ink]"
        >
          {t("posture.open")}
        </button>
        {recordingId ? (
          <button
            type="button"
            onClick={onStop}
            className="rounded bg-[--color-recording] px-2.5 py-1 text-xs font-medium text-white"
          >
            {t("action.stop")}
          </button>
        ) : (
          <button
            type="button"
            onClick={onRecord}
            className="rounded border border-[--color-line] px-2.5 py-1 text-xs font-medium hover:bg-[--color-surface]"
          >
            {t("action.record")}
          </button>
        )}
      </header>

      <ul className="min-h-0 flex-1 overflow-y-auto">
        {meetings.length === 0 ? (
          <li className="px-4 py-6 text-sm text-[--color-ink-muted]">
            {t("meetings.empty")}
            <span className="mt-1 block text-xs">{t("meetings.emptyHint")}</span>
          </li>
        ) : (
          meetings.map((meeting) => (
            <li key={meeting.id}>
              <button
                type="button"
                onClick={() => onSelect(meeting.id)}
                className={`w-full border-b border-[--color-line] px-4 py-3 text-left ${
                  meeting.id === activeId ? "bg-[--color-surface]" : ""
                }`}
              >
                <span className="block truncate text-sm">
                  {displayTitle(meeting)}
                </span>
                <span className="mt-0.5 flex items-center gap-1.5 text-xs text-[--color-ink-muted]">
                  {meeting.id === recordingId ? (
                    <>
                      <span className="inline-block size-1.5 rounded-full bg-[--color-recording]" />
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
      <header className="border-b border-[--color-line] px-6 py-4">
        {editing ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (draft.trim()) onRetitle(draft.trim());
              setEditing(false);
            }}
            className="flex gap-2"
          >
            <input
              autoFocus
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              className="min-w-0 flex-1 rounded border border-[--color-line] bg-[--color-surface] px-2 py-1 text-lg"
            />
            <button type="submit" className="rounded border border-[--color-line] px-3 text-sm">
              {t("action.save")}
            </button>
            <button
              type="button"
              onClick={() => setEditing(false)}
              className="rounded px-3 text-sm text-[--color-ink-muted]"
            >
              {t("action.cancel")}
            </button>
          </form>
        ) : (
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold">
                {displayTitle(meeting)}
              </h1>
              <p className="mt-0.5 text-xs text-[--color-ink-muted]">
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
                className="rounded border border-[--color-line] px-2.5 py-1 text-xs"
              >
                {t("action.rename")}
              </button>
              <button
                type="button"
                onClick={() => {
                  // Deleting removes the audio too and cannot be undone.
                  if (window.confirm(t("meeting.deleteConfirm"))) onDelete();
                }}
                className="rounded border border-[--color-line] px-2.5 py-1 text-xs text-[--color-recording]"
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
        <div className="border-b border-[--color-line] bg-[--color-surface-raised] px-6 py-2 text-xs">
          <p className="font-medium text-[--color-recording]">
            {t("meeting.incomplete")}
          </p>
          <ul className="mt-1 list-disc pl-4 text-[--color-ink-muted]">
            {meeting.audioNotes.map((note) => (
              <li key={note}>{note}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {dropped > 0 ? (
        <p className="border-b border-[--color-line] bg-[--color-surface-raised] px-6 py-2 text-xs text-[--color-ink-muted]">
          {t("transcript.dropped")}
        </p>
      ) : null}

      <section className="min-h-0 flex-1 overflow-y-auto px-6 py-4">
        {segments.length === 0 ? (
          <p className="text-sm text-[--color-ink-muted]">
            {live ? t("transcript.listening") : t("transcript.empty")}
          </p>
        ) : (
          <ol className="space-y-3">
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
    <li className="grid grid-cols-[auto_1fr] gap-3">
      <span className="pt-0.5 font-mono text-xs text-[--color-ink-muted] tabular-nums">
        {formatTimestamp(segment.startMs)}
      </span>
      <span className="text-sm">
        <span className="mr-2 font-medium">{speaker}</span>
        {segment.text}
      </span>
    </li>
  );
}

function EmptyState(): React.JSX.Element {
  return (
    <div className="grid h-full place-items-center text-sm text-[--color-ink-muted]">
      {t("meeting.selectPrompt")}
    </div>
  );
}

function displayTitle(meeting: Meeting): string {
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
  onClose: () => void;
  onRerunSetup: () => void;
}): React.JSX.Element {
  const { settings, watchlist, error, update, addWatched, removeWatched } =
    useSettings();
  const [draft, setDraft] = useState("");

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{t("settings.title")}</h1>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
        >
          {t("settings.close")}
        </button>
      </header>

      {error ? (
        <p className="mb-4 text-sm text-[--color-recording]">{error}</p>
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
            <span className="block text-xs text-[--color-ink-muted]">
              {t("settings.autoRecord.hint")}
            </span>
          </span>
        </label>
      </section>

      <section className="mb-8">
        <span className="block text-sm font-medium">
          {t("settings.chineseScript")}
        </span>
        <span className="mb-2 block text-xs text-[--color-ink-muted]">
          {t("settings.chineseScript.hint")}
        </span>
        <select
          value={settings?.chineseScript ?? "simplified"}
          onChange={(changed) =>
            void update({
              chineseScript: changed.target.value as "simplified" | "traditional",
            })
          }
          className="rounded border border-[--color-line] bg-[--color-surface-raised] px-2 py-1 text-sm"
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
        <p className="mb-3 text-xs text-[--color-ink-muted]">
          {t("watchlist.hint")}
        </p>

        {watchlist && watchlist.entries.length === 0 ? (
          <p className="mb-3 text-xs text-[--color-recording]">
            {t("watchlist.empty")}
          </p>
        ) : null}

        <ul className="mb-4">
          {watchlist?.entries.map((entry) => (
            <li
              key={entry.id}
              className="flex items-center justify-between border-b border-[--color-line] py-2"
            >
              <span>
                <span className="block text-sm">{entry.name}</span>
                <span className="block font-mono text-xs text-[--color-ink-muted]">
                  {entry.kind === "browserMeetings"
                    ? t("watchlist.browserMeetings")
                    : entry.id}
                </span>
              </span>
              <button
                type="button"
                onClick={() => void removeWatched(entry.id)}
                className="rounded border border-[--color-line] px-2 py-1 text-xs hover:bg-[--color-surface-raised]"
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
            className="min-w-0 flex-1 rounded border border-[--color-line] bg-[--color-surface-raised] px-2 py-1 text-sm"
          />
          <button
            type="submit"
            className="rounded border border-[--color-line] px-3 py-1 text-sm hover:bg-[--color-surface-raised]"
          >
            {t("watchlist.add")}
          </button>
        </form>

        {watchlist && watchlist.suggestions.length > 0 ? (
          <>
            <h3 className="text-xs font-medium text-[--color-ink-muted]">
              {t("watchlist.suggested")}
            </h3>
            <ul>
              {watchlist.suggestions.map((entry) => (
                <li key={entry.id} className="flex items-center justify-between py-2">
                  <span className="text-sm text-[--color-ink-muted]">
                    {entry.name}
                  </span>
                  <button
                    type="button"
                    onClick={() => void addWatched(entry.id)}
                    className="rounded border border-[--color-line] px-2 py-1 text-xs hover:bg-[--color-surface-raised]"
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
            <span className="mt-0.5 block text-xs text-[--color-ink-muted]">
              {t("updates.hint")}
            </span>
          </span>
        </label>

        {/* An Operator who skipped a step needs a way back that is not
            reinstalling. */}
        <button
          type="button"
          onClick={onRerunSetup}
          className="mt-4 rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
        >
          {t("onboarding.reopen")}
        </button>
      </section>
    </div>
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
function RegistryPanel({ onClose }: { onClose: () => void }): React.JSX.Element {
  const { speakers, error, rename, forgetVoice } = useRegistry();
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  const nameOf = (speaker: Speaker): string => {
    if (speaker.displayName) return speaker.displayName;
    return speaker.isOperator ? t("registry.you") : t("registry.unnamed");
  };

  const voiceprintLabel = (speaker: Speaker): string => {
    if (!speaker.hasVoiceprint) return t("registry.voiceprint.none");
    return speaker.confirmed
      ? t("registry.voiceprint.confirmed")
      : t("registry.voiceprint.unconfirmed");
  };

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{t("registry.title")}</h1>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
        >
          {t("registry.close")}
        </button>
      </header>

      <p className="mb-4 text-xs text-[--color-ink-muted]">
        {t("registry.hint")}
      </p>

      {error ? (
        <p className="mb-4 text-sm text-[--color-recording]">{error}</p>
      ) : null}

      {speakers && speakers.speakers.length === 0 ? (
        <p className="text-xs text-[--color-ink-muted]">{t("registry.empty")}</p>
      ) : null}

      <ul>
        {speakers?.speakers.map((speaker) => (
          <li
            key={speaker.id}
            className="border-b border-[--color-line] py-3"
          >
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                {editingId === speaker.id ? (
                  <form
                    onSubmit={(submitted) => {
                      submitted.preventDefault();
                      if (draft.trim()) void rename(speaker.id, draft.trim());
                      setEditingId(null);
                    }}
                    className="flex gap-2"
                  >
                    <input
                      autoFocus
                      value={draft}
                      onChange={(changed) => setDraft(changed.target.value)}
                      className="min-w-0 rounded border border-[--color-line] bg-[--color-surface-raised] px-2 py-1 text-sm"
                    />
                    <button type="submit" className="rounded border border-[--color-line] px-2 text-xs">
                      {t("action.save")}
                    </button>
                    <button
                      type="button"
                      onClick={() => setEditingId(null)}
                      className="px-2 text-xs text-[--color-ink-muted]"
                    >
                      {t("action.cancel")}
                    </button>
                  </form>
                ) : (
                  <span className="block truncate text-sm">{nameOf(speaker)}</span>
                )}
                <span className="mt-0.5 block text-xs text-[--color-ink-muted]">
                  {voiceprintLabel(speaker)} · {speaker.meetingsSeenIn}{" "}
                  {t("registry.meetings")}
                </span>
              </div>

              <div className="flex shrink-0 gap-2">
                <button
                  type="button"
                  onClick={() => {
                    setDraft(speaker.displayName ?? "");
                    setEditingId(speaker.id);
                  }}
                  className="rounded border border-[--color-line] px-2 py-1 text-xs hover:bg-[--color-surface-raised]"
                >
                  {t("registry.rename")}
                </button>
                {speaker.hasVoiceprint ? (
                  <button
                    type="button"
                    onClick={() => setConfirmingId(speaker.id)}
                    className="rounded border border-[--color-line] px-2 py-1 text-xs hover:bg-[--color-surface-raised]"
                  >
                    {t("registry.forget")}
                  </button>
                ) : null}
              </div>
            </div>

            {editingId === speaker.id ? (
              <p className="mt-2 text-xs text-[--color-ink-muted]">
                {t("registry.rename.hint")}
              </p>
            ) : null}

            {/* Said before it happens, not after: a biometric deletion is a
                legible act, and the Operator has to know it costs
                recognition and costs the record nothing. */}
            {confirmingId === speaker.id ? (
              <div className="mt-3 rounded border border-[--color-line] p-3">
                <p className="mb-2 text-xs text-[--color-ink-muted]">
                  {t("registry.forget.hint")}
                </p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => {
                      void forgetVoice(speaker.id);
                      setConfirmingId(null);
                    }}
                    className="rounded border border-[--color-recording] px-2 py-1 text-xs text-[--color-recording]"
                  >
                    {t("registry.forget.confirm")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmingId(null)}
                    className="px-2 py-1 text-xs text-[--color-ink-muted]"
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
    <div className="border-t border-[--color-line] px-6 py-4">
      {error ? (
        <p className="mb-3 text-xs text-[--color-recording]">{error}</p>
      ) : null}

      <section className="mb-5">
        <h2 className="text-sm font-medium">{t("summary.title")}</h2>
        {meeting?.summary ? (
          <>
            <pre className="mt-2 whitespace-pre-wrap font-sans text-sm">
              {meeting.summary}
            </pre>
            {meeting.summaryBackend ? (
              /* Story 38: which Backend actually ran, beside the thing it
                 produced rather than buried in Settings. */
              <p className="mt-2 text-xs text-[--color-ink-muted]">
                {t("summary.generatedBy")} {meeting.summaryBackend}
              </p>
            ) : null}
          </>
        ) : (
          <p className="mt-1 text-xs text-[--color-ink-muted]">
            {t("summary.none")}
          </p>
        )}
        <button
          type="button"
          disabled={generating}
          onClick={() => void generate()}
          className="mt-3 rounded border border-[--color-line] px-3 py-1 text-xs hover:bg-[--color-surface-raised] disabled:opacity-50"
        >
          {generating ? t("summary.generating") : t("summary.generate")}
        </button>
      </section>

      <section>
        <h2 className="text-sm font-medium">{t("notes.title")}</h2>
        <p className="mb-2 text-xs text-[--color-ink-muted]">{t("notes.hint")}</p>
        <textarea
          value={notes}
          onChange={(changed) => setDraft(changed.target.value)}
          placeholder={t("notes.placeholder")}
          rows={5}
          className="w-full rounded border border-[--color-line] bg-[--color-surface-raised] px-2 py-1 text-sm"
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
      <p className="mb-3 text-xs text-[--color-ink-muted]">{t("backend.hint")}</p>

      {error ? (
        <p className="mb-3 text-sm text-[--color-recording]">{error}</p>
      ) : null}

      {backends && !backends.chosen ? (
        <p className="mb-3 text-xs text-[--color-recording]">
          {t("summary.unchosen")}
        </p>
      ) : null}

      <ul className="mb-4">
        {backends?.options.map((option) => (
          <li key={option.id} className="border-b border-[--color-line] py-3">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <span className="block text-sm">
                  {option.displayName}
                  {option.id === "local" ? (
                    <span className="ml-2 rounded bg-[--color-surface-raised] px-1.5 py-0.5 text-xs">
                      {t("backend.recommended")}
                    </span>
                  ) : null}
                </span>
                <span className="mt-0.5 block text-xs text-[--color-ink-muted]">
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
                className="shrink-0 rounded border border-[--color-line] px-2 py-1 text-xs hover:bg-[--color-surface-raised] disabled:opacity-40"
              >
                {backends.chosen === option.id ? "✓" : "Use"}
              </button>
            </div>

            {option.dataHandling ? (
              <p className="mt-2 text-xs text-[--color-ink-muted]">
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
                <p className="text-xs text-[--color-ink-muted]">
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
                    className="min-w-0 flex-1 rounded border border-[--color-line] bg-[--color-surface-raised] px-2 py-1 text-xs"
                  />
                  <button
                    type="button"
                    onClick={() => {
                      void setKey(option.id, keyDraft[option.id] ?? "");
                      setKeyDraft({ ...keyDraft, [option.id]: "" });
                    }}
                    className="rounded border border-[--color-line] px-2 py-1 text-xs"
                  >
                    {t("backend.key.save")}
                  </button>
                  {option.hasKey ? (
                    <button
                      type="button"
                      onClick={() => void setKey(option.id, null)}
                      className="rounded px-2 py-1 text-xs text-[--color-ink-muted]"
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
              <div className="mt-3 rounded border border-[--color-recording] p-3">
                <p className="text-sm font-medium">{t("backend.warning.title")}</p>
                <p className="mt-1 text-xs text-[--color-ink-muted]">
                  {t("backend.warning.body")}
                </p>
                <div className="mt-2 flex gap-2">
                  <button
                    type="button"
                    onClick={() => {
                      void choose(option.id, true);
                      setConfirming(null);
                    }}
                    className="rounded border border-[--color-recording] px-2 py-1 text-xs text-[--color-recording]"
                  >
                    {t("backend.warning.accept")}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirming(null)}
                    className="px-2 py-1 text-xs text-[--color-ink-muted]"
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
          <span className="mt-0.5 block text-xs text-[--color-ink-muted]">
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
  const { posture, error } = usePosture();

  return (
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-6 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{t("posture.title")}</h1>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
        >
          {t("registry.close")}
        </button>
      </header>

      {error ? (
        <p className="mb-4 text-sm text-[--color-recording]">{error}</p>
      ) : null}

      {posture ? (
        <>
          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.holds")}</h2>
            <dl className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
              <dt className="text-[--color-ink-muted]">{t("posture.meetings")}</dt>
              <dd>{posture.meetings}</dd>
              <dt className="text-[--color-ink-muted]">{t("posture.speakers")}</dt>
              <dd>{posture.speakers}</dd>
              {/* The biometric count, as a number rather than a category. */}
              <dt className="text-[--color-ink-muted]">
                {t("posture.voiceprints")}
              </dt>
              <dd>{posture.voiceprints}</dd>
              <dt className="text-[--color-ink-muted]">{t("posture.models")}</dt>
              <dd>{posture.models.join(", ") || "—"}</dd>
              <dt className="text-[--color-ink-muted]">{t("posture.folder")}</dt>
              <dd className="truncate font-mono text-xs">{posture.historyDir}</dd>
            </dl>
            <p className="mt-2 text-xs text-[--color-ink-muted]">
              {posture.calendarGranted
                ? t("posture.calendar.granted")
                : t("posture.calendar.withheld")}
            </p>
          </section>

          <section className="mb-6">
            <h2 className="text-sm font-medium">{t("posture.wire")}</h2>
            <p
              className={`mt-1 text-xs ${
                posture.currentlySilent ? "" : "text-[--color-recording]"
              }`}
            >
              {posture.currentlySilent
                ? t("posture.silent")
                : t("posture.notSilent")}
            </p>
            <ul className="mt-2">
              {posture.traffic.map((entry) => (
                <li key={entry.name} className="border-b border-[--color-line] py-2">
                  <span className="block text-sm">
                    {entry.name} —{" "}
                    <span className="text-[--color-ink-muted]">
                      {entry.enabled ? t("posture.enabled") : t("posture.disabled")}
                    </span>
                  </span>
                  <span className="block font-mono text-xs text-[--color-ink-muted]">
                    {entry.host}
                  </span>
                  <span className="mt-0.5 block text-xs text-[--color-ink-muted]">
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
                  <span className="block text-xs text-[--color-ink-muted]">
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
                  <span className="block text-xs text-[--color-ink-muted]">
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
    <div className="flex h-full flex-col overflow-y-auto p-6">
      <header className="mb-4">
        <h1 className="text-lg font-semibold">{t("onboarding.title")}</h1>
        <p className="text-xs text-[--color-ink-muted]">
          {t("onboarding.step")} {step + 1} {t("onboarding.of")} {steps.length}
        </p>
      </header>

      <div className="min-h-0 flex-1">
        {current === "briefing" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.briefing.title")}</h2>
            <pre className="mt-2 max-h-[50vh] overflow-y-auto whitespace-pre-wrap rounded border border-[--color-line] bg-[--color-surface-raised] p-3 font-sans text-sm">
              {briefing?.text ?? ""}
            </pre>
            {briefing?.acknowledged ? null : (
              <button
                type="button"
                onClick={() => void acknowledge()}
                className="mt-3 rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised]"
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
            <p className="mt-2 text-sm text-[--color-ink-muted]">
              {t("onboarding.permissions.body")}
            </p>
          </section>
        ) : null}

        {current === "models" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.models.title")}</h2>
            <p className="mt-2 text-sm text-[--color-ink-muted]">
              {t("onboarding.models.body")}
            </p>
          </section>
        ) : null}

        {current === "folder" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.folder.title")}</h2>
            <p className="mt-2 text-sm text-[--color-ink-muted]">
              {t("onboarding.folder.body")}
            </p>
          </section>
        ) : null}

        {current === "backend" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.backend.title")}</h2>
            <p className="mt-2 text-sm text-[--color-ink-muted]">
              {t("onboarding.backend.body")}
            </p>
            <BackendPanel />
          </section>
        ) : null}

        {current === "calendar" ? (
          <section>
            <h2 className="text-sm font-medium">{t("onboarding.calendar.title")}</h2>
            <p className="mt-2 text-sm text-[--color-ink-muted]">
              {t("onboarding.calendar.body")}
            </p>
            <p className="mt-2 text-xs text-[--color-ink-muted]">
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
          className="rounded border border-[--color-line] px-3 py-1.5 text-sm hover:bg-[--color-surface-raised] disabled:opacity-40"
        >
          {step + 1 === steps.length ? t("onboarding.done") : t("onboarding.next")}
        </button>
        {!blocked && current !== "briefing" && current !== "backend" ? (
          <button
            type="button"
            onClick={advance}
            className="px-3 py-1.5 text-sm text-[--color-ink-muted]"
          >
            {t("onboarding.skip")}
          </button>
        ) : null}
      </footer>
    </div>
  );
}
