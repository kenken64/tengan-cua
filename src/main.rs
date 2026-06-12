use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use xcap::Monitor;

mod keys;
mod platform;
mod stake_agent;
mod windows;

use keys::KeySpec;
pub(crate) use platform::platform_name;
use windows::WindowInfo;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Cross-platform desktop control helper driven by Codex CLI vision"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List visible monitors and their desktop coordinate origins.
    Monitors,
    /// List visible application windows and their desktop bounds.
    Windows,
    /// Show the detected operating system and its keyboard shortcut conventions.
    OsInfo,
    /// Capture monitor screenshot PNG files.
    Capture {
        /// Monitor index from `monitors`, or omit to use the primary monitor.
        #[arg(long)]
        monitor: Option<usize>,
        /// Capture every visible monitor.
        #[arg(long)]
        all_monitors: bool,
        /// Output directory for captured screenshots.
        #[arg(long, default_value = "screenshots")]
        out_dir: PathBuf,
    },
    /// Record one monitor to an MP4 file using ffmpeg.
    RecordMonitor {
        /// Monitor index from `monitors`.
        #[arg(long)]
        monitor: usize,
        /// Stop after this many seconds. Omit to record until interrupted.
        #[arg(long)]
        seconds: Option<u64>,
        /// Output directory for generated recordings.
        #[arg(long, default_value = "runs/recordings")]
        out_dir: PathBuf,
        /// Explicit output MP4 path. Overrides --out-dir.
        #[arg(long)]
        output: Option<PathBuf>,
        /// ffmpeg executable to run.
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg_bin: String,
        /// Capture framerate.
        #[arg(long, default_value_t = 15)]
        framerate: u32,
        /// Do not draw the mouse cursor into the recording.
        #[arg(long)]
        no_mouse: bool,
        /// Overwrite the output file if it already exists.
        #[arg(long)]
        overwrite: bool,
        /// Extract sampled PNG frames to this directory after recording.
        #[arg(long)]
        frames_dir: Option<PathBuf>,
        /// Frame sampling rate used with --frames-dir.
        #[arg(long, default_value_t = 0.5)]
        frame_fps: f32,
    },
    /// Extract sampled PNG frames from an MP4 using ffmpeg.
    ExtractFrames {
        /// Source MP4 file.
        #[arg(long)]
        input: PathBuf,
        /// Output directory for PNG frames.
        #[arg(long)]
        out_dir: PathBuf,
        /// Frame sampling rate, for example 0.5 means one frame every two seconds.
        #[arg(long, default_value_t = 0.5)]
        fps: f32,
        /// ffmpeg executable to run.
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg_bin: String,
        /// Overwrite existing frame files.
        #[arg(long)]
        overwrite: bool,
    },
    /// Capture screenshots and send them to Codex CLI with --image.
    AskCodex {
        /// Natural language desktop instruction, for example: "click the Save button".
        instruction: String,
        /// Monitor index from `monitors`, or omit to use the primary monitor.
        #[arg(long)]
        monitor: Option<usize>,
        /// Capture every visible monitor and attach all screenshots to Codex.
        #[arg(long)]
        all_monitors: bool,
        /// Output directory for screenshots and Codex response JSON.
        #[arg(long, default_value = "runs")]
        out_dir: PathBuf,
        /// Codex executable to run. Defaults to codex.cmd on Windows and codex elsewhere.
        #[arg(long)]
        codex_bin: Option<String>,
        /// Actually execute the returned mouse/keyboard actions.
        #[arg(long)]
        execute: bool,
    },
    /// Run the Stake Blackjack/Tengan autonomous observe-act-verify loop.
    StakeAgent(stake_agent::StakeAgentArgs),
    /// Execute a JSON action plan previously produced by `ask-codex`.
    Execute {
        /// Path to the JSON action plan.
        plan: PathBuf,
        /// Add this desktop X origin to screenshot-relative coordinates.
        #[arg(long, default_value_t = 0)]
        origin_x: i32,
        /// Add this desktop Y origin to screenshot-relative coordinates.
        #[arg(long, default_value_t = 0)]
        origin_y: i32,
        /// Divide screenshot X coordinates by this scale before adding origin-x.
        #[arg(long, default_value_t = 1.0)]
        scale_x: f32,
        /// Divide screenshot Y coordinates by this scale before adding origin-y.
        #[arg(long, default_value_t = 1.0)]
        scale_y: f32,
    },
    /// Move and click an absolute desktop coordinate.
    Click {
        x: i32,
        y: i32,
        #[arg(long, default_value = "left")]
        button: MouseButton,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ActionPlan {
    pub(crate) summary: String,
    pub(crate) confidence: f32,
    pub(crate) actions: Vec<Action>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Action {
    MoveMouse {
        #[serde(default)]
        monitor_index: Option<usize>,
        x: i32,
        y: i32,
    },
    Click {
        #[serde(default)]
        monitor_index: Option<usize>,
        x: i32,
        y: i32,
    },
    DoubleClick {
        #[serde(default)]
        monitor_index: Option<usize>,
        x: i32,
        y: i32,
    },
    RightClick {
        #[serde(default)]
        monitor_index: Option<usize>,
        x: i32,
        y: i32,
    },
    TypeText {
        #[serde(default)]
        monitor_index: Option<usize>,
        text: String,
    },
    PasteText {
        #[serde(default)]
        monitor_index: Option<usize>,
        text: String,
    },
    PressKey {
        #[serde(default)]
        monitor_index: Option<usize>,
        key: KeySpec,
    },
    FocusWindow {
        #[serde(default)]
        monitor_index: Option<usize>,
        window: String,
    },
    Scroll {
        #[serde(default)]
        monitor_index: Option<usize>,
        amount: i32,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
}

#[derive(Debug)]
pub(crate) struct CapturedScreen {
    pub(crate) path: PathBuf,
    pub(crate) monitor_index: usize,
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) desktop_width: u32,
    pub(crate) desktop_height: u32,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) monitor_name: String,
}

#[derive(Clone, Copy, Debug)]
struct CoordinateTransform {
    origin_x: i32,
    origin_y: i32,
    scale_x: f32,
    scale_y: f32,
}

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";

fn main() -> Result<()> {
    init_dpi_awareness();

    match Cli::parse().command {
        Commands::Monitors => list_monitors(),
        Commands::Windows => list_windows(),
        Commands::OsInfo => {
            print_os_info();
            Ok(())
        }
        Commands::Capture {
            monitor,
            all_monitors,
            out_dir,
        } => {
            let captures = capture_monitors(monitor, all_monitors, &out_dir)?;
            for capture in captures {
                print_capture(&capture);
            }
            Ok(())
        }
        Commands::RecordMonitor {
            monitor,
            seconds,
            out_dir,
            output,
            ffmpeg_bin,
            framerate,
            no_mouse,
            overwrite,
            frames_dir,
            frame_fps,
        } => record_monitor(
            monitor,
            seconds,
            &out_dir,
            output.as_deref(),
            &ffmpeg_bin,
            framerate,
            !no_mouse,
            overwrite,
            frames_dir.as_deref(),
            frame_fps,
        ),
        Commands::ExtractFrames {
            input,
            out_dir,
            fps,
            ffmpeg_bin,
            overwrite,
        } => extract_frames(&input, &out_dir, fps, &ffmpeg_bin, overwrite),
        Commands::AskCodex {
            instruction,
            monitor,
            all_monitors,
            out_dir,
            codex_bin,
            execute,
        } => {
            let codex_bin = codex_bin.unwrap_or_else(default_codex_bin);
            ask_codex(
                &codex_bin,
                &instruction,
                monitor,
                all_monitors,
                &out_dir,
                execute,
            )
        }
        Commands::StakeAgent(args) => stake_agent::run(args),
        Commands::Execute {
            plan,
            origin_x,
            origin_y,
            scale_x,
            scale_y,
        } => {
            let plan = read_plan(&plan)?;
            execute_plan(&plan, origin_x, origin_y, scale_x, scale_y)
        }
        Commands::Click { x, y, button } => click_at(x, y, button.into()),
    }
}

fn init_dpi_awareness() {
    #[cfg(target_os = "windows")]
    {
        let _ = enigo::set_dpi_awareness();
    }
}

pub(crate) fn default_codex_bin() -> String {
    if cfg!(target_os = "windows") {
        "codex.cmd".to_string()
    } else {
        "codex".to_string()
    }
}

fn print_os_info() {
    println!("platform={}", platform::platform_description());
    println!(
        "primary_shortcut_modifier={}",
        platform::primary_modifier_name()
    );
    println!(
        "key_mapping: cmd/primary -> {}, ctrl -> Control, alt/option -> {}, win/super/meta -> {}",
        platform::primary_modifier_name(),
        if cfg!(target_os = "macos") {
            "Option"
        } else {
            "Alt"
        },
        if cfg!(target_os = "macos") {
            "Command"
        } else {
            "Super/Windows key"
        },
    );
    println!(
        "semantic_shortcuts: select_all, copy, paste, cut, undo, redo, save, find, new_tab, close_tab map to the {} conventions",
        platform_name()
    );
}

fn list_windows() -> Result<()> {
    for window in windows::list_windows()? {
        println!(
            "app={:?} title={:?} desktop_bounds=({}, {}, {}x{}) focused={}",
            window.app,
            window.title,
            window.x,
            window.y,
            window.width,
            window.height,
            window.focused
        );
    }
    Ok(())
}

fn list_monitors() -> Result<()> {
    for (index, monitor) in Monitor::all()?.iter().enumerate() {
        println!(
            "#{index}: name={:?} primary={} origin=({}, {}) size={}x{} scale={}",
            monitor.friendly_name()?,
            monitor.is_primary()?,
            monitor.x()?,
            monitor.y()?,
            monitor.width()?,
            monitor.height()?,
            monitor.scale_factor()?
        );
    }
    Ok(())
}

fn record_monitor(
    monitor_index: usize,
    seconds: Option<u64>,
    out_dir: &Path,
    output: Option<&Path>,
    ffmpeg_bin: &str,
    framerate: u32,
    draw_mouse: bool,
    overwrite: bool,
    frames_dir: Option<&Path>,
    frame_fps: f32,
) -> Result<()> {
    if !cfg!(target_os = "windows") {
        bail!("record-monitor currently supports Windows ffmpeg gdigrab only");
    }
    if framerate == 0 {
        bail!("--framerate must be greater than 0");
    }
    if seconds == Some(0) {
        bail!("--seconds must be greater than 0 when provided");
    }

    let selected = select_monitors(Some(monitor_index), false)?
        .into_iter()
        .next()
        .context("selected monitor was not found")?;

    let monitor = selected.monitor;
    let monitor_name = monitor
        .friendly_name()
        .unwrap_or_else(|_| "monitor".to_string());
    let output_path = match output {
        Some(path) => path.to_path_buf(),
        None => {
            fs::create_dir_all(out_dir)
                .with_context(|| format!("failed to create {}", out_dir.display()))?;
            out_dir.join(format!(
                "monitor-{}-{}-{}.mp4",
                monitor_index,
                timestamp_millis()?,
                normalized_filename(&monitor_name)
            ))
        }
    };

    if output_path.exists() && !overwrite {
        bail!(
            "{} already exists; pass --overwrite or choose a different --output",
            output_path.display()
        );
    }
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let origin_x = monitor.x()?;
    let origin_y = monitor.y()?;
    let width = monitor.width()?;
    let height = monitor.height()?;
    let video_size = format!("{width}x{height}");

    println!(
        "recording monitor_index={} name={:?} origin=({}, {}) size={}x{} framerate={} mouse={} output={}",
        monitor_index,
        monitor_name,
        origin_x,
        origin_y,
        width,
        height,
        framerate,
        draw_mouse,
        output_path.display()
    );
    if let Some(seconds) = seconds {
        println!("duration_seconds={seconds}");
    } else {
        println!("duration_seconds=until-interrupted");
    }

    let mut command = Command::new(ffmpeg_bin);
    command
        .arg("-hide_banner")
        .arg(if overwrite { "-y" } else { "-n" })
        .arg("-f")
        .arg("gdigrab")
        .arg("-draw_mouse")
        .arg(if draw_mouse { "1" } else { "0" })
        .arg("-framerate")
        .arg(framerate.to_string())
        .arg("-offset_x")
        .arg(origin_x.to_string())
        .arg("-offset_y")
        .arg(origin_y.to_string())
        .arg("-video_size")
        .arg(video_size)
        .arg("-i")
        .arg("desktop");

    if let Some(seconds) = seconds {
        command.arg("-t").arg(seconds.to_string());
    }

    command
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("23")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg(&output_path);

    let status = command
        .status()
        .with_context(|| format!("failed to start `{ffmpeg_bin}`"))?;

    if !status.success() {
        bail!("ffmpeg exited with status {status}");
    }

    println!("recording_saved={}", output_path.display());
    if let Some(frames_dir) = frames_dir {
        extract_frames(&output_path, frames_dir, frame_fps, ffmpeg_bin, overwrite)?;
    }

    Ok(())
}

fn extract_frames(
    input: &Path,
    out_dir: &Path,
    fps: f32,
    ffmpeg_bin: &str,
    overwrite: bool,
) -> Result<()> {
    if !input.exists() {
        bail!("input video does not exist: {}", input.display());
    }
    if !fps.is_finite() || fps <= 0.0 {
        bail!("--fps must be greater than 0");
    }

    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    if !overwrite && has_existing_frame_files(out_dir)? {
        bail!(
            "{} already contains frame_*.png files; pass --overwrite or choose a different --out-dir",
            out_dir.display()
        );
    }

    let pattern = out_dir.join("frame_%05d.png");
    println!(
        "extracting_frames input={} fps={} output_pattern={}",
        input.display(),
        fps,
        pattern.display()
    );

    let status = Command::new(ffmpeg_bin)
        .arg("-hide_banner")
        .arg(if overwrite { "-y" } else { "-n" })
        .arg("-i")
        .arg(input)
        .arg("-vf")
        .arg(format!("fps={fps}"))
        .arg(&pattern)
        .status()
        .with_context(|| format!("failed to start `{ffmpeg_bin}`"))?;

    if !status.success() {
        bail!("ffmpeg exited with status {status}");
    }

    println!("frames_saved_dir={}", out_dir.display());
    Ok(())
}

fn has_existing_frame_files(out_dir: &Path) -> Result<bool> {
    for entry in
        fs::read_dir(out_dir).with_context(|| format!("failed to read {}", out_dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", out_dir.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("frame_") && name.ends_with(".png") {
            return Ok(true);
        }
    }

    Ok(false)
}

fn ask_codex(
    codex_bin: &str,
    instruction: &str,
    monitor_index: Option<usize>,
    all_monitors: bool,
    out_dir: &Path,
    execute: bool,
) -> Result<()> {
    let captures = capture_monitors(monitor_index, all_monitors, out_dir)?;
    let visible_windows = windows::list_windows().unwrap_or_else(|err| {
        transcript_notice(&format!("window listing unavailable: {err}"));
        Vec::new()
    });
    let response_path = out_dir.join(format!("codex-action-{}.json", timestamp_millis()?));
    let schema_path = PathBuf::from("schemas").join("codex_action.schema.json");
    let prompt = build_codex_prompt(instruction, &captures, &visible_windows);

    let mut command = Command::new(codex_bin);
    command.arg("exec").arg("--skip-git-repo-check");

    for capture in &captures {
        command.arg("--image").arg(&capture.path);
    }

    let mut child = command
        .arg("--output-schema")
        .arg(&schema_path)
        .arg("--output-last-message")
        .arg(&response_path)
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `{codex_bin}`"))?;

    {
        let mut stdin = child.stdin.take().context("failed to open Codex stdin")?;
        stdin
            .write_all(prompt.as_bytes())
            .context("failed to write prompt to Codex stdin")?;
    }

    let status = child
        .wait()
        .with_context(|| format!("failed to wait for `{codex_bin}`"))?;

    if !status.success() {
        bail!("Codex CLI exited with status {status}");
    }

    for capture in &captures {
        print_capture(capture);
    }
    println!("plan={}", response_path.display());

    let plan = read_plan(&response_path)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);

    if execute {
        execute_plan_for_captures(&plan, &captures)?;
    }

    Ok(())
}

fn build_codex_prompt(
    instruction: &str,
    captures: &[CapturedScreen],
    visible_windows: &[WindowInfo],
) -> String {
    let screenshots = captures
        .iter()
        .enumerate()
        .map(|(image_index, capture)| {
            format!(
                "Image {}: monitor_index={}, monitor_name={:?}, desktop_origin=({}, {}), desktop_size={}x{}, image_size={}x{}, file={}",
                image_index + 1,
                capture.monitor_index,
                capture.monitor_name,
                capture.origin_x,
                capture.origin_y,
                capture.desktop_width,
                capture.desktop_height,
                capture.image_width,
                capture.image_height,
                capture.path.display()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    let windows_text = if visible_windows.is_empty() {
        "unavailable".to_string()
    } else {
        visible_windows
            .iter()
            .map(|window| {
                format!(
                    "app={:?} title={:?} desktop_bounds=({}, {}, {}x{}) focused={}",
                    window.app,
                    window.title,
                    window.x,
                    window.y,
                    window.width,
                    window.height,
                    window.focused
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };

    format!(
        "You are controlling a {} desktop from attached screenshot images. \
The primary keyboard shortcut modifier on this operating system is {}. \
Return only a JSON action plan matching the schema. \
Use coordinates relative to the selected screenshot image pixels, not absolute desktop coordinates. \
For every coordinate action, set monitor_index to the monitor_index of the screenshot image used for that coordinate. \
For actions without coordinates, set monitor_index to null. \
For press_key, key accepts named keys (enter, escape, tab, backspace, delete, space, arrow_up, arrow_down, arrow_left, arrow_right, home, end, page_up, page_down, f1-f12), \
OS-aware shortcuts that map to the correct modifier on this operating system (select_all, copy, paste, cut, undo, redo, save, find, new_tab, close_tab), \
and modifier combos joined with + such as cmd+l, ctrl+c, alt+f4, or cmd+shift+t. \
In combos, cmd and primary mean Command on macOS and Control on Windows and Linux, while ctrl always means the literal Control key; \
use ctrl only for terminal control sequences and use cmd or the OS-aware shortcut names for ordinary application shortcuts. \
Visible windows: {}. \
Window desktop_bounds are absolute desktop coordinates; use each screenshot's desktop_origin and desktop_size to locate a window inside the screenshot image. \
If the user instruction names an application or window, first return a focus_window action whose window field is copied exactly from the matching visible window's title, or its app name when the title is empty or the whole application is meant, \
then target every later coordinate and keyboard action inside that window's desktop_bounds region of the screenshot. \
If the named application or window does not match any visible window, return an empty actions array and explain the mismatch in summary. \
For Enter/Return and other non-text keys, use press_key; never submit by using type_text with a newline. \
For terminal commands on macOS Terminal, iTerm, tmux, Codex, Claude, or shell panes, first click the exact target prompt, then clear any existing prompt draft with press_key control_a followed by press_key control_k, then use paste_text for the command text without a trailing newline, then use press_key enter if the user asked to run or submit it. \
For ordinary text fields, search boxes, chat inputs, forms, and editable UI controls where the user asks to replace the current text, first click the exact field, then use press_key select_all, then use paste_text for the new text without a trailing newline. \
Use paste_text instead of type_text for terminal commands, long text, or text containing shell punctuation; use type_text only for short ordinary text fields. \
Terminal targeting rules: when multiple Terminal, tmux, Codex, Claude, or shell windows/panes are visible, first list the visible candidates mentally by window title, active tab title, project path, prompt text, and frontmost/inactive visual state. \
Choose a terminal candidate only if it satisfies every user-specified anchor such as project name, path, window title, tab title, pane label, or visible prompt text. \
If the user says current, active, or front terminal, use the visually frontmost active terminal window; do not use a background or lower terminal just because it also matches the app type. \
If more than one candidate still matches, or if the requested project/path cannot be read, return an empty actions array and explain the ambiguity in summary. \
Ignore pasted prior-result prose inside the user instruction, especially sentences beginning with Targeted, Clicked, Typed, Pressed, Submitted, or summary-like descriptions of previous actions; treat those as context, not new targeting requirements. \
Screenshots: {}. \
Prefer a single precise click action when the user asks to click a visible target. \
If the target is ambiguous, return an empty actions array and explain the ambiguity in summary. \
User instruction: {}",
        platform::platform_description(),
        platform::primary_modifier_name(),
        windows_text,
        screenshots,
        instruction
    )
}

pub(crate) fn capture_monitors(
    monitor_index: Option<usize>,
    all_monitors: bool,
    out_dir: &Path,
) -> Result<Vec<CapturedScreen>> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    select_monitors(monitor_index, all_monitors)?
        .into_iter()
        .map(|selected| capture_selected_monitor(selected, out_dir))
        .collect()
}

fn capture_selected_monitor(selected: SelectedMonitor, out_dir: &Path) -> Result<CapturedScreen> {
    let monitor = selected.monitor;
    let image = monitor.capture_image()?;
    let image_width = image.width();
    let image_height = image.height();
    let monitor_name = monitor
        .friendly_name()
        .unwrap_or_else(|_| "monitor".to_string());
    let path = out_dir.join(format!(
        "screen-{}-{}.png",
        timestamp_millis()?,
        normalized_filename(&monitor_name)
    ));
    image
        .save(&path)
        .with_context(|| format!("failed to save {}", path.display()))?;

    Ok(CapturedScreen {
        path,
        monitor_index: selected.index,
        origin_x: monitor.x()?,
        origin_y: monitor.y()?,
        desktop_width: monitor.width()?,
        desktop_height: monitor.height()?,
        image_width,
        image_height,
        monitor_name,
    })
}

#[derive(Debug)]
struct SelectedMonitor {
    index: usize,
    monitor: Monitor,
}

fn select_monitors(index: Option<usize>, all_monitors: bool) -> Result<Vec<SelectedMonitor>> {
    if all_monitors && index.is_some() {
        bail!("use either --all-monitors or --monitor, not both");
    }

    let monitors = Monitor::all()?;
    if monitors.is_empty() {
        bail!("no monitors found");
    }

    if let Some(index) = index {
        let monitor = monitors
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow!("monitor index {index} does not exist"))?;

        return Ok(vec![SelectedMonitor { index, monitor }]);
    }

    if all_monitors {
        return Ok(monitors
            .into_iter()
            .enumerate()
            .map(|(index, monitor)| SelectedMonitor { index, monitor })
            .collect());
    }

    monitors
        .iter()
        .enumerate()
        .find(|(_, monitor)| monitor.is_primary().unwrap_or(false))
        .map(|(index, monitor)| SelectedMonitor {
            index,
            monitor: monitor.clone(),
        })
        .or_else(|| {
            monitors
                .first()
                .cloned()
                .map(|monitor| SelectedMonitor { index: 0, monitor })
        })
        .map(|monitor| vec![monitor])
        .ok_or_else(|| anyhow!("no monitor available"))
}

fn execute_plan(
    plan: &ActionPlan,
    origin_x: i32,
    origin_y: i32,
    scale_x: f32,
    scale_y: f32,
) -> Result<()> {
    execute_plan_with_transform_resolver(plan, |_| {
        Ok(CoordinateTransform::new(
            origin_x, origin_y, scale_x, scale_y,
        ))
    })
}

pub(crate) fn execute_plan_for_captures(
    plan: &ActionPlan,
    captures: &[CapturedScreen],
) -> Result<()> {
    execute_plan_with_transform_resolver(plan, |action| {
        let Some(monitor_index) = action.monitor_index() else {
            if captures.len() == 1 {
                return Ok(captures[0].coordinate_transform());
            }

            bail!("coordinate action is missing monitor_index in a multi-monitor plan");
        };

        captures
            .iter()
            .find(|capture| capture.monitor_index == monitor_index)
            .map(CapturedScreen::coordinate_transform)
            .ok_or_else(|| anyhow!("monitor_index {monitor_index} was not captured"))
    })
}

fn execute_plan_with_transform_resolver<F>(plan: &ActionPlan, mut transform_for: F) -> Result<()>
where
    F: FnMut(&Action) -> Result<CoordinateTransform>,
{
    if plan.actions.is_empty() {
        transcript_notice("plan has no actions to execute");
        return Ok(());
    }

    let mut enigo = Enigo::new(&Settings::default()).map_err(|err| anyhow!("{err}"))?;

    let action_count = plan.actions.len();
    transcript_header(&format!(
        "executing {action_count} action(s); summary={}; confidence={:.2}",
        json_string(&plan.summary),
        plan.confidence
    ));

    for (index, action) in plan.actions.iter().enumerate() {
        let step = index + 1;

        match action {
            Action::MoveMouse { x, y, .. } => {
                let transform = transform_for(action)?;
                let (desktop_x, desktop_y) = transform.to_desktop(*x, *y);
                transcript_action(
                    step,
                    action_count,
                    "move_mouse",
                    &format!(
                        "monitor={} screenshot=({}, {}) desktop=({}, {})",
                        format_monitor_index(action.monitor_index()),
                        x,
                        y,
                        desktop_x,
                        desktop_y
                    ),
                );
                enigo
                    .move_mouse(desktop_x, desktop_y, Coordinate::Abs)
                    .map_err(|err| anyhow!("{err}"))?;
            }
            Action::Click { x, y, .. } => {
                let transform = transform_for(action)?;
                let (desktop_x, desktop_y) = transform.to_desktop(*x, *y);
                transcript_action(
                    step,
                    action_count,
                    "click",
                    &format!(
                        "button=left monitor={} screenshot=({}, {}) desktop=({}, {})",
                        format_monitor_index(action.monitor_index()),
                        x,
                        y,
                        desktop_x,
                        desktop_y
                    ),
                );
                move_and_click(&mut enigo, desktop_x, desktop_y, Button::Left)?;
            }
            Action::DoubleClick { x, y, .. } => {
                let transform = transform_for(action)?;
                let (desktop_x, desktop_y) = transform.to_desktop(*x, *y);
                transcript_action(
                    step,
                    action_count,
                    "double_click",
                    &format!(
                        "button=left monitor={} screenshot=({}, {}) desktop=({}, {})",
                        format_monitor_index(action.monitor_index()),
                        x,
                        y,
                        desktop_x,
                        desktop_y
                    ),
                );
                move_and_click(&mut enigo, desktop_x, desktop_y, Button::Left)?;
                move_and_click(&mut enigo, desktop_x, desktop_y, Button::Left)?;
            }
            Action::RightClick { x, y, .. } => {
                let transform = transform_for(action)?;
                let (desktop_x, desktop_y) = transform.to_desktop(*x, *y);
                transcript_action(
                    step,
                    action_count,
                    "click",
                    &format!(
                        "button=right monitor={} screenshot=({}, {}) desktop=({}, {})",
                        format_monitor_index(action.monitor_index()),
                        x,
                        y,
                        desktop_x,
                        desktop_y
                    ),
                );
                move_and_click(&mut enigo, desktop_x, desktop_y, Button::Right)?;
            }
            Action::TypeText { text, .. } => {
                transcript_action(
                    step,
                    action_count,
                    "type_text",
                    &format!(
                        "monitor={} text={}",
                        format_monitor_index(action.monitor_index()),
                        json_string(text)
                    ),
                );
                enigo.text(text).map_err(|err| anyhow!("{err}"))?;
            }
            Action::PasteText { text, .. } => {
                transcript_action(
                    step,
                    action_count,
                    "paste_text",
                    &format!(
                        "monitor={} text={}",
                        format_monitor_index(action.monitor_index()),
                        json_string(text)
                    ),
                );
                paste_text(&mut enigo, text)?;
            }
            Action::PressKey { key, .. } => {
                transcript_action(
                    step,
                    action_count,
                    "press_key",
                    &format!(
                        "monitor={} key={}",
                        format_monitor_index(action.monitor_index()),
                        json_string(key.as_str())
                    ),
                );
                key.press(&mut enigo)?;
            }
            Action::FocusWindow { window, .. } => {
                transcript_action(
                    step,
                    action_count,
                    "focus_window",
                    &format!("window={}", json_string(window)),
                );
                let focused = windows::focus_window(window)?;
                transcript_notice(&format!(
                    "focused app={:?} title={:?}",
                    focused.app, focused.title
                ));
                // Give the window manager time to raise the window before
                // the next click or keystroke lands.
                thread::sleep(Duration::from_millis(400));
            }
            Action::Scroll { amount, x, y, .. } => {
                if let (Some(x), Some(y)) = (x, y) {
                    let transform = transform_for(action)?;
                    let (desktop_x, desktop_y) = transform.to_desktop(*x, *y);
                    transcript_action(
                        step,
                        action_count,
                        "scroll",
                        &format!(
                            "monitor={} amount={} screenshot=({}, {}) desktop=({}, {})",
                            format_monitor_index(action.monitor_index()),
                            amount,
                            x,
                            y,
                            desktop_x,
                            desktop_y
                        ),
                    );
                    enigo
                        .move_mouse(desktop_x, desktop_y, Coordinate::Abs)
                        .map_err(|err| anyhow!("{err}"))?;
                } else {
                    transcript_action(
                        step,
                        action_count,
                        "scroll",
                        &format!(
                            "monitor={} amount={} cursor=current",
                            format_monitor_index(action.monitor_index()),
                            amount
                        ),
                    );
                }
                enigo
                    .scroll(*amount, Axis::Vertical)
                    .map_err(|err| anyhow!("{err}"))?;
            }
        }
    }

    transcript_header("execution complete");

    Ok(())
}

fn transcript_header(message: &str) {
    println!("{ANSI_BOLD}{ANSI_CYAN}transcript:{ANSI_RESET} {message}");
}

fn transcript_notice(message: &str) {
    println!("{ANSI_BOLD}{ANSI_YELLOW}transcript:{ANSI_RESET} {message}");
}

fn transcript_action(step: usize, total: usize, action: &str, details: &str) {
    println!(
        "{ANSI_DIM}transcript {step}/{total}:{ANSI_RESET} {ANSI_BOLD}{ANSI_GREEN}{action}{ANSI_RESET} {details}"
    );
}

pub(crate) fn print_capture(capture: &CapturedScreen) {
    let transform = capture.coordinate_transform();
    println!(
        "monitor_index={} screenshot={} origin=({}, {}) desktop_size={}x{} image_size={}x{} scale=({:.3}, {:.3})",
        capture.monitor_index,
        capture.path.display(),
        capture.origin_x,
        capture.origin_y,
        capture.desktop_width,
        capture.desktop_height,
        capture.image_width,
        capture.image_height,
        transform.scale_x,
        transform.scale_y
    );
}

fn format_monitor_index(monitor_index: Option<usize>) -> String {
    monitor_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<unprintable>\"".to_string())
}

impl Action {
    fn monitor_index(&self) -> Option<usize> {
        match self {
            Action::MoveMouse { monitor_index, .. }
            | Action::Click { monitor_index, .. }
            | Action::DoubleClick { monitor_index, .. }
            | Action::RightClick { monitor_index, .. }
            | Action::TypeText { monitor_index, .. }
            | Action::PasteText { monitor_index, .. }
            | Action::PressKey { monitor_index, .. }
            | Action::FocusWindow { monitor_index, .. }
            | Action::Scroll { monitor_index, .. } => *monitor_index,
        }
    }
}

impl CapturedScreen {
    fn coordinate_transform(&self) -> CoordinateTransform {
        CoordinateTransform::new(
            self.origin_x,
            self.origin_y,
            coordinate_scale(self.image_width, self.desktop_width),
            coordinate_scale(self.image_height, self.desktop_height),
        )
    }
}

impl CoordinateTransform {
    fn new(origin_x: i32, origin_y: i32, scale_x: f32, scale_y: f32) -> Self {
        Self {
            origin_x,
            origin_y,
            scale_x: sanitize_scale(scale_x),
            scale_y: sanitize_scale(scale_y),
        }
    }

    fn to_desktop(self, screenshot_x: i32, screenshot_y: i32) -> (i32, i32) {
        (
            self.origin_x + scale_coordinate(screenshot_x, self.scale_x),
            self.origin_y + scale_coordinate(screenshot_y, self.scale_y),
        )
    }
}

fn coordinate_scale(image_size: u32, desktop_size: u32) -> f32 {
    if desktop_size == 0 {
        return 1.0;
    }

    sanitize_scale(image_size as f32 / desktop_size as f32)
}

fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn scale_coordinate(value: i32, scale: f32) -> i32 {
    (value as f32 / sanitize_scale(scale)).round() as i32
}

fn click_at(x: i32, y: i32, button: Button) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|err| anyhow!("{err}"))?;
    move_and_click(&mut enigo, x, y, button)
}

fn move_and_click(enigo: &mut Enigo, x: i32, y: i32, button: Button) -> Result<()> {
    enigo
        .move_mouse(x, y, Coordinate::Abs)
        .map_err(|err| anyhow!("{err}"))?;
    enigo
        .button(button, Direction::Click)
        .map_err(|err| anyhow!("{err}"))?;
    Ok(())
}

fn paste_text(enigo: &mut Enigo, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    if cfg!(target_os = "macos") {
        if let Err(err) = paste_text_macos(enigo, text) {
            transcript_notice(&format!(
                "macOS paste_text failed; falling back to type_text: {err}"
            ));
            enigo.text(text).map_err(|err| anyhow!("{err}"))?;
        }
        return Ok(());
    }

    enigo.text(text).map_err(|err| anyhow!("{err}"))?;
    Ok(())
}

fn paste_text_macos(enigo: &mut Enigo, text: &str) -> Result<()> {
    let previous_clipboard = read_macos_clipboard().ok();
    write_macos_clipboard(text.as_bytes())?;
    thread::sleep(Duration::from_millis(80));
    let paste_result = paste_clipboard_hotkey(enigo);
    thread::sleep(Duration::from_millis(250));

    if let Some(previous_clipboard) = previous_clipboard {
        if let Err(err) = write_macos_clipboard(&previous_clipboard) {
            transcript_notice(&format!(
                "failed to restore macOS clipboard after paste: {err}"
            ));
        }
    }

    paste_result
}

fn read_macos_clipboard() -> Result<Vec<u8>> {
    let output = Command::new("pbpaste")
        .output()
        .context("failed to run pbpaste")?;
    if !output.status.success() {
        bail!("pbpaste exited with status {}", output.status);
    }
    Ok(output.stdout)
}

fn write_macos_clipboard(bytes: &[u8]) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run pbcopy")?;

    {
        let mut stdin = child.stdin.take().context("failed to open pbcopy stdin")?;
        stdin
            .write_all(bytes)
            .context("failed to write to pbcopy")?;
    }

    let status = child.wait().context("failed to wait for pbcopy")?;
    if !status.success() {
        bail!("pbcopy exited with status {status}");
    }

    Ok(())
}

fn paste_clipboard_hotkey(enigo: &mut Enigo) -> Result<()> {
    keys::press_chord(enigo, &[platform::primary_modifier()], Key::Unicode('v'))
}

fn read_plan(path: &Path) -> Result<ActionPlan> {
    let json = fs::read_to_string(path)
        .with_context(|| format!("failed to read action plan {}", path.display()))?;
    serde_json::from_str(&json).with_context(|| format!("invalid action plan {}", path.display()))
}

pub(crate) fn timestamp_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_millis())
}

fn normalized_filename(name: &str) -> String {
    let normalized = name
        .chars()
        .map(|ch| match ch {
            '|' | '\\' | ':' | '/' | '"' | '<' | '>' | '?' | '*' => '-',
            ch if ch.is_control() => '-',
            ch => ch,
        })
        .collect::<String>();

    let trimmed = normalized.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "monitor".to_string()
    } else {
        trimmed.to_string()
    }
}

impl From<MouseButton> for Button {
    fn from(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, ActionPlan, CapturedScreen, CoordinateTransform, WindowInfo, build_codex_prompt,
        coordinate_scale,
    };

    #[test]
    fn parses_strict_schema_click_with_nullable_unused_fields() {
        let json = r#"{
            "summary": "click target",
            "confidence": 0.9,
            "actions": [
                {"type": "click", "monitor_index": 1, "x": 10, "y": 20, "text": null, "amount": null}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            plan.actions[0],
            Action::Click {
                monitor_index: Some(1),
                x: 10,
                y: 20
            }
        ));
    }

    #[test]
    fn parses_strict_schema_type_text_with_nullable_coordinate_fields() {
        let json = r#"{
            "summary": "type into focused field",
            "confidence": 0.9,
            "actions": [
                {"type": "type_text", "monitor_index": null, "x": null, "y": null, "text": "hello", "amount": null}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(
            matches!(plan.actions[0], Action::TypeText { monitor_index: None, ref text } if text == "hello")
        );
    }

    #[test]
    fn parses_strict_schema_press_key_with_enter() {
        let json = r#"{
            "summary": "press enter",
            "confidence": 0.9,
            "actions": [
                {"type": "press_key", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": "enter"}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            &plan.actions[0],
            Action::PressKey {
                monitor_index: None,
                key
            } if key.as_str() == "enter"
        ));
    }

    #[test]
    fn rejects_press_key_with_unknown_key() {
        let json = r#"{
            "summary": "press bogus key",
            "confidence": 0.9,
            "actions": [
                {"type": "press_key", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": "warp_drive"}
            ]
        }"#;

        assert!(serde_json::from_str::<ActionPlan>(json).is_err());
    }

    #[test]
    fn parses_strict_schema_press_key_with_modifier_combo() {
        let json = r#"{
            "summary": "focus address bar",
            "confidence": 0.9,
            "actions": [
                {"type": "press_key", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": "cmd+l", "window": null}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            &plan.actions[0],
            Action::PressKey {
                monitor_index: None,
                key
            } if key.as_str() == "cmd+l"
        ));
    }

    #[test]
    fn parses_strict_schema_focus_window() {
        let json = r#"{
            "summary": "focus the Chrome window",
            "confidence": 0.9,
            "actions": [
                {"type": "focus_window", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": null, "window": "Stake.com - Blackjack"}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            &plan.actions[0],
            Action::FocusWindow {
                monitor_index: None,
                window
            } if window == "Stake.com - Blackjack"
        ));
    }

    #[test]
    fn parses_strict_schema_terminal_clear_control_key() {
        let json = r#"{
            "summary": "clear prompt",
            "confidence": 0.9,
            "actions": [
                {"type": "press_key", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": "control_a"}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            &plan.actions[0],
            Action::PressKey {
                monitor_index: None,
                key
            } if key.as_str() == "control_a"
        ));
    }

    #[test]
    fn parses_strict_schema_select_all() {
        let json = r#"{
            "summary": "select field contents",
            "confidence": 0.9,
            "actions": [
                {"type": "press_key", "monitor_index": null, "x": null, "y": null, "text": null, "amount": null, "key": "select_all"}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            &plan.actions[0],
            Action::PressKey {
                monitor_index: None,
                key
            } if key.as_str() == "select_all"
        ));
    }

    #[test]
    fn parses_strict_schema_paste_text() {
        let json = r#"{
            "summary": "paste command",
            "confidence": 0.9,
            "actions": [
                {"type": "paste_text", "monitor_index": null, "x": null, "y": null, "text": "cargo test", "amount": null, "key": null}
            ]
        }"#;

        let plan: ActionPlan = serde_json::from_str(json).unwrap();

        assert_eq!(plan.actions.len(), 1);
        assert!(
            matches!(plan.actions[0], Action::PasteText { monitor_index: None, ref text } if text == "cargo test")
        );
    }

    fn test_captures() -> Vec<CapturedScreen> {
        vec![CapturedScreen {
            path: "runs/screen.png".into(),
            monitor_index: 0,
            origin_x: 0,
            origin_y: 0,
            desktop_width: 1440,
            desktop_height: 900,
            image_width: 2880,
            image_height: 1800,
            monitor_name: "Built-in Retina Display".to_string(),
        }]
    }

    #[test]
    fn prompt_tells_codex_to_paste_terminal_commands_without_newline() {
        let prompt = build_codex_prompt("run cargo test in the terminal", &test_captures(), &[]);

        assert!(prompt.contains("use paste_text for the command text without a trailing newline"));
        assert!(prompt.contains("press_key control_a followed by press_key control_k"));
        assert!(prompt.contains("press_key select_all"));
        assert!(prompt.contains("then use press_key enter"));
    }

    #[test]
    fn prompt_describes_operating_system_and_keyboard_conventions() {
        let prompt = build_codex_prompt("copy the selected text", &test_captures(), &[]);

        assert!(prompt.contains(super::platform_name()));
        assert!(prompt.contains(&format!(
            "The primary keyboard shortcut modifier on this operating system is {}",
            super::platform::primary_modifier_name()
        )));
        assert!(prompt.contains("cmd and primary mean Command on macOS and Control on Windows and Linux"));
        assert!(prompt.contains("select_all, copy, paste, cut, undo, redo, save, find, new_tab, close_tab"));
        assert!(prompt.contains("Visible windows: unavailable"));
    }

    #[test]
    fn prompt_lists_visible_windows_and_focus_window_rule() {
        let visible_windows = vec![WindowInfo {
            app: "Google Chrome".to_string(),
            title: "Stake.com - Blackjack".to_string(),
            x: 100,
            y: 50,
            width: 1280,
            height: 800,
            focused: true,
            id: None,
        }];

        let prompt = build_codex_prompt(
            "click Hit in the Stake.com Chrome window",
            &test_captures(),
            &visible_windows,
        );

        assert!(prompt.contains(
            "app=\"Google Chrome\" title=\"Stake.com - Blackjack\" desktop_bounds=(100, 50, 1280x800) focused=true"
        ));
        assert!(prompt.contains("first return a focus_window action"));
    }

    #[test]
    fn converts_retina_screenshot_pixels_to_desktop_coordinates() {
        let transform = CoordinateTransform::new(100, 50, 2.0, 2.0);

        assert_eq!(transform.to_desktop(400, 200), (300, 150));
    }

    #[test]
    fn derives_coordinate_scale_from_image_and_desktop_sizes() {
        assert_eq!(coordinate_scale(3840, 1920), 2.0);
        assert_eq!(coordinate_scale(1920, 1920), 1.0);
        assert_eq!(coordinate_scale(1920, 0), 1.0);
    }
}
