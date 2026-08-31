/**
 * The Client's whole relationship with the Core, in one hook.
 *
 * Everything the UI knows arrives either as a response to a request or as a
 * notification; nothing is polled and nothing is read from disk. That is not
 * a style choice — the Core is the record's only writer (ADR-0026), and a
 * renderer that reached around it would be a second one.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import type { JsonRpcNotification } from "@protocol/JsonRpcNotification";
import type { Meeting } from "@protocol/Meeting";
import type { SettingsResponse } from "@protocol/SettingsResponse";
import type { SettingsSetParams } from "@protocol/SettingsSetParams";
import type { SpeakerListResponse } from "@protocol/SpeakerListResponse";
import type { SummaryBackendsResponse } from "@protocol/SummaryBackendsResponse";
import type { SpeakerResponse } from "@protocol/SpeakerResponse";
import type { WatchlistResponse } from "@protocol/WatchlistResponse";
import type { MeetingDetailResponse } from "@protocol/MeetingDetailResponse";
import type { MeetingListResponse } from "@protocol/MeetingListResponse";
import type { MeetingResponse } from "@protocol/MeetingResponse";
import type { StatusResponse } from "@protocol/StatusResponse";
import type { TranscriptSegment } from "@protocol/TranscriptSegment";
import type { TranscriptSnapshotResponse } from "@protocol/TranscriptSnapshotResponse";

export type CoreState = {
  status: StatusResponse | null;
  meetings: Meeting[];
  error: string | null;
};

export function useCore() {
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, list] = await Promise.all([
        window.evertranscript.status(),
        window.evertranscript.request<MeetingListResponse>("meeting/list", {
          limit: 200,
        }),
      ]);
      setStatus(nextStatus);
      setMeetings(list.meetings);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Meeting changes arrive as notifications; this interval is only a
    // fallback for uptime and for a Core that restarted under us.
    const timer = setInterval(() => void refresh(), 5000);
    const unsubscribe = window.evertranscript.onNotification((notification) => {
      if (
        notification.method === "meeting/changed" ||
        notification.method === "core/stateChanged"
      ) {
        void refresh();
      }
    });
    return () => {
      clearInterval(timer);
      unsubscribe();
    };
  }, [refresh]);

  const startRecording = useCallback(async () => {
    await window.evertranscript.request<MeetingResponse>("meeting/start", {});
    await refresh();
  }, [refresh]);

  const stopRecording = useCallback(async () => {
    await window.evertranscript.request<MeetingResponse>("meeting/stop", {});
    await refresh();
  }, [refresh]);

  const retitle = useCallback(
    async (id: string, title: string) => {
      await window.evertranscript.request<MeetingResponse>("meeting/retitle", {
        id,
        title,
      });
      await refresh();
    },
    [refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      await window.evertranscript.request("meeting/delete", { id });
      await refresh();
    },
    [refresh],
  );

  return {
    status,
    meetings,
    error,
    refresh,
    startRecording,
    stopRecording,
    retitle,
    remove,
  };
}

/**
 * A Meeting's transcript, live when it is the one recording.
 *
 * The subscribe call returns the transcript so far *and* subscribes in one
 * step, so a segment finishing between "fetch" and "subscribe" cannot fall
 * through the gap (ADR-0028, snapshot-then-tail).
 */
export function useTranscript(meetingId: string | null, live: boolean) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [dropped, setDropped] = useState(0);
  const currentId = useRef<string | null>(null);

  useEffect(() => {
    currentId.current = meetingId;
    setSegments([]);
    setDropped(0);
    if (!meetingId) return;

    let cancelled = false;

    const load = async () => {
      if (live) {
        const snapshot =
          await window.evertranscript.request<TranscriptSnapshotResponse>(
            "transcript/subscribe",
            { meetingId },
          );
        if (!cancelled) setSegments(snapshot.segments);
      } else {
        const detail =
          await window.evertranscript.request<MeetingDetailResponse>(
            "meeting/get",
            { id: meetingId },
          );
        if (!cancelled) setSegments(detail.segments);
      }
    };
    void load().catch(() => {
      /* the surrounding view already reports connection failures */
    });

    const unsubscribe = window.evertranscript.onNotification(
      (notification: JsonRpcNotification) => {
        if (notification.method === "transcript/segmentAdded") {
          const params = notification.params as
            | { meetingId: string; segment: TranscriptSegment }
            | undefined;
          if (!params || params.meetingId !== currentId.current) return;
          setSegments((previous) => [...previous, params.segment]);
        }
        if (notification.method === "transcript/captionsDropped") {
          const params = notification.params as { dropped: number } | undefined;
          setDropped((previous) => previous + (params?.dropped ?? 0));
        }
      },
    );

    return () => {
      cancelled = true;
      unsubscribe();
      if (live) {
        void window.evertranscript
          .request("transcript/unsubscribe", {})
          .catch(() => {
            /* the connection is going away anyway */
          });
      }
    };
  }, [meetingId, live]);

  return { segments, dropped };
}

/**
 * Settings and the Watchlist, together because they are one screen.
 *
 * Both are small and rarely change, so they are fetched on demand rather
 * than polled: a settings panel that re-reads twice a second would be
 * writing to the Core more often than the Operator does.
 */
export function useSettings() {
  const [settings, setSettings] = useState<SettingsResponse | null>(null);
  const [watchlist, setWatchlist] = useState<WatchlistResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextSettings, nextWatchlist] = await Promise.all([
        window.evertranscript.request<SettingsResponse>("settings/get", {}),
        window.evertranscript.request<WatchlistResponse>("watchlist/get", {}),
      ]);
      setSettings(nextSettings);
      setWatchlist(nextWatchlist);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const update = useCallback(
    async (change: Partial<SettingsSetParams>) => {
      setSettings(
        await window.evertranscript.request<SettingsResponse>("settings/set", change),
      );
    },
    [],
  );

  const addWatched = useCallback(async (id: string, name?: string) => {
    setWatchlist(
      await window.evertranscript.request<WatchlistResponse>("watchlist/add", {
        id,
        name,
      }),
    );
  }, []);

  const removeWatched = useCallback(async (id: string) => {
    setWatchlist(
      await window.evertranscript.request<WatchlistResponse>("watchlist/remove", {
        id,
      }),
    );
  }, []);

  return { settings, watchlist, error, update, addWatched, removeWatched };
}

/**
 * The Voice Registry.
 *
 * ADR-0008 made this surface mandatory rather than optional: the product
 * stores biometric identifiers for people who never consented, and the
 * bargain struck in exchange was that the inventory is fully inspectable and
 * every Voiceprint individually deletable. It loads without a Meeting open
 * because it describes the installation, not any one recording.
 */
export function useRegistry() {
  const [speakers, setSpeakers] = useState<SpeakerListResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSpeakers(
        await window.evertranscript.request<SpeakerListResponse>("speaker/list", {}),
      );
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const rename = useCallback(
    async (id: string, displayName: string) => {
      await window.evertranscript.request<SpeakerResponse>("speaker/rename", {
        id,
        displayName,
      });
      // Refetched rather than patched in place: a rename is retroactive
      // across every Meeting, so the counts beside every other row can
      // change too.
      await refresh();
    },
    [refresh],
  );

  const forgetVoice = useCallback(
    async (id: string) => {
      await window.evertranscript.request<SpeakerResponse>(
        "speaker/deleteVoiceprint",
        { id },
      );
      await refresh();
    },
    [refresh],
  );

  return { speakers, error, rename, forgetVoice };
}

/**
 * Notes and Summary for one Meeting.
 *
 * Notes save on a debounce rather than on a button: they are written *during*
 * a meeting, and a Save button is a thing to forget while listening to
 * somebody.
 */
export function useMeetingWriting(meetingId: string | null) {
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!meetingId) return;
    try {
      const detail = await window.evertranscript.request<MeetingDetailResponse>(
        "meeting/get",
        { id: meetingId },
      );
      setMeeting(detail.meeting);
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [meetingId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const saveNotes = useCallback(
    async (notes: string) => {
      if (!meetingId) return;
      const response = await window.evertranscript.request<MeetingResponse>(
        "meeting/setNotes",
        { id: meetingId, notes },
      );
      setMeeting(response.meeting);
    },
    [meetingId],
  );

  const generate = useCallback(async () => {
    if (!meetingId) return;
    setGenerating(true);
    setError(null);
    try {
      const response = await window.evertranscript.request<MeetingResponse>(
        "summary/generate",
        { id: meetingId },
      );
      setMeeting(response.meeting);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setGenerating(false);
    }
  }, [meetingId]);

  return { meeting, generating, error, saveNotes, generate };
}

/**
 * The Summary Backend picker (ADR-0013, ADR-0010).
 *
 * `chosen` being undefined is a real state the screen must show, not one to
 * default away: every configuration this product runs traces to an explicit
 * act.
 */
export function useSummaryBackends() {
  const [backends, setBackends] = useState<SummaryBackendsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setBackends(
        await window.evertranscript.request<SummaryBackendsResponse>(
          "summary/backends",
          {},
        ),
      );
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const choose = useCallback(
    async (id: string, acceptedWarning: boolean) => {
      // The Core refuses a cloud choice without the acceptance, so this is
      // a convenience rather than the gate. The gate must not live in a
      // renderer that a bug could skip.
      await window.evertranscript.request<SettingsResponse>("settings/set", {
        ...(acceptedWarning ? { summaryCloudWarningAccepted: true } : {}),
        summaryBackend: id,
      });
      await refresh();
    },
    [refresh],
  );

  const setStrict = useCallback(
    async (strict: boolean) => {
      await window.evertranscript.request<SettingsResponse>("settings/set", {
        summaryStrict: strict,
      });
      await refresh();
    },
    [refresh],
  );

  const setKey = useCallback(
    async (provider: string, key: string | null) => {
      await window.evertranscript.request<SummaryBackendsResponse>(
        "summary/setKey",
        key ? { provider, key } : { provider },
      );
      await refresh();
    },
    [refresh],
  );

  return { backends, error, choose, setStrict, setKey };
}
