# Roadmap

Each stage should end with something that runs. Order matters more than dates.

## 0. Skeleton

- [ ] Tauri 2 app with React + TypeScript, window titled "Glint" with the logo
- [ ] Cargo workspace: `crates/glint-core` (pure logic) and `src-tauri` (app shell)
- [ ] CI: fmt, clippy and tests on every push; app build on Windows and macOS

## 1. Window capture

- [ ] List open windows with titles and app icons
- [ ] Pick one and stream frames from it (Windows Graphics Capture on Windows, ScreenCaptureKit on macOS)
- [ ] Live preview in the UI, a few frames per second is enough
- [ ] Draw a capture region by hand as a fallback

## 2. Slide detection

- [ ] Downscale + grayscale frames, compare with the last stable frame
- [ ] Count a change only above an area threshold and after it stays still (~1.5 s)
- [ ] Motion map: mask regions that keep changing (webcam tiles, presenter bubble)
- [ ] Detect bullet builds and replace the previous slide instead of adding a new one
- [ ] Tests on synthetic frame sequences for all of the above

## 3. Lectures and review

- [ ] Save each lecture as a folder: slide PNGs + `lecture.json`
- [ ] Library screen with all lectures
- [ ] Review screen: slide list, big preview, delete / crop / merge with previous
- [ ] Blur faces in saved slides

## 4. OCR

- [ ] Windows.Media.Ocr and Apple Vision behind one trait
- [ ] Store text with line positions; guess the title from the top line
- [ ] Search across all slides

## 5. Export

- [ ] Markdown export
- [ ] PDF export with slides and text

## 6. Notes

- [ ] Provider trait with Ollama, Anthropic and OpenAI implementations
- [ ] API keys in the OS keychain, never in config files
- [ ] Per-slide summary, then sections, then a lecture overview
- [ ] Highlight the key 20% on slides and in a "5-minute" view
- [ ] Every point links back to its slide; unclear parts are flagged, not invented

## 7. Audio (optional)

- [ ] System audio capture (WASAPI loopback / ScreenCaptureKit)
- [ ] Local transcription with whisper.cpp bindings, split by slide timestamps

## 8. Interface

- [ ] Design tokens from DESIGN.md, three themes (Calm, Bubbly, Bold), light and dark
- [ ] Mentor character with moods and the sparkle trail
- [ ] First-launch flow: pick a style, set up the mentor

## 9. Release

- [ ] Installers for Windows (MSI/NSIS) and macOS (DMG)
- [ ] Screenshots and a short GIF in the README
