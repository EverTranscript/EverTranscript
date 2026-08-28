import { useEffect, useState } from "react";

import type { StatusResponse } from "@protocol/StatusResponse";

import { t } from "./i18n";

/**
 * The M1 Client: proof the protocol works end to end. The Meeting list,
 * transcript, and live captions land in ticket 10.
 */
export function App(): React.JSX.Element {
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    const refresh = async (): Promise<void> => {
      try {
        const next = await window.evertranscript.status();
        if (!cancelled) {
          setStatus(next);
          setError(null);
        }
      } catch (cause) {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      }
    };

    void refresh();
    const timer = setInterval(() => void refresh(), 2000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  if (error) {
    return (
      <main className="state state--error">
        <h1>{t("core.unreachable.title")}</h1>
        <p>{error}</p>
      </main>
    );
  }

  if (!status) {
    return (
      <main className="state">
        <p>{t("core.connecting")}</p>
      </main>
    );
  }

  return (
    <main className="app">
      <header>
        <h1>EverTranscript</h1>
        <span className={`badge badge--${status.state}`}>
          {status.state === "recording" ? t("state.recording") : t("state.idle")}
        </span>
      </header>

      <dl className="facts">
        <dt>{t("field.version")}</dt>
        <dd>{status.version}</dd>
        <dt>{t("field.uptime")}</dt>
        <dd>{formatUptime(status.uptimeSeconds)}</dd>
        <dt>{t("field.history")}</dt>
        <dd className="path">{status.historyDir}</dd>
      </dl>

      {status.incompleteCopyWarning ? (
        <p className="warning">{status.incompleteCopyWarning}</p>
      ) : null}
    </main>
  );
}

function formatUptime(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds % 60}s`;
  return `${seconds}s`;
}
