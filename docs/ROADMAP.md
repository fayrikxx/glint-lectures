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
- [x] The same on macOS with ScreenCaptureKit
- [x] Crop the capture to the lecture area

## 2. Slide detection

- [ ] Analyze frames on a background thread that stays out of the way of other programs: a downscaled copy of the slide area, with stillness measured in time (a window that doesn't change sends no frames at all)
- [ ] Replay recorded captures through the detector to measure it on real lectures: missed slides, extra slides, delay
- [ ] Count a new slide when the slide area changes enough and then stays still (~1.5 s), and keep its sharpest frame
- [ ] Show the slides found so far as a strip of thumbnails under the preview
- [ ] Don't count what isn't a new slide: cursor, laser pointer, captions, reactions, player controls, webcam tiles, the presenter bubble, video playing in the window
- [ ] Bullet builds and live ink update the current slide, and going back to an earlier slide links to it instead of adding a copy
- [ ] Find the slide area by itself and follow it through layout changes (resize, fullscreen, side panels) without counting them as new slides
- [ ] A manual crop stays on top of the found area; notice when the slides leave the window (pop-out, picture-in-picture) and suggest the window that shows them
- [ ] Tests on synthetic frame sequences for all of the above
- [ ] Capture a screen region when window capture doesn't work for some app

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
