import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type WindowInfo = {
  id: number;
  pid: number;
  title: string;
  appName: string;
  icon: string | null;
};

type ListState =
  | { status: "loading" }
  | { status: "ready"; windows: WindowInfo[] }
  | { status: "failed"; error: string };

type Props = {
  starting: boolean;
  startError: string | null;
  onStart: (target: WindowInfo) => void;
};

export default function WindowPicker({ starting, startError, onStart }: Props) {
  const [list, setList] = useState<ListState>({ status: "loading" });
  const [selectedId, setSelectedId] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    setList({ status: "loading" });
    try {
      const windows = await invoke<WindowInfo[]>("list_windows");
      setList({ status: "ready", windows });
    } catch (error) {
      setList({ status: "failed", error: String(error) });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const selected =
    list.status === "ready" ? list.windows.find((item) => item.id === selectedId) : undefined;

  return (
    <section className="picker" aria-labelledby="picker-title">
      <div className="picker-head">
        <h2 id="picker-title" className="picker-title">
          Which window is the lecture in?
        </h2>
        <button
          type="button"
          className="chunky-button"
          onClick={() => void refresh()}
          disabled={list.status === "loading"}
        >
          Refresh
        </button>
      </div>

      {list.status === "loading" && <p className="picker-note">Looking for windows...</p>}

      {list.status === "failed" && (
        <p className="picker-note" role="alert">
          Couldn't list windows: {list.error}
        </p>
      )}

      {list.status === "ready" && list.windows.length === 0 && (
        <p className="picker-note">No windows yet. Open your lecture, then hit Refresh.</p>
      )}

      {list.status === "ready" && list.windows.length > 0 && (
        <ul className="window-list">
          {list.windows.map((item) => (
            <li key={item.id}>
              <button
                type="button"
                className="window-card"
                aria-pressed={item.id === selectedId}
                onClick={() => setSelectedId(item.id)}
              >
                <WindowIcon item={item} />
                <span className="window-text">
                  <span className="window-title">{item.title}</span>
                  {item.appName && <span className="window-app">{item.appName}</span>}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="picker-foot">
        <button
          type="button"
          className="chunky-button"
          onClick={() => selected && onStart(selected)}
          disabled={!selected || starting}
        >
          {starting ? "Starting..." : "Start capture"}
        </button>
        {startError && (
          <p className="picker-note" role="alert">
            Couldn't start: {startError}
          </p>
        )}
      </div>
    </section>
  );
}

export function WindowIcon({ item }: { item: WindowInfo }) {
  return item.icon ? (
    <img className="window-icon" src={item.icon} alt="" width={32} height={32} />
  ) : (
    <span className="window-icon window-icon-empty" aria-hidden="true">
      {(item.appName || item.title).charAt(0).toUpperCase()}
    </span>
  );
}
