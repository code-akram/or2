//! `cargo xtask demo-gif`: the README's demo GIF (`docs/media/or2-demo.gif`) from a recording of
//! the app's `ReadmeDemoDeviceTest` (see docs/build.md, "README demo").
//!
//! The recording is the test's `files/demo/` pulled from a phone: `frames/` (JPEGs and
//! `frames.txt`, each frame's time in milliseconds) and `timeline.json` (the screen's size and
//! system bars, the frames' width, and the scenes and touches on the same clock). The GIF puts the
//! phone's screen, cropped to the app and rounded, in a bezel on the left of an 840 × 740 card, and
//! the storyboard on the right: the six scene captions, the current one lit. Every touch is drawn as
//! a soft finger mark where and when it landed.
//!
//! The work is done by ImageMagick (`magick`: the card, the captions, the masks) and FFmpeg (the
//! timing, the composition, the GIF's palette), with Noto Sans found through fontconfig. The
//! recording is not in the repository (it comes from a device), so the GIF cannot be regenerated as
//! a check; `--check` checks the checked-in GIF instead: its size on the card, its byte budget, and
//! that the README shows it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{Result, fail, repo_root};

const USAGE: &str =
    "usage: cargo xtask demo-gif <recording-dir> [--out <file.gif>] [--fps <n>] | --check";

/// The card.
const WIDTH: u32 = 840;
const HEIGHT: u32 = 740;
/// The phone's screen on the card: its height, top-left corner and corner radius.
const PHONE_HEIGHT: u32 = 680;
const PHONE_X: u32 = 64;
const PHONE_Y: u32 = 30;
const SCREEN_RADIUS: u32 = 27;
/// The storyboard's left edge, its first step's baseline and the step pitch.
const PANEL_X: u32 = 452;
const STEP_Y: u32 = 262;
const STEP_PITCH: u32 = 62;
/// A caption changes this long before its scene's first touch, so it is read as the step happens.
const CAPTION_LEAD: f64 = 0.35;
/// The finger mark's image is this many pixels square, the mark centred in it.
const MARK: u32 = 64;
/// The checked-in GIF's budget: GitHub shows larger images, but slowly.
const BUDGET_BYTES: u64 = 8 * 1024 * 1024;

const GIF: &str = "docs/media/or2-demo.gif";

/// Catppuccin Mocha, as the app's theme (`ui/Theme.kt`).
const BASE: &str = "#181825";
const GLOW: &str = "#24263a";
const TEXT: &str = "#cdd6f4";
const MUTED: &str = "#9399b2";
const SUBTLE: &str = "#6c7086";
const DIM: &str = "#45475a";
const ACCENT: &str = "#89b4fa";
const CRUST: &str = "#11111b";

#[derive(Debug, PartialEq)]
struct Options {
    recording: Option<PathBuf>,
    out: PathBuf,
    fps: u32,
    check: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut options = Options {
        recording: None,
        out: repo_root().join(GIF),
        fps: 20,
        check: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value; {USAGE}"))
        };
        match arg.as_str() {
            "--out" => options.out = PathBuf::from(value("--out")?),
            "--fps" => {
                let fps = value("--fps")?;
                options.fps = fps
                    .parse()
                    .ok()
                    .filter(|f| (5..=50).contains(f))
                    .ok_or(format!("--fps {fps}: 5 to 50"))?;
            }
            "--check" => options.check = true,
            other if !other.starts_with('-') && options.recording.is_none() => {
                options.recording = Some(PathBuf::from(other))
            }
            other => return fail(format!("unknown argument {other:?}; {USAGE}")),
        }
    }
    if options.check == options.recording.is_some() {
        return fail(USAGE);
    }
    Ok(options)
}

pub fn run(args: &[String]) -> Result<()> {
    let options = parse(args)?;
    match &options.recording {
        None => check(&repo_root()),
        Some(recording) => render(recording, &options.out, options.fps),
    }
}

// --- the recording ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Screen {
    width: f64,
    height: f64,
    status_bar: f64,
    navigation_bar: f64,
}

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Scene {
        t: f64,
        caption: String,
    },
    Tap {
        t: f64,
        x: f64,
        y: f64,
    },
    Drag {
        t: f64,
        x: f64,
        y: f64,
        dy: f64,
        ms: f64,
    },
}

#[derive(Debug, PartialEq)]
struct Timeline {
    screen: Screen,
    frame_width: f64,
    events: Vec<Event>,
}

fn number(value: &Value, key: &str) -> Result<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .ok_or(format!("timeline.json: {key:?} is missing or not a number"))
}

fn parse_timeline(text: &str) -> Result<Timeline> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("timeline.json: {e}"))?;
    let screen = root.get("screen").ok_or("timeline.json: no \"screen\"")?;
    let screen = Screen {
        width: number(screen, "width")?,
        height: number(screen, "height")?,
        status_bar: number(screen, "statusBar")?,
        navigation_bar: number(screen, "navigationBar")?,
    };
    let mut events = Vec::new();
    for event in root
        .get("events")
        .and_then(Value::as_array)
        .ok_or("timeline.json: no \"events\"")?
    {
        let t = number(event, "t")?;
        events.push(match event.get("kind").and_then(Value::as_str) {
            Some("scene") => Event::Scene {
                t,
                caption: event
                    .get("caption")
                    .and_then(Value::as_str)
                    .ok_or("timeline.json: a scene has no caption")?
                    .to_owned(),
            },
            Some("tap") => Event::Tap {
                t,
                x: number(event, "x")?,
                y: number(event, "y")?,
            },
            Some("drag") => Event::Drag {
                t,
                x: number(event, "x")?,
                y: number(event, "y")?,
                dy: number(event, "dy")?,
                ms: number(event, "ms")?,
            },
            other => return fail(format!("timeline.json: unknown event kind {other:?}")),
        });
    }
    if !events.iter().any(|e| matches!(e, Event::Scene { .. })) {
        return fail("timeline.json: no scenes");
    }
    Ok(Timeline {
        screen,
        frame_width: number(&root, "frameWidth")?,
        events,
    })
}

/// `frames.txt`: one `index time_ms` per line, in order.
fn parse_frames(text: &str) -> Result<Vec<(String, f64)>> {
    let frames: Vec<(String, f64)> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.split_whitespace();
            let index = parts.next().unwrap_or_default().to_owned();
            let time = parts
                .next()
                .and_then(|t| t.parse().ok())
                .ok_or(format!("frames.txt: bad line {line:?}"))?;
            Ok((index, time))
        })
        .collect::<Result<_>>()?;
    if frames.len() < 2 || frames.windows(2).any(|pair| pair[1].1 < pair[0].1) {
        return fail("frames.txt: needs two or more frames in time order");
    }
    Ok(frames)
}

// --- the layout ------------------------------------------------------------------------------

/// Where the recording lands on the card.
#[derive(Debug, PartialEq)]
struct Layout {
    /// The frames' crop: the app between the system bars, in frame pixels.
    crop_top: u32,
    crop_height: u32,
    frame_width: u32,
    /// The screen on the card.
    phone_width: u32,
    /// Screen pixels (the touches') to frame pixels.
    frame_scale: f64,
}

impl Layout {
    fn of(timeline: &Timeline) -> Result<Layout> {
        let screen = &timeline.screen;
        let frame_scale = timeline.frame_width / screen.width;
        let crop_top = (screen.status_bar * frame_scale).round() as u32;
        let crop_height = ((screen.height - screen.status_bar - screen.navigation_bar)
            * frame_scale)
            .round() as u32;
        if crop_height < 100 || timeline.frame_width < 100.0 {
            return fail("timeline.json: the screen is too small");
        }
        // Even sizes: FFmpeg's scalers and the GIF encoder both prefer them.
        let phone_width = ((PHONE_HEIGHT as f64 * timeline.frame_width / crop_height as f64) / 2.0)
            .round() as u32
            * 2;
        Ok(Layout {
            crop_top,
            crop_height,
            frame_width: timeline.frame_width as u32,
            phone_width,
            frame_scale,
        })
    }

    /// A touch at screen pixel (`x`, `y`) on the card.
    fn card(&self, x: f64, y: f64) -> (f64, f64) {
        let scale = PHONE_HEIGHT as f64 / self.crop_height as f64;
        (
            PHONE_X as f64 + x * self.frame_scale * scale,
            PHONE_Y as f64 + (y * self.frame_scale - self.crop_top as f64) * scale,
        )
    }

    /// A distance of `dy` screen pixels on the card.
    fn card_dy(&self, dy: f64) -> f64 {
        dy * self.frame_scale * PHONE_HEIGHT as f64 / self.crop_height as f64
    }
}

/// The scenes as captions with the seconds they show, on the GIF's clock (its first frame is 0).
fn scenes(events: &[Event], start_ms: f64, end: f64) -> Vec<(String, f64, f64)> {
    let starts: Vec<(String, f64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Scene { t, caption } => Some((caption.clone(), (t - start_ms) / 1000.0)),
            _ => None,
        })
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, (caption, t))| {
            let from = if i == 0 {
                0.0
            } else {
                (t - CAPTION_LEAD).max(0.0)
            };
            let to = starts
                .get(i + 1)
                .map_or(end, |(_, next)| (next - CAPTION_LEAD).max(0.0));
            (caption.clone(), from, to)
        })
        .collect()
}

/// The concat list that plays the frames at their recorded times, from the first scene on.
fn concat_list(frames: &[(String, f64)], dir: &Path, start_ms: f64) -> (String, f64) {
    // The last frame at or before the first scene: the GIF starts there.
    let first = frames
        .iter()
        .rposition(|(_, t)| *t <= start_ms)
        .unwrap_or(0);
    let shown = &frames[first..];
    let mut list = String::new();
    for (i, (index, t)) in shown.iter().enumerate() {
        let duration = shown
            .get(i + 1)
            .map_or(0.05, |(_, next)| (next - t) / 1000.0);
        let _ = writeln!(
            list,
            "file '{}'\nduration {duration:.3}",
            dir.join(format!("{index}.jpg")).display()
        );
    }
    // The concat demuxer takes the last entry's duration from a repeat of it.
    if let Some((index, _)) = shown.last() {
        let _ = writeln!(
            list,
            "file '{}'",
            dir.join(format!("{index}.jpg")).display()
        );
    }
    (list, shown[0].1)
}

/// The FFmpeg filter graph. Inputs: 0 the frames, 1 the card, 2 the screen mask, then one per scene
/// (its storyboard), then one finger mark per touch.
fn filter_graph(
    timeline: &Timeline,
    layout: &Layout,
    fps: u32,
    start_ms: f64,
    duration: f64,
) -> String {
    let mut graph = String::new();
    let _ = write!(
        graph,
        "[0:v]fps={fps},crop={}:{}:0:{},scale={}:{PHONE_HEIGHT}:flags=lanczos,format=rgba[phone];\
         [2:v]format=gray[mask];[phone][mask]alphamerge[screen];\
         [1:v][screen]overlay={PHONE_X}:{PHONE_Y}:shortest=1[v0];",
        layout.frame_width, layout.crop_height, layout.crop_top, layout.phone_width,
    );
    let mut last = "v0".to_owned();
    let scenes = scenes(&timeline.events, start_ms, duration);
    for (i, (_, from, to)) in scenes.iter().enumerate() {
        let _ = write!(
            graph,
            "[{input}:v]format=rgba[p{i}];[{last}][p{i}]overlay=0:0:shortest=1:enable='between(t,{from:.3},{to:.3})'[s{i}];",
            input = 3 + i,
        );
        last = format!("s{i}");
    }
    let mut input = 3 + scenes.len();
    let half = MARK as f64 / 2.0;
    for (i, event) in timeline.events.iter().enumerate() {
        let (t, x, y, dy, ms) = match *event {
            Event::Tap { t, x, y } => (t, x, y, 0.0, 0.0),
            Event::Drag { t, x, y, dy, ms } => (t, x, y, dy, ms),
            Event::Scene { .. } => continue,
        };
        let t = (t - start_ms) / 1000.0;
        let (cx, cy) = layout.card(x, y);
        let shown = t - 0.12;
        let fade = t + 0.25 + ms / 1000.0;
        let gone = fade + 0.32;
        let y_expr = if ms > 0.0 {
            // The finger follows the drag's ease-out.
            format!(
                "{:.2}+{:.2}*(1-pow(1-min(1,max(0,(t-{t:.3})/{:.3})),2))",
                cy - half,
                layout.card_dy(dy),
                ms / 1000.0
            )
        } else {
            format!("{:.2}", cy - half)
        };
        let _ = write!(
            graph,
            "[{input}:v]format=rgba,fade=t=in:st={shown:.3}:d=0.08:alpha=1,fade=t=out:st={fade:.3}:d=0.3:alpha=1[m{i}];\
             [{last}][m{i}]overlay=x={:.2}:y='{y_expr}':eval=frame:shortest=1:enable='between(t,{shown:.3},{gone:.3})'[t{i}];",
            cx - half,
        );
        last = format!("t{i}");
        input += 1;
    }
    let _ = write!(
        graph,
        "[{last}]trim=duration={duration:.3},split[a][b];[a]palettegen=max_colors=256:stats_mode=full[palette];\
         [b][palette]paletteuse=dither=sierra2_4a:diff_mode=rectangle"
    );
    graph
}

// --- rendering -------------------------------------------------------------------------------

struct Fonts {
    regular: String,
    medium: String,
    bold: String,
    mono: String,
}

/// Noto Sans through fontconfig; a fallback to another family would change the look, so it is an error.
fn fonts() -> Result<Fonts> {
    let find = |pattern: &str, family: &str| -> Result<String> {
        let output = Command::new("fc-match")
            .args(["-f", "%{family}\n%{file}", pattern])
            .output()
            .map_err(|e| format!("fc-match (fontconfig) is needed: {e}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let (found, file) = (
            lines.next().unwrap_or_default(),
            lines.next().unwrap_or_default(),
        );
        if !found.split(',').any(|name| name == family) || file.is_empty() {
            return fail(format!(
                "the {pattern:?} font is needed (fontconfig found {found:?}); install Noto Sans"
            ));
        }
        Ok(file.to_owned())
    };
    Ok(Fonts {
        regular: find("Noto Sans:style=Regular", "Noto Sans")?,
        medium: find("Noto Sans:style=Medium", "Noto Sans")?,
        bold: find("Noto Sans:style=Bold", "Noto Sans")?,
        mono: find("Noto Sans Mono:style=Regular", "Noto Sans Mono")?,
    })
}

fn run_tool(program: &str, args: &[String]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program} is needed: {e}"))?;
    if !output.status.success() {
        return fail(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// The card: the background, its glow, and the phone's shadow and bezel.
fn card_args(layout: &Layout, out: &Path) -> Vec<String> {
    let (x0, y0) = (PHONE_X - 8, PHONE_Y - 8);
    let (x1, y1) = (PHONE_X + layout.phone_width + 8, PHONE_Y + PHONE_HEIGHT + 8);
    let mut args = strings(&["-size", &format!("{WIDTH}x{HEIGHT}"), &format!("xc:{BASE}")]);
    args.extend(strings(&[
        "(",
        "-size",
        &format!("{WIDTH}x{HEIGHT}"),
        &format!("radial-gradient:{GLOW}-{BASE}"),
        "-resize",
        "160%x100%",
        "-gravity",
        "northwest",
        "-crop",
        &format!("{WIDTH}x{HEIGHT}+0+0"),
        "+repage",
        ")",
        "-compose",
        "lighten",
        "-composite",
        "(",
        "-size",
        &format!("{WIDTH}x{HEIGHT}"),
        "xc:none",
        "-fill",
        "rgba(0,0,0,0.65)",
        "-draw",
        &format!(
            "roundrectangle {},{} {},{} 40,40",
            x0 + 4,
            y0 + 14,
            x1 + 4,
            y1 + 14
        ),
        "-blur",
        "0x18",
        ")",
        "-compose",
        "over",
        "-composite",
        "-fill",
        "#0c0c13",
        "-stroke",
        "#34354a",
        "-strokewidth",
        "1.5",
        "-draw",
        &format!("roundrectangle {x0},{y0} {x1},{y1} 36,36"),
    ]));
    args.push(out.display().to_string());
    args
}

/// One storyboard: the name, the line under it, the scene captions with `active` lit (0-based).
fn panel_args(captions: &[String], active: usize, fonts: &Fonts, out: &Path) -> Vec<String> {
    let mut args = strings(&[
        "-size",
        &format!("{WIDTH}x{HEIGHT}"),
        "xc:none",
        "-stroke",
        "none",
    ]);
    let mut text = |font: &str, size: u32, color: &str, x: u32, y: u32, words: &str| {
        args.extend(strings(&[
            "-font",
            font,
            "-pointsize",
            &size.to_string(),
            "-fill",
            color,
            "-annotate",
            &format!("+{x}+{y}"),
            words,
        ]));
    };
    text(&fonts.bold, 54, TEXT, PANEL_X, 122, "or2");
    text(
        &fonts.regular,
        18,
        MUTED,
        PANEL_X,
        162,
        "An Android SSH and mosh client for the",
    );
    text(
        &fonts.regular,
        18,
        MUTED,
        PANEL_X,
        187,
        "coding agents on your own machines",
    );
    text(
        &fonts.mono,
        14,
        SUBTLE,
        PANEL_X,
        HEIGHT - 44,
        "ssh · mosh · tmux · herdr · android",
    );
    for (i, caption) in captions.iter().enumerate() {
        let y = STEP_Y + i as u32 * STEP_PITCH;
        let (cx, cy) = (PANEL_X + 13, y - 7);
        let number = (i + 1).to_string();
        if i == active {
            args.extend(strings(&[
                "-fill",
                ACCENT,
                "-draw",
                &format!("circle {cx},{cy} {cx},{}", cy - 13),
            ]));
            args.extend(strings(&[
                "-draw",
                &format!(
                    "roundrectangle {},{} {},{} 1.5,1.5",
                    PANEL_X + 42,
                    y + 14,
                    PANEL_X + 362,
                    y + 17
                ),
            ]));
            args.extend(strings(&[
                "-font",
                &fonts.bold,
                "-pointsize",
                "14",
                "-fill",
                CRUST,
                "-annotate",
                &format!("+{}+{}", cx - 4, cy + 5),
                &number,
            ]));
            args.extend(strings(&[
                "-font",
                &fonts.medium,
                "-pointsize",
                "21",
                "-fill",
                TEXT,
                "-annotate",
                &format!("+{}+{y}", PANEL_X + 42),
                caption,
            ]));
        } else {
            // Steps already shown stay readable; the ones to come are dimmer.
            let (ring, color) = if i < active {
                (SUBTLE, MUTED)
            } else {
                (DIM, SUBTLE)
            };
            args.extend(strings(&[
                "-fill",
                "none",
                "-stroke",
                ring,
                "-strokewidth",
                "1.5",
                "-draw",
                &format!("circle {cx},{cy} {cx},{}", cy - 12),
                "-stroke",
                "none",
            ]));
            args.extend(strings(&[
                "-font",
                &fonts.medium,
                "-pointsize",
                "14",
                "-fill",
                color,
                "-annotate",
                &format!("+{}+{}", cx - 4, cy + 5),
                &number,
            ]));
            args.extend(strings(&[
                "-font",
                &fonts.regular,
                "-pointsize",
                "20",
                "-fill",
                color,
                "-annotate",
                &format!("+{}+{y}", PANEL_X + 42),
                caption,
            ]));
        }
    }
    args.push(out.display().to_string());
    args
}

fn render(recording: &Path, out: &Path, fps: u32) -> Result<()> {
    let read = |name: &str| {
        std::fs::read_to_string(recording.join(name))
            .map_err(|e| format!("{}: {e}", recording.join(name).display()))
    };
    let timeline = parse_timeline(&read("timeline.json")?)?;
    let frames = parse_frames(&read("frames/frames.txt")?)?;
    let layout = Layout::of(&timeline)?;
    let fonts = fonts()?;
    let start_ms = timeline.events.iter().find_map(|e| match e {
        Event::Scene { t, .. } => Some(*t),
        _ => None,
    });
    let start_ms = start_ms.unwrap_or(frames[0].1);
    let frames_dir =
        std::fs::canonicalize(recording.join("frames")).map_err(|e| format!("frames/: {e}"))?;
    let (list, first_ms) = concat_list(&frames, &frames_dir, start_ms);
    // The GIF's clock starts at its first frame.
    let duration = (frames.last().map_or(0.0, |f| f.1) - first_ms) / 1000.0;

    let work = std::env::temp_dir().join(format!("or2-demo-gif-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let path = |name: &str| work.join(name);
    std::fs::write(path("frames.txt"), list).map_err(|e| e.to_string())?;
    run_tool("magick", &card_args(&layout, &path("card.png")))?;
    run_tool(
        "magick",
        &strings(&[
            "-size",
            &format!("{}x{PHONE_HEIGHT}", layout.phone_width),
            "xc:black",
            "-fill",
            "white",
            "-draw",
            &format!(
                "roundrectangle 0,0 {},{} {SCREEN_RADIUS},{SCREEN_RADIUS}",
                layout.phone_width - 1,
                PHONE_HEIGHT - 1
            ),
            &path("mask.png").display().to_string(),
        ]),
    )?;
    let c = MARK / 2;
    run_tool(
        "magick",
        &strings(&[
            "-size",
            &format!("{MARK}x{MARK}"),
            "xc:none",
            "-fill",
            "rgba(255,255,255,0.30)",
            "-stroke",
            "rgba(255,255,255,0.90)",
            "-strokewidth",
            "2.5",
            "-draw",
            &format!("circle {c},{c} {c},{}", c - 20),
            &path("mark.png").display().to_string(),
        ]),
    )?;
    let captions: Vec<String> = timeline
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Scene { caption, .. } => Some(caption.clone()),
            _ => None,
        })
        .collect();
    for active in 0..captions.len() {
        run_tool(
            "magick",
            &panel_args(
                &captions,
                active,
                &fonts,
                &path(&format!("panel{active}.png")),
            ),
        )?;
    }

    let graph = filter_graph(&timeline, &layout, fps, first_ms, duration);
    std::fs::write(path("graph.txt"), &graph).map_err(|e| e.to_string())?;
    let looped = |file: PathBuf| {
        strings(&[
            "-loop",
            "1",
            "-framerate",
            &fps.to_string(),
            "-i",
            &file.display().to_string(),
        ])
    };
    let mut args = strings(&[
        "-v",
        "error",
        "-y",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        &path("frames.txt").display().to_string(),
    ]);
    args.extend(looped(path("card.png")));
    args.extend(looped(path("mask.png")));
    for active in 0..captions.len() {
        args.extend(looped(path(&format!("panel{active}.png"))));
    }
    let touches = timeline
        .events
        .iter()
        .filter(|e| !matches!(e, Event::Scene { .. }))
        .count();
    for _ in 0..touches {
        args.extend(looped(path("mark.png")));
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    args.extend(strings(&[
        "-/filter_complex",
        &path("graph.txt").display().to_string(),
        "-loop",
        "0",
        &out.display().to_string(),
    ]));
    run_tool("ffmpeg", &args)?;
    let _ = std::fs::remove_dir_all(&work);
    let bytes = std::fs::metadata(out).map_err(|e| e.to_string())?.len();
    println!(
        "{}: {WIDTH}x{HEIGHT}, {duration:.1} s at {fps} fps, {:.1} MiB",
        out.display(),
        bytes as f64 / 1048576.0
    );
    if bytes > BUDGET_BYTES {
        return fail(format!(
            "{} is over the {} MiB budget: lower --fps",
            out.display(),
            BUDGET_BYTES / 1048576
        ));
    }
    Ok(())
}

// --- the check -------------------------------------------------------------------------------

/// The checked-in GIF: a GIF89a on the card's size, within budget, and shown by the README.
fn check(root: &Path) -> Result<()> {
    let gif = root.join(GIF);
    let bytes = std::fs::read(&gif).map_err(|e| format!("{GIF}: {e}"))?;
    check_gif(&bytes)?;
    let readme =
        std::fs::read_to_string(root.join("README.md")).map_err(|e| format!("README.md: {e}"))?;
    if !readme.contains(GIF) {
        return fail(format!("README.md does not show {GIF}"));
    }
    println!("{GIF}: ok ({:.1} MiB)", bytes.len() as f64 / 1048576.0);
    Ok(())
}

fn check_gif(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 13 || &bytes[..6] != b"GIF89a" {
        return fail(format!("{GIF} is not a GIF89a"));
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    if (width, height) != (WIDTH, HEIGHT) {
        return fail(format!("{GIF} is {width}x{height}, not {WIDTH}x{HEIGHT}"));
    }
    if bytes.len() as u64 > BUDGET_BYTES {
        return fail(format!(
            "{GIF} is over the {} MiB budget",
            BUDGET_BYTES / 1048576
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMELINE: &str = r#"{
      "screen": {"width": 1440, "height": 3216, "statusBar": 131, "navigationBar": 85},
      "frameWidth": 1080,
      "events": [
        {"t": 400, "kind": "scene", "index": 1, "caption": "One"},
        {"t": 3300, "kind": "scene", "index": 2, "caption": "Two"},
        {"t": 3302, "kind": "tap", "x": 720, "y": 1608},
        {"t": 5000, "kind": "drag", "x": 100, "y": 1000, "dy": 1400, "ms": 260}
      ]
    }"#;

    #[test]
    fn the_timeline_is_read_with_every_event() {
        let timeline = parse_timeline(TIMELINE).unwrap();
        assert_eq!(
            timeline.screen,
            Screen {
                width: 1440.0,
                height: 3216.0,
                status_bar: 131.0,
                navigation_bar: 85.0
            }
        );
        assert_eq!(timeline.frame_width, 1080.0);
        assert_eq!(timeline.events.len(), 4);
        assert_eq!(
            timeline.events[2],
            Event::Tap {
                t: 3302.0,
                x: 720.0,
                y: 1608.0
            }
        );
        assert!(parse_timeline(r#"{"screen": {}, "frameWidth": 1, "events": []}"#).is_err());
        assert!(
            parse_timeline(&TIMELINE.replace("\"tap\"", "\"swipe\""))
                .unwrap_err()
                .contains("swipe")
        );
    }

    #[test]
    fn the_layout_crops_the_system_bars_and_maps_touches_onto_the_screen() {
        let layout = Layout::of(&parse_timeline(TIMELINE).unwrap()).unwrap();
        assert_eq!(layout.crop_top, 98); // 131 × 0.75
        assert_eq!(layout.crop_height, 2250); // (3216 − 131 − 85) × 0.75
        assert_eq!(layout.phone_width, 326);
        // The screen's centre column lands on the phone's centre column; the status bar's foot on its top.
        let (x, y) = layout.card(720.0, 131.0);
        assert!((x - (PHONE_X as f64 + 1080.0 / 2.0 * PHONE_HEIGHT as f64 / 2250.0)).abs() < 0.01);
        assert!((y - PHONE_Y as f64).abs() < 0.2);
    }

    #[test]
    fn captions_change_just_before_their_scene_and_the_last_runs_to_the_end() {
        let timeline = parse_timeline(TIMELINE).unwrap();
        let shown = scenes(&timeline.events, 400.0, 10.0);
        assert_eq!(shown[0], ("One".to_owned(), 0.0, 2.9 - CAPTION_LEAD));
        assert_eq!(shown[1], ("Two".to_owned(), 2.9 - CAPTION_LEAD, 10.0));
    }

    #[test]
    fn the_frames_play_at_their_recorded_times_from_the_first_scene() {
        let frames = parse_frames("00000 0\n00001 300\n00002 450\n00003 500\n").unwrap();
        let (list, first) = concat_list(&frames, Path::new("/f"), 400.0);
        assert_eq!(first, 300.0);
        assert_eq!(
            list,
            "file '/f/00001.jpg'\nduration 0.150\nfile '/f/00002.jpg'\nduration 0.050\nfile '/f/00003.jpg'\nduration 0.050\nfile '/f/00003.jpg'\n"
        );
        assert!(parse_frames("00000 10\n00001 5\n").is_err());
    }

    #[test]
    fn the_graph_has_one_storyboard_per_scene_and_one_mark_per_touch() {
        let timeline = parse_timeline(TIMELINE).unwrap();
        let layout = Layout::of(&timeline).unwrap();
        let graph = filter_graph(&timeline, &layout, 20, 400.0, 12.0);
        assert!(graph.starts_with("[0:v]fps=20,crop=1080:2250:0:98,scale=326:680"));
        assert!(graph.contains("[3:v]format=rgba[p0]") && graph.contains("[4:v]format=rgba[p1]"));
        assert!(graph.contains("[5:v]format=rgba,fade") && graph.contains("[6:v]format=rgba,fade"));
        assert!(graph.contains("pow(1-min(1,max(0,(t-4.600)/0.260)),2)")); // The drag's finger moves.
        assert!(graph.ends_with("paletteuse=dither=sierra2_4a:diff_mode=rectangle"));
    }

    #[test]
    fn the_check_wants_a_gif_on_the_card_size_within_budget() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend(840u16.to_le_bytes());
        gif.extend(740u16.to_le_bytes());
        gif.extend([0; 8]);
        assert!(check_gif(&gif).is_ok());
        assert!(check_gif(b"GIF87a\x48\x03\xe4\x02\0\0\0").is_err());
        let mut small = gif.clone();
        small[6] = 0;
        assert!(check_gif(&small).unwrap_err().contains("not 840x740"));
    }

    #[test]
    fn options_take_a_recording_or_check() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--check".into(), "dir".into()]).is_err());
        let options = parse(&["dir".into(), "--fps".into(), "15".into()]).unwrap();
        assert_eq!(
            (options.recording, options.fps, options.check),
            (Some(PathBuf::from("dir")), 15, false)
        );
        assert!(parse(&["dir".into(), "--fps".into(), "90".into()]).is_err());
    }
}
