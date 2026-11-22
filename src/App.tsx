import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import logo from "../assets/logo.svg";
import logoDark from "../assets/logo-dark.svg";
import CaptureView, { type CropArea } from "./CaptureView";
import WindowPicker, { type WindowInfo } from "./WindowPicker";

type Screen =
  | { kind: "loading" }
  | { kind: "picking"; starting: boolean; error: string | null }
  | { kind: "capturing"; sessionId: number; target: WindowInfo | null; crop: CropArea | null };

type CaptureStatus = { sessionId: number; windowId: number; crop: CropArea | null };

const PICKING: Screen = { kind: "picking", starting: false, error: null };

export default function App() {
  const [screen, setScreen] = useState<Screen>({ kind: "loading" });

  useEffect(() => {
    // After a page reload the capture can still be running in Rust, so pick it up
    // instead of showing the window list as if nothing was going on.
    async function resume() {
      const status = await invoke<CaptureStatus | null>("capture_status");
      if (!status) {
        setScreen(PICKING);
        return;
      }
      const windows = await invoke<WindowInfo[]>("list_windows").catch(() => []);
      setScreen({
        kind: "capturing",
        sessionId: status.sessionId,
        target: windows.find((item) => item.id === status.windowId) ?? null,
        crop: status.crop,
      });
    }
    resume().catch(() => setScreen(PICKING));
  }, []);

  async function startCapture(target: WindowInfo) {
    setScreen({ kind: "picking", starting: true, error: null });
    try {
      const sessionId = await invoke<number>("start_capture", {
        windowId: target.id,
        pid: target.pid,
      });
      setScreen({ kind: "capturing", sessionId, target, crop: null });
    } catch (error) {
      setScreen({ kind: "picking", starting: false, error: String(error) });
    }
  }

  async function stopCapture(sessionId: number) {
    setScreen(PICKING);
    await invoke("stop_capture", { sessionId }).catch(() => {});
  }

  return (
    <main className="app">
      <header className="app-header">
        <picture>
          <source srcSet={logoDark} media="(prefers-color-scheme: dark)" />
          <img className="app-logo" src={logo} alt="" width={40} height={40} />
        </picture>
        <h1 className="wordmark">Glint</h1>
      </header>
      {screen.kind === "picking" && (
        <WindowPicker
          starting={screen.starting}
          startError={screen.error}
          onStart={(target) => void startCapture(target)}
        />
      )}
      {screen.kind === "capturing" && (
        <CaptureView
          key={screen.sessionId}
          sessionId={screen.sessionId}
          target={screen.target}
          initialCrop={screen.crop}
          onStop={() => void stopCapture(screen.sessionId)}
        />
      )}
    </main>
  );
}
