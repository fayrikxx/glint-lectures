import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { WindowIcon, type WindowInfo } from "./WindowPicker";

// Matches CaptureState::code in src-tauri/src/capture/mod.rs.
const NO_CAPTURE = 0;
const CAPTURING = 1;
const MINIMIZED = 2;
const HIDDEN = 3;
const WINDOW_CLOSED = 4;
const FAILED = 5;

const PREVIEW_HEADER_LEN = 20;

type CaptureStatus = {
  sessionId: number;
  windowId: number;
  state: { kind: string; message?: string };
};

type Props = {
  sessionId: number;
  target: WindowInfo | null;
  onStop: () => void;
};

export default function CaptureView({ sessionId, target, onStop }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [state, setState] = useState(CAPTURING);
  const [hasFrame, setHasFrame] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    // `invoke` can't be cancelled, so the loop checks this flag after every reply.
    // In dev, StrictMode mounts twice and the first loop stops after its pending request.
    let stopped = false;

    async function pullFrames() {
      let after = 0;
      while (!stopped) {
        let reply: ArrayBuffer;
        try {
          reply = await invoke<ArrayBuffer>("preview_frame", { after });
        } catch (error) {
          if (!stopped) {
            setFailure(String(error));
            setState(FAILED);
          }
          return;
        }
        if (stopped) {
          return;
        }

        const header = new DataView(reply, 0, PREVIEW_HEADER_LEN);
        const id = Number(header.getBigUint64(0, true));
        const width = header.getUint32(8, true);
        const height = header.getUint32(12, true);
        const code = header.getUint32(16, true);
        setState(code);

        if (id !== 0) {
          after = id;
          drawFrame(canvasRef.current, reply, width, height);
          setHasFrame(true);
        }
        if (code === FAILED) {
          const status = await invoke<CaptureStatus | null>("capture_status");
          setFailure(status?.state.message ?? null);
        }
        if (code === NO_CAPTURE || code === WINDOW_CLOSED || code === FAILED) {
          return;
        }
      }
    }

    void pullFrames();
    return () => {
      stopped = true;
    };
  }, [sessionId]);

  const finished = state === NO_CAPTURE || state === WINDOW_CLOSED || state === FAILED;

  return (
    <section className="capture" aria-labelledby="capture-title">
      <div className="capture-head">
        <div className="capture-window">
          {target && <WindowIcon item={target} />}
          <div className="window-text">
            <h2 id="capture-title" className="window-title capture-title">
              {target?.title ?? "Capturing a window"}
            </h2>
            {target?.appName && <span className="window-app">{target.appName}</span>}
          </div>
        </div>
        <button type="button" className="chunky-button" onClick={onStop}>
          {finished ? "Pick another window" : "Stop"}
        </button>
      </div>

      <div className="capture-preview">
        <canvas
          ref={canvasRef}
          className="capture-canvas"
          hidden={!hasFrame}
          aria-label="Live preview of the captured window"
        />
        {!hasFrame && !finished && <p className="picker-note">Waiting for the first frame...</p>}
      </div>

      <p className="capture-status" role="status">
        {statusText(state, failure)}
      </p>
    </section>
  );
}

function drawFrame(
  canvas: HTMLCanvasElement | null,
  reply: ArrayBuffer,
  width: number,
  height: number,
) {
  const context = canvas?.getContext("2d");
  if (!canvas || !context || width === 0 || height === 0) {
    return;
  }
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  const pixels = new Uint8ClampedArray(reply, PREVIEW_HEADER_LEN, width * height * 4);
  context.putImageData(new ImageData(pixels, width, height), 0, 0);
}

function statusText(state: number, failure: string | null): string {
  switch (state) {
    case CAPTURING:
      return "Capturing.";
    case MINIMIZED:
      return "The window is minimized. Restore it to keep capturing.";
    case HIDDEN:
      return "The window is hidden. Bring it back to keep capturing.";
    case WINDOW_CLOSED:
      return "The window was closed.";
    case FAILED:
      return failure ? `Capture stopped: ${failure}` : "Capture stopped.";
    default:
      return "Capture isn't running.";
  }
}
