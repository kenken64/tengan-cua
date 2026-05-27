use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
use serde::{Deserialize, Serialize};
use xcap::Monitor;

mod stake_agent;

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

pub(crate) fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        std::env::consts::OS
    }
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

fn ask_codex(
    codex_bin: &str,
    instruction: &str,
    monitor_index: Option<usize>,
    all_monitors: bool,
    out_dir: &Path,
    execute: bool,
) -> Result<()> {
    let captures = capture_monitors(monitor_index, all_monitors, out_dir)?;
    let response_path = out_dir.join(format!("codex-action-{}.json", timestamp_millis()?));
    let schema_path = PathBuf::from("schemas").join("codex_action.schema.json");
    let prompt = build_codex_prompt(instruction, &captures);

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

fn build_codex_prompt(instruction: &str, captures: &[CapturedScreen]) -> String {
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

    format!(
        "You are controlling a {} desktop from attached screenshot images. \
Return only a JSON action plan matching the schema. \
Use coordinates relative to the selected screenshot image pixels, not absolute desktop coordinates. \
For every coordinate action, set monitor_index to the monitor_index of the screenshot image used for that coordinate. \
For actions without coordinates, set monitor_index to null. \
Screenshots: {}. \
Prefer a single precise click action when the user asks to click a visible target. \
If the target is ambiguous, return an empty actions array and explain the ambiguity in summary. \
User instruction: {}",
        platform_name(),
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
    let mut enigo = Enigo::new(&Settings::default()).map_err(|err| anyhow!("{err}"))?;

    if plan.actions.is_empty() {
        transcript_notice("plan has no actions to execute");
        return Ok(());
    }

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
    use super::{Action, ActionPlan, CoordinateTransform, coordinate_scale};

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
