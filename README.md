<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" width="112" alt="Glint logo">
  </picture>
</p>

<h1 align="center">Glint</h1>

<p align="center">Catch the key 20% of every lecture.</p>

---

Glint is a desktop app for Windows and macOS that sits next to your online lecture
(Teams, Zoom, a browser tab, anything with a window) and quietly saves every new slide.
Afterwards it reads the text on the slides and turns the whole lecture into short notes
that point back to the slides they came from.

I started it because lecturers flip through slides fast, rarely upload them on time,
and taking 40 screenshots by hand means missing half of what is being said.

> Early development. Nothing is usable yet; see the [roadmap](docs/ROADMAP.md) for where things are.

## What it should do

- **Capture a window** and grab a new frame only when the slide actually changes.
- **Ignore webcams and names.** Participant tiles move all the time, slides don't, so moving
  regions are masked out and faces are blurred in the saved images.
- **Merge bullet builds.** When bullet points appear one by one, keep only the final slide
  instead of five near-duplicates.
- **Read the slides** with the OCR built into the OS (Windows.Media.Ocr / Apple Vision), offline.
- **Write notes.** Group slides into sections, highlight the few ideas that matter most and link
  every point to its source slide. Works with a local model (Ollama) or a cloud API key.
- **Export** to PDF or Markdown.

## How it works

```
window capture ─▶ downscale + compare ─▶ stable for ~1.5 s? ─▶ new slide
                        │                                     │
                 mask moving regions                   merge if it only
                 (webcams, cursor)                     added text to the last one
                                                              │
                                     OCR ─▶ notes (optional LLM) ─▶ export
```

Slide detection is plain image processing, no ML: frames are shrunk and converted to grayscale,
compared against the previous stable frame, and a change only counts once it covers enough of
the area and stays still for a moment. That keeps animations, the mouse pointer and laser
pointers from producing junk slides.

## Stack

- [Tauri 2](https://tauri.app) for the app shell
- Rust for capture, slide detection, OCR bindings and storage
- React + TypeScript + Vite for the interface

The detection logic lives in its own crate with no UI or platform dependencies, so it can be
tested on synthetic frames.

## Building

You'll need Rust (stable), Node 20+ and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS, then:

```sh
npm install
npm run tauri dev
```

`npm run tauri build` makes a release build. The slide detection tests run with
`cargo test -p glint-core` and don't need a screen.

## Design

Colors, type, themes and the mascot are described in [docs/DESIGN.md](docs/DESIGN.md).

## License

[MIT](LICENSE)
