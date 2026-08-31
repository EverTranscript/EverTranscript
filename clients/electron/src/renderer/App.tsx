import { useMemo, useState } from "react";

import type { Meeting } from "@protocol/Meeting";
import type { TranscriptSegment } from "@protocol/TranscriptSegment";

import { isMessageKey, t } from "./i18n";
import type { Speaker } from "@protocol/Speaker";
import { useCore, useRegistry, useSettings, useTranscript } from "./useCore";

export function App(): React.JSX.Element {
  const core = useCore();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showingSettings, setShowingSettings] = useState(false);
  const [showingRegistry, setShowingRegistry] = useState(false);

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
          setShowingRegistry((open) => !open);
        }}
      />
      <main className="flex h-full min-w-0 flex-col overflow-hidden">
        {showingSettings ? (
          <SettingsPanel onClose={() => setShowingSettings(false)} />
        ) : showingRegistry ? (
          <RegistryPanel onClose={() => setShowingRegistry(false)} />
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
}: {
  meetings: Meeting[];
  activeId: string | null;
  recordingId: string | null;
  onSelect: (id: string) => void;
  onRecord: () => void;
  onStop: () => void;
  onSettings: () => void;
  onRegistry: () => void;
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
function SettingsPanel({ onClose }: { onClose: () => void }): React.JSX.Element {
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
