use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glint_core::dump_format::{self, Event, EventRow, FrameRow};
use glint_core::{BgraFrame, CropArea};

use super::lock;

/// Frames waiting for the writer. When it falls further behind, new frames are dropped
/// rather than piling up in memory.
const QUEUE_LEN: usize = 2;

/// Records the frames the analysis gets into `GLINT_DUMP_DIR`, so real lectures can be
/// replayed later with `examples/replay.rs`. Only the crop area is kept, unless
/// `GLINT_DUMP_FULL=1` asks for the whole window.
pub(super) struct Recorder {
    /// Where recordings go, or `None` when nothing gets recorded.
    target: Option<Target>,
    recording: Option<Recording>,
    running: bool,
    /// Why frames aren't being recorded right now, for the dev line.
    note: Option<String>,
}

struct Target {
    dir: PathBuf,
    whole_window: bool,
}

impl Recorder {
    pub(super) fn from_env() -> Self {
        let Some(dir) = env::var_os("GLINT_DUMP_DIR") else {
            return Self::new(None, None);
        };
        let dir = PathBuf::from(dir);
        match check_dir(&dir, repo_root()) {
            Ok(()) => {
                let whole_window = env::var_os("GLINT_DUMP_FULL").is_some_and(|value| value == "1");
                Self::new(Some(Target { dir, whole_window }), None)
            }
            Err(reason) => Self::new(None, Some(format!("not recording: {reason}"))),
        }
    }

    fn new(target: Option<Target>, note: Option<String>) -> Self {
        Self {
            target,
            recording: None,
            running: true,
            note,
        }
    }

    pub(super) fn frame(&mut self, frame: &BgraFrame, crop: Option<CropArea>, now: Duration) {
        let Some(whole_window) = self.target.as_ref().map(|target| target.whole_window) else {
            return;
        };
        let area = match (crop, whole_window) {
            (Some(area), _) => area,
            (None, true) => CropArea::WHOLE,
            (None, false) => {
                self.note = Some("not recording: set a crop first".to_string());
                return;
            }
        };
        if self.recording.is_none() && !self.start(now) {
            return;
        }

        let image = if whole_window {
            frame.clone()
        } else {
            frame.crop(area)
        };
        let result = match &mut self.recording {
            Some(recording) => recording.add(frame.width(), frame.height(), area, image, now),
            None => Ok(()),
        };
        match result {
            Ok(()) => self.note = None,
            Err(reason) => self.stop(reason),
        }
    }

    pub(super) fn running(&mut self, now: Duration, running: bool) {
        self.running = running;
        let event = if running { Event::Resume } else { Event::Pause };
        let result = match &mut self.recording {
            Some(recording) => recording.event(now, event),
            None => Ok(()),
        };
        if let Err(reason) = result {
            self.stop(reason);
        }
    }

    pub(super) fn status(&self) -> Option<String> {
        self.note.clone().or_else(|| {
            self.recording
                .as_ref()
                .map(|recording| format!("recording frames · {}", recording.saved))
        })
    }

    fn start(&mut self, now: Duration) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_millis());
        let dir = target.dir.join(started.to_string());
        let mut recording = match Recording::create(&dir) {
            Ok(recording) => recording,
            Err(err) => {
                self.stop(format!("can't start in {}: {err}", dir.display()));
                return false;
            }
        };
        // The replay starts out running, so it has to learn about a pause from before.
        if !self.running
            && let Err(reason) = recording.event(now, Event::Pause)
        {
            self.stop(reason);
            return false;
        }
        self.recording = Some(recording);
        true
    }

    fn stop(&mut self, reason: String) {
        self.target = None;
        self.recording = None;
        self.note = Some(format!("recording stopped: {reason}"));
    }
}

struct Recording {
    dir: PathBuf,
    frames: File,
    events: File,
    next_frame: u64,
    saved: u64,
    /// The last saved image and where it came from, so copies of it can be skipped.
    last: Option<(u32, u32, CropArea, Arc<BgraFrame>)>,
    jobs: Option<SyncSender<Job>>,
    writer: Option<JoinHandle<()>>,
    /// Set by the writer when it can't write an image.
    failure: Arc<Mutex<Option<String>>>,
}

struct Job {
    path: PathBuf,
    image: Arc<BgraFrame>,
}

impl Recording {
    fn create(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        let frames = csv_file(&dir.join("frames.csv"), FrameRow::HEADER)?;
        let events = csv_file(&dir.join("events.csv"), EventRow::HEADER)?;
        let (jobs, queue) = mpsc::sync_channel(QUEUE_LEN);
        let failure = Arc::new(Mutex::new(None));
        let writer = {
            let failure = Arc::clone(&failure);
            thread::Builder::new()
                .name("glint-dump".to_string())
                .spawn(move || write_images(&queue, &failure))
                .map_err(|err| format!("failed to start the writer: {err}"))?
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            frames,
            events,
            next_frame: 1,
            saved: 0,
            last: None,
            jobs: Some(jobs),
            writer: Some(writer),
            failure,
        })
    }

    fn add(
        &mut self,
        width: u32,
        height: u32,
        area: CropArea,
        image: BgraFrame,
        now: Duration,
    ) -> Result<(), String> {
        if let Some(reason) = lock(&self.failure).take() {
            return Err(reason);
        }
        // The same picture in the same place changes nothing for the analysis.
        if let Some((last_width, last_height, last_area, last_image)) = &self.last
            && (*last_width, *last_height, *last_area) == (width, height, area)
            && **last_image == image
        {
            return Ok(());
        }

        let mut row = FrameRow {
            frame: self.next_frame,
            ms: millis(now),
            width,
            height,
            area,
            saved: false,
        };
        self.next_frame += 1;
        let image = Arc::new(image);
        let job = Job {
            path: self.dir.join(row.image_name()),
            image: Arc::clone(&image),
        };
        match self.jobs.as_ref().map(|jobs| jobs.try_send(job)) {
            Some(Ok(())) => {
                row.saved = true;
                self.saved += 1;
                self.last = Some((width, height, area, image));
            }
            Some(Err(TrySendError::Full(_))) | None => {}
            Some(Err(TrySendError::Disconnected(_))) => {
                let reason = lock(&self.failure).take();
                return Err(reason.unwrap_or_else(|| "the writer stopped".to_string()));
            }
        }
        writeln!(self.frames, "{}", row.to_line())
            .map_err(|err| format!("can't write frames.csv: {err}"))
    }

    fn event(&mut self, now: Duration, event: Event) -> Result<(), String> {
        let row = EventRow {
            ms: millis(now),
            event,
        };
        writeln!(self.events, "{}", row.to_line())
            .map_err(|err| format!("can't write events.csv: {err}"))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // Closing the queue lets the writer finish the frames already in it, then stop.
        drop(self.jobs.take());
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn csv_file(path: &Path, header: &str) -> Result<File, String> {
    let mut file = File::create(path).map_err(|err| err.to_string())?;
    writeln!(file, "{header}").map_err(|err| err.to_string())?;
    Ok(file)
}

fn write_images(queue: &Receiver<Job>, failure: &Mutex<Option<String>>) {
    for job in queue {
        if let Err(err) = write_png(&job.path, &job.image) {
            let name = job.path.file_name().unwrap_or_default().to_string_lossy();
            *lock(failure) = Some(format!("can't write {name}: {err}"));
            return;
        }
    }
}

fn write_png(path: &Path, image: &BgraFrame) -> Result<(), String> {
    let file = File::create(path).map_err(|err| err.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width(), image.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
    writer
        .write_image_data(&dump_format::to_rgb(image))
        .map_err(|err| err.to_string())?;
    writer.finish().map_err(|err| err.to_string())
}

fn millis(time: Duration) -> u64 {
    u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
}

/// The repository this build came from. Recording only exists in dev builds, and those
/// run on the machine they were built on.
fn repo_root() -> &'static Path {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest)
}

/// Recordings may only go to an absolute path outside the repository, so frames can't end
/// up in a commit by accident.
fn check_dir(dir: &Path, repo: &Path) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err("GLINT_DUMP_DIR has to be an absolute path".to_string());
    }
    if dir.components().any(|part| part == Component::ParentDir) {
        return Err("GLINT_DUMP_DIR can't contain ..".to_string());
    }
    // Windows paths ignore case.
    let lowercase = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
    if lowercase(dir).starts_with(lowercase(repo)) {
        return Err("GLINT_DUMP_DIR can't be inside the repository".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_core::dump_format::{parse_events, parse_frames};

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn frame(value: u8) -> BgraFrame {
        BgraFrame::new(64, 36, vec![value; 64 * 36 * 4]).unwrap()
    }

    /// A fresh folder in the system temp folder, and the one recording made in it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("glint-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn recording(&self) -> PathBuf {
            let mut entries = fs::read_dir(&self.0).unwrap();
            let recording = entries.next().unwrap().unwrap().path();
            assert!(entries.next().is_none());
            recording
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn recordings_stay_out_of_the_repository() {
        let repo = Path::new(r"D:\Projects\glint-lectures");
        assert!(check_dir(Path::new("samples"), repo).is_err());
        assert!(check_dir(Path::new(r"D:\Projects\glint-lectures\samples"), repo).is_err());
        assert!(check_dir(Path::new(r"d:\projects\Glint-Lectures"), repo).is_err());
        assert!(check_dir(Path::new(r"D:\Projects\glint-lectures\..\samples"), repo).is_err());
        assert!(check_dir(Path::new(r"D:\glint-samples"), repo).is_ok());
        assert!(check_dir(Path::new(r"D:\Projects\glint-lectures-samples"), repo).is_ok());
    }

    #[test]
    fn the_crop_area_is_saved_once_per_change() {
        let temp = TempDir::new("dump-area");
        let mut recorder = Recorder::new(
            Some(Target {
                dir: temp.0.clone(),
                whole_window: false,
            }),
            None,
        );
        let area = CropArea::new(0.0, 0.0, 0.5, 0.5).unwrap();

        recorder.frame(&frame(10), None, at(0));
        assert_eq!(
            recorder.status().as_deref(),
            Some("not recording: set a crop first")
        );
        recorder.frame(&frame(10), Some(area), at(100));
        recorder.frame(&frame(10), Some(area), at(350));
        recorder.running(at(400), false);
        recorder.frame(&frame(20), Some(area), at(600));
        assert_eq!(recorder.status().as_deref(), Some("recording frames · 2"));
        drop(recorder);

        let recording = temp.recording();
        let frames =
            parse_frames(&fs::read_to_string(recording.join("frames.csv")).unwrap()).unwrap();
        let saved: Vec<_> = frames
            .iter()
            .map(|row| (row.frame, row.ms, row.saved))
            .collect();
        assert_eq!(saved, [(1, 100, true), (2, 600, true)]);
        assert!(frames.iter().all(|row| row.area == area));

        let events =
            parse_events(&fs::read_to_string(recording.join("events.csv")).unwrap()).unwrap();
        assert_eq!(
            events,
            [EventRow {
                ms: 400,
                event: Event::Pause
            }]
        );

        // Only the area was saved: half the frame's width and height.
        let png = png::Decoder::new(std::io::BufReader::new(
            File::open(recording.join("000001.png")).unwrap(),
        ))
        .read_info()
        .unwrap();
        assert_eq!((png.info().width, png.info().height), (32, 18));
    }

    #[test]
    fn the_whole_window_is_saved_only_when_asked_for() {
        let temp = TempDir::new("dump-window");
        let mut recorder = Recorder::new(
            Some(Target {
                dir: temp.0.clone(),
                whole_window: true,
            }),
            None,
        );
        recorder.frame(&frame(10), None, at(0));
        drop(recorder);

        let recording = temp.recording();
        let frames =
            parse_frames(&fs::read_to_string(recording.join("frames.csv")).unwrap()).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].area, CropArea::WHOLE);
        let png = png::Decoder::new(std::io::BufReader::new(
            File::open(recording.join("000001.png")).unwrap(),
        ))
        .read_info()
        .unwrap();
        assert_eq!((png.info().width, png.info().height), (64, 36));
    }
}
