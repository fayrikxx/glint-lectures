# Roadmap

Each stage should end with something that runs. Order matters more than dates.

## 0. Skeleton

- [x] Tauri 2 app with React + TypeScript, window titled "Glint" with the logo
- [x] Cargo workspace: `crates/glint-core` (pure logic) and `src-tauri` (app shell)
- [x] CI: fmt, clippy and tests on every push; app build on Windows and macOS

## 1. Window capture

- [x] List open windows with titles and app icons
- [x] Stream frames from the picked window with a live preview, and cope with closed, minimized and resized windows (Windows Graphics Capture)
- [x] Warn when the captured window is completely covered: browsers stop drawing it then, so the capture freezes
- [ ] The same on macOS with ScreenCaptureKit
- [x] Crop the capture to the lecture area
- [ ] Capture a screen region when window capture doesn't work for some app

## 2. Slide detection

- [ ] Downscale + grayscale frames, compare with the last stable frame
- [ ] Count a change only above an area threshold and after it stays still (~1.5 s). Measure stillness in time: a window that doesn't change sends no frames at all
- [ ] Motion map: mask regions that keep changing (webcam tiles, presenter bubble)
- [ ] Detect bullet builds and replace the previous slide instead of adding a new one
- [ ] Tests on synthetic frame sequences for all of the above

## 3. Lectures and review

- [ ] Save each lecture as a folder: slide PNGs + `lecture.json`
- [ ] Library screen with all lectures
- [ ] Review screen: slide list, big preview, delete / crop / merge with previous
- [ ] Switch to another window without ending the lecture (slideshows and pop-out windows open new ones)
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

- [ ] Capture the lecture's audio (WASAPI loopback on Windows, the same ScreenCaptureKit stream as the video on macOS)
- [ ] Local transcription with whisper.cpp bindings, split by slide timestamps

## 8. Interface

- [ ] Design tokens from DESIGN.md, three themes (Calm, Bubbly, Bold), light and dark
- [ ] Mentor character with moods and the sparkle trail
- [ ] First-launch flow: pick a style, set up the mentor

## 9. Release

- [ ] Installers for Windows (MSI/NSIS) and macOS (DMG)
- [ ] Sign macOS builds with a Developer ID, otherwise the Screen Recording permission resets on every update
- [ ] Screenshots and a short GIF in the README
