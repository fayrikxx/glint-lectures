use std::env;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use glint_core::dump_format::{self, Event, FrameRow};
use glint_core::replay::{Replay, ReplayReport, Step};
use glint_core::{AnalysisState, BgraFrame, RgbaImage};

/// Runs a capture recorded with `GLINT_DUMP_DIR` through the same analysis the app uses
/// and prints numbers about it, nothing that was on screen. Every frame gets shrunk
/// twice, so run it in release:
///
/// `cargo run -p glint-core --example replay --release -- <recording> [--debug]`
///
/// The numbers also go to `_out\report.csv` in the recording. `--debug` saves the analysis
/// picture of every frame that changed into `_out\debug`, with the changed blocks tinted.
fn main() -> ExitCode {
    let mut recording = None;
    let mut debug = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--debug" => debug = true,
            _ => recording = Some(PathBuf::from(arg)),
        }
    }
    let Some(recording) = recording else {
        eprintln!("usage: replay <recording folder> [--debug]");
        return ExitCode::FAILURE;
    };
    match run(&recording, debug) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("replay failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(recording: &Path, debug: bool) -> Result<(), String> {
    let frames = dump_format::parse_frames(&read(&recording.join("frames.csv"))?)
        .map_err(|err| format!("frames.csv, {err}"))?;
    let events = dump_format::parse_events(&read(&recording.join("events.csv"))?)
        .map_err(|err| format!("events.csv, {err}"))?;
    let out = recording.join("_out");
    let debug_out = out.join("debug");
    let made = if debug { &debug_out } else { &out };
    fs::create_dir_all(made).map_err(|err| format!("{}: {err}", made.display()))?;

    let mut replay = Replay::new();
    let mut events = events.into_iter().peekable();
    let mut dropped = 0;
    let mut end = Duration::ZERO;
    for row in &frames {
        // The analysis thread pauses or resumes before it looks at a frame that came at
        // the same time, so events go first.
        while let Some(event) = events.next_if(|event| event.ms <= row.ms) {
            replay.step(Duration::from_millis(event.ms), step_for(event.event));
        }
        end = Duration::from_millis(row.ms);
        if !row.saved {
            dropped += 1;
            continue;
        }
        let frame = load(recording, row)?;
        let area = row.area;
        replay.step(
            end,
            Step::Frame {
                frame: &frame,
                area,
            },
        );
        if debug && let Some(image) = replay.changed_blocks_image() {
            save_png(&debug_out.join(row.image_name()), &image)?;
        }
    }
    for event in events {
        end = end.max(Duration::from_millis(event.ms));
        replay.step(Duration::from_millis(event.ms), step_for(event.event));
    }

    let report = replay.finish(end);
    print_report(&report, dropped);
    write_report(&out.join("report.csv"), &report, dropped)
}

fn step_for(event: Event) -> Step<'static> {
    match event {
        Event::Pause => Step::Pause,
        Event::Resume => Step::Resume,
    }
}

/// Reads a frame's PNG. A recording that only kept the crop area gets the area put back
/// into a window-sized frame, so the analysis sees what it saw live.
fn load(recording: &Path, row: &FrameRow) -> Result<BgraFrame, String> {
    let name = row.image_name();
    let file = File::open(recording.join(&name)).map_err(|err| format!("{name}: {err}"))?;
    let mut reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|err| format!("{name}: {err}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{name} is too big"))?;
    let mut rgb = vec![0; size];
    let info = reader
        .next_frame(&mut rgb)
        .map_err(|err| format!("{name}: {err}"))?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgb, png::BitDepth::Eight) {
        return Err(format!("{name} isn't an 8-bit RGB image"));
    }
    rgb.truncate(info.buffer_size());

    let image = dump_format::from_rgb(info.width, info.height, &rgb)
        .ok_or_else(|| format!("{name} has fewer pixels than its size says"))?;
    if (image.width(), image.height()) == (row.width, row.height) {
        return Ok(image);
    }
    dump_format::place_in_window(&image, row.width, row.height, row.area)
        .ok_or_else(|| format!("{name} doesn't match the area in frames.csv"))
}

fn save_png(path: &Path, image: &RgbaImage) -> Result<(), String> {
    let fail = |err: png::EncodingError| format!("{}: {err}", path.display());
    let file = File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&image.pixels).map_err(fail)?;
    writer.finish().map_err(fail)
}

fn print_report(report: &ReplayReport, dropped: usize) {
    let seconds = |time: Duration| time.as_secs_f64();
    println!(
        "frames: {} ({dropped} dropped while recording)",
        report.frames
    );
    println!("length: {:.1} s", seconds(report.end));
    if let (Some(median), Some(p95), Some(longest)) = (
        report.gap_percentile(0.5),
        report.gap_percentile(0.95),
        report.gap_percentile(1.0),
    ) {
        println!(
            "gaps between frames: median {:.2} s, p95 {:.2} s, longest {:.2} s",
            seconds(median),
            seconds(p95),
            seconds(longest),
        );
    }
    println!(
        "changing: {} times, {:.1} s in total",
        report.times_in(AnalysisState::Changing),
        seconds(report.time_in(AnalysisState::Changing)),
    );
    println!(
        "still: {:.1} s, paused: {:.1} s",
        seconds(report.time_in(AnalysisState::Still)),
        seconds(report.time_in(AnalysisState::Paused)),
    );
    match (
        report.noise_percentile(0.5),
        report.noise_percentile(0.99),
        report.noise_percentile(0.999),
    ) {
        (Some(p50), Some(p99), Some(p999)) => println!(
            "codec noise on still frames, biggest channel difference out of 255: \
             p50 {p50}, p99 {p99}, p99.9 {p999} ({} pixels)",
            report.noise_samples(),
        ),
        _ => println!("codec noise: no still frames to measure it on"),
    }
}

fn write_report(path: &Path, report: &ReplayReport, dropped: usize) -> Result<(), String> {
    let seconds = |time: Duration| format!("{:.3}", time.as_secs_f64());
    let mut rows = vec![
        ("frames", report.frames.to_string()),
        ("dropped_frames", dropped.to_string()),
        ("length_s", seconds(report.end)),
        (
            "changing_times",
            report.times_in(AnalysisState::Changing).to_string(),
        ),
        (
            "changing_s",
            seconds(report.time_in(AnalysisState::Changing)),
        ),
        ("still_s", seconds(report.time_in(AnalysisState::Still))),
        ("paused_s", seconds(report.time_in(AnalysisState::Paused))),
    ];
    for (name, fraction) in [("gap_p50_s", 0.5), ("gap_p95_s", 0.95), ("gap_max_s", 1.0)] {
        if let Some(gap) = report.gap_percentile(fraction) {
            rows.push((name, seconds(gap)));
        }
    }
    for (name, fraction) in [
        ("noise_p50", 0.5),
        ("noise_p99", 0.99),
        ("noise_p999", 0.999),
    ] {
        if let Some(noise) = report.noise_percentile(fraction) {
            rows.push((name, noise.to_string()));
        }
    }
    rows.push(("noise_pixels", report.noise_samples().to_string()));

    let lines: String = rows
        .iter()
        .map(|(name, value)| format!("{name},{value}\n"))
        .collect();
    fs::write(path, format!("metric,value\n{lines}"))
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))
}
