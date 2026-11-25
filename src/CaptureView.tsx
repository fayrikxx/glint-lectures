import { useEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { WindowIcon, type WindowInfo } from "./WindowPicker";

// Matches CaptureState::code in src-tauri/src/capture/mod.rs.
const NO_CAPTURE = 0;
const CAPTURING = 1;
const MINIMIZED = 2;
const HIDDEN = 3;
const COVERED = 4;
const WINDOW_CLOSED = 5;
const FAILED = 6;

const PREVIEW_HEADER_LEN = 20;
// A drag smaller than this, as a share of the width or height, is a stray click.
const MIN_CROP_SIZE = 0.02;

/** Edges as fractions of the window's width and height. Matches CropBox in Rust. */
export type CropArea = { left: number; top: number; right: number; bottom: number };

type Point = { x: number; y: number };
type Size = { width: number; height: number };

type AnalysisReport = { state: string; seconds: number };

type CaptureStatus = {
  sessionId: number;
  windowId: number;
  state: { kind: string; message?: string };
  analysis: AnalysisReport;
};

type Props = {
  sessionId: number;
  target: WindowInfo | null;
  initialCrop: CropArea | null;
  onStop: () => void;
};

export default function CaptureView({ sessionId, target, initialCrop, onStop }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [state, setState] = useState(CAPTURING);
  const [frameSize, setFrameSize] = useState<Size | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  // `crop` is the area Rust uses. While editing, `draft` is the box on screen until Done.
  const [crop, setCrop] = useState(initialCrop);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<CropArea | null>(null);
  const [dragBox, setDragBox] = useState<CropArea | null>(null);
  const [cropError, setCropError] = useState<string | null>(null);
  const dragStart = useRef<Point | null>(null);
  // Dev builds show what the analysis thinks of the picture, to check it by hand.
  const [analysis, setAnalysis] = useState<AnalysisReport | null>(null);

  const finished = state === NO_CAPTURE || state === WINDOW_CLOSED || state === FAILED;
  const cropping = editing && !finished;
  // With a crop area set, the preview shows only that part: the pixels slides are read from.
  const showCropped = crop !== null && !editing;

  useEffect(() => {
    // `invoke` can't be cancelled, so the loop checks this flag after every reply. A new
    // loop starts whenever the preview switches between the whole window and the crop
    // area, and in dev StrictMode mounts twice. The old loop stops after its pending request.
    let stopped = false;

    async function pullFrames() {
      // 0 gets the newest frame right away instead of waiting for the window to change.
      let after = 0;
      while (!stopped) {
        let reply: ArrayBuffer;
        try {
          reply = await invoke<ArrayBuffer>("preview_frame", { after, cropped: showCropped });
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
          setFrameSize((old) =>
            old?.width === width && old.height === height ? old : { width, height },
          );
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
  }, [sessionId, showCropped]);

  useEffect(() => {
    if (!import.meta.env.DEV) {
      return;
    }
    let stopped = false;
    const timer = window.setInterval(() => {
      invoke<CaptureStatus | null>("capture_status")
        .then((status) => {
          if (!stopped) {
            setAnalysis(status?.sessionId === sessionId ? status.analysis : null);
          }
        })
        .catch(() => {});
    }, 1000);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [sessionId]);

  function startEditing() {
    setDraft(crop);
    setCropError(null);
    setEditing(true);
  }

  async function finishEditing() {
    try {
      await invoke("set_crop", { sessionId, area: draft });
    } catch (error) {
      setCropError(String(error));
      return;
    }
    setCrop(draft);
    setCropError(null);
    setEditing(false);
  }

  function startDrag(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) {
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    dragStart.current = pointIn(event);
  }

  function moveDrag(event: PointerEvent<HTMLDivElement>) {
    if (dragStart.current) {
      setDragBox(boxBetween(dragStart.current, pointIn(event)));
    }
  }

  function endDrag(event: PointerEvent<HTMLDivElement>) {
    const start = dragStart.current;
    dragStart.current = null;
    setDragBox(null);
    if (!start) {
      return;
    }
    const box = boxBetween(start, pointIn(event));
    if (box.right - box.left >= MIN_CROP_SIZE && box.bottom - box.top >= MIN_CROP_SIZE) {
      setDraft(box);
    }
  }

  function cancelDrag() {
    dragStart.current = null;
    setDragBox(null);
  }

  const shownBox = cropping ? (dragBox ?? draft) : null;

  let note = statusText(state, failure);
  if (cropError) {
    note = `Couldn't set the crop: ${cropError}`;
  } else if (cropping && state === CAPTURING) {
    note = "Drag over the window to frame the slides.";
  }

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
        <div
          className="capture-image"
          hidden={!frameSize}
          data-cropping={cropping || undefined}
          style={frameSize ? ratioStyle(frameSize) : undefined}
          onPointerDown={cropping ? startDrag : undefined}
          onPointerMove={cropping ? moveDrag : undefined}
          onPointerUp={cropping ? endDrag : undefined}
          onPointerCancel={cropping ? cancelDrag : undefined}
        >
          <canvas
            ref={canvasRef}
            className="capture-canvas"
            role="img"
            aria-label={
              showCropped
                ? "Live preview of the cropped area"
                : "Live preview of the captured window"
            }
          />
          {shownBox && <div className="crop-box" style={boxStyle(shownBox)} />}
        </div>
        {!frameSize && !finished && <p className="picker-note">Waiting for the first frame...</p>}
      </div>

      {analysis && !finished && (
        <p className="capture-status">
          {analysis.state} {analysis.seconds.toFixed(1)} s
        </p>
      )}

      <div className="capture-foot">
        <p className="capture-status" role="status">
          {note}
        </p>
        {frameSize && !finished && (
          <div className="capture-actions">
            {cropping ? (
              <>
                <button
                  type="button"
                  className="chunky-button secondary"
                  disabled={!draft}
                  onClick={() => setDraft(null)}
                >
                  Whole window
                </button>
                <button
                  type="button"
                  className="chunky-button"
                  onClick={() => void finishEditing()}
                >
                  Done
                </button>
              </>
            ) : (
              <button type="button" className="chunky-button secondary" onClick={startEditing}>
                Crop
              </button>
            )}
          </div>
        )}
      </div>
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

/** Where the pointer is inside the element, as fractions of its size. */
function pointIn(event: PointerEvent<HTMLElement>): Point {
  const bounds = event.currentTarget.getBoundingClientRect();
  return {
    x: clamp((event.clientX - bounds.left) / bounds.width),
    y: clamp((event.clientY - bounds.top) / bounds.height),
  };
}

function clamp(value: number): number {
  return Math.min(1, Math.max(0, value));
}

function boxBetween(a: Point, b: Point): CropArea {
  return {
    left: Math.min(a.x, b.x),
    top: Math.min(a.y, b.y),
    right: Math.max(a.x, b.x),
    bottom: Math.max(a.y, b.y),
  };
}

function boxStyle(box: CropArea): CSSProperties {
  return {
    left: `${box.left * 100}%`,
    top: `${box.top * 100}%`,
    width: `${(box.right - box.left) * 100}%`,
    height: `${(box.bottom - box.top) * 100}%`,
  };
}

// .capture-image takes the frame's aspect ratio from this variable.
function ratioStyle(size: Size): CSSProperties {
  return { "--frame-ratio": size.width / size.height } as CSSProperties;
}

function statusText(state: number, failure: string | null): string {
  switch (state) {
    case CAPTURING:
      return "Capturing.";
    case MINIMIZED:
      return "The window is minimized. Restore it to keep capturing.";
    case HIDDEN:
      return "The window is hidden. Bring it back to keep capturing.";
    case COVERED:
      return "The window is completely covered. Browsers pause windows you can't see, so leave a corner of it in view.";
    case WINDOW_CLOSED:
      return "The window was closed.";
    case FAILED:
      return failure ? `Capture stopped: ${failure}` : "Capture stopped.";
    default:
      return "Capture isn't running.";
  }
}
