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
