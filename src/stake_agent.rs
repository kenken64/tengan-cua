use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    Action, ActionPlan, CapturedScreen, capture_monitors, default_codex_bin,
    execute_plan_for_captures, platform::platform_description, print_capture, timestamp_millis,
};

const SESSION_LIMIT_MILLIS: u128 = 60 * 60 * 1000;
const MAX_HISTORY: usize = 100;

#[derive(Args, Debug)]
pub(crate) struct StakeAgentArgs {
    /// Monitor index from `monitors`, or omit to use the primary monitor.
    #[arg(long)]
    monitor: Option<usize>,
    /// Capture every visible monitor and attach all screenshots to Codex.
    #[arg(long)]
    all_monitors: bool,
    /// Output directory for screenshots and Stake agent response JSON.
    #[arg(long, default_value = "runs/stake-agent")]
    out_dir: PathBuf,
    /// Stake/Tengan policy prompt to load.
    #[arg(long, default_value = "Agents.md")]
    prompt_file: PathBuf,
    /// Persistent session state JSON file.
    #[arg(long, default_value = "runs/stake-agent-state.json")]
    state_file: PathBuf,
    /// Codex executable to run. Defaults to codex.cmd on Windows and codex elsewhere.
    #[arg(long)]
    codex_bin: Option<String>,
    /// Delay between observe-act-verify ticks.
    #[arg(long, default_value_t = 4)]
    interval_seconds: u64,
    /// Stop after one observe-act-verify tick.
    #[arg(long)]
    once: bool,
    /// Maximum observe-act-verify ticks for this run.
    #[arg(long)]
    max_ticks: Option<u64>,
    /// Actually execute validated click actions. Without this flag, the agent dry-runs.
    #[arg(long)]
    execute: bool,
    /// Do not run a verification observation after executing actions.
    #[arg(long)]
    no_verify: bool,
    /// Minimum model confidence required before executing actions.
    #[arg(long, default_value_t = 0.82)]
    min_confidence: f32,
    /// Ignore any previous session state and start a fresh state file.
    #[arg(long)]
    reset_state: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct StakeAgentSession {
    started_at_millis: u128,
    updated_at_millis: u128,
    tick_count: u64,
    last_phase: Option<StakePhase>,
    last_balance: Option<f32>,
    loss_streak: u32,
    win_streak: u32,
    negative_count_streak: u32,
    tengan_unreadable_ticks: u32,
    abort_reason: Option<String>,
    history: Vec<StakeTickHistory>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StakeTickHistory {
    tick: u64,
    at_millis: u128,
    phase: StakePhase,
    confidence: f32,
    balance: Option<f32>,
    true_count: Option<f32>,
    summary: String,
    validation: String,
    executed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct StakeAgentResponse {
    summary: String,
    confidence: f32,
    phase: StakePhase,
    state: StakeAgentState,
    safety: StakeSafety,
    actions: Vec<StakeAgentAction>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StakePhase {
    PreBet,
    Insurance,
    InHand,
    SplitHand,
    Dealing,
    Idle,
    Abort,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
struct StakeAgentState {
    tengan: TenganState,
    table: TableState,
}

#[derive(Debug, Serialize, Deserialize)]
struct TenganState {
    true_count: Option<f32>,
    run_count: Option<i32>,
    decks_remaining: Option<f32>,
    hands_played: Option<u32>,
    recommended_action: Option<String>,
    hand_state: Option<String>,
    edge_percent: Option<f32>,
    next_bet: Option<f32>,
    deviation_note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TableState {
    countdown_seconds: Option<f32>,
    balance: Option<f32>,
    total_bet: Option<f32>,
    your_hand_value: Option<String>,
    dealer_upcard: Option<String>,
    kopioh_seat_label: Option<String>,
    bet_placed: Option<bool>,
    bet_amount: Option<f32>,
    total_hands: Option<u32>,
    active_hand_idx: Option<u32>,
    result_text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StakeSafety {
    abort: bool,
    reason: Option<String>,
    hazards: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StakeAgentAction {
    kind: StakeActionKind,
    monitor_index: Option<usize>,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<StakeButton>,
    chip_value: Option<u32>,
    bet_target: Option<BetTarget>,
    description: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StakeActionKind {
    Observe,
    ClickButton,
    ClickChip,
    ClickBetCircle,
    ClickDealNow,
    ClickUndo,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StakeButton {
    Hit,
    Stand,
    Double,
    Split,
    InsuranceYes,
    InsuranceNo,
    Other,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BetTarget {
    Main,
    Side213,
    Pairs,
    OtherSeat,
}

#[derive(Debug)]
struct ValidationOutcome {
    plan: ActionPlan,
    abort_reason: Option<String>,
    rejected_reasons: Vec<String>,
}

pub(crate) fn run(args: StakeAgentArgs) -> Result<()> {
    if args.monitor.is_some() && args.all_monitors {
        bail!("use either --all-monitors or --monitor, not both");
    }

    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("failed to create {}", args.out_dir.display()))?;

    if let Some(parent) = args
        .state_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let policy = fs::read_to_string(&args.prompt_file)
        .with_context(|| format!("failed to read {}", args.prompt_file.display()))?;
    let codex_bin = args.codex_bin.clone().unwrap_or_else(default_codex_bin);
    let mut session = if args.reset_state {
        StakeAgentSession::new(timestamp_millis()?)
    } else {
        StakeAgentSession::load_or_new(&args.state_file)?
    };

    if session.abort_reason.is_some() && !args.reset_state {
        println!(
            "stake-agent: previous session is stopped: {}",
            session.abort_reason.as_deref().unwrap_or("unknown")
        );
        println!("stake-agent: rerun with --reset-state to start a fresh session");
        return Ok(());
    }

    let mut run_ticks = 0_u64;

    loop {
        run_ticks += 1;
        run_decision_tick(&args, &policy, &codex_bin, &mut session)?;
        session.save(&args.state_file)?;

        if session.abort_reason.is_some() {
            println!(
                "stake-agent: stopped: {}",
                session.abort_reason.as_deref().unwrap_or("unknown")
            );
            break;
        }

        if args.once || args.max_ticks.is_some_and(|max| run_ticks >= max) {
            break;
        }

        thread::sleep(Duration::from_secs(args.interval_seconds));
    }

    Ok(())
}

fn run_decision_tick(
    args: &StakeAgentArgs,
    policy: &str,
    codex_bin: &str,
    session: &mut StakeAgentSession,
) -> Result<()> {
    let tick_started = timestamp_millis()?;
    let captures = capture_monitors(args.monitor, args.all_monitors, &args.out_dir)?;
    let response = run_model(
        codex_bin,
        policy,
        session,
        &captures,
        &args.out_dir,
        StakeModelMode::Decision,
    )?;

    for capture in &captures {
        print_capture(capture);
    }

    session.update_counters(&response, tick_started);
    let validation = validate_response(&response, session, args.min_confidence);
    print_response_report(&response, &validation, args.execute);

    let should_execute =
        args.execute && validation.abort_reason.is_none() && !validation.plan.actions.is_empty();
    if should_execute {
        execute_plan_for_captures(&validation.plan, &captures)?;
    }

    session.record_history(&response, &validation, tick_started, should_execute);

    if let Some(reason) = validation.abort_reason {
        session.abort_reason = Some(reason);
        return Ok(());
    }

    if should_execute && !args.no_verify {
        run_verification_tick(args, policy, codex_bin, session)?;
    }

    Ok(())
}

fn run_verification_tick(
    args: &StakeAgentArgs,
    policy: &str,
    codex_bin: &str,
    session: &mut StakeAgentSession,
) -> Result<()> {
    let tick_started = timestamp_millis()?;
    let captures = capture_monitors(args.monitor, args.all_monitors, &args.out_dir)?;
    let response = run_model(
        codex_bin,
        policy,
        session,
        &captures,
        &args.out_dir,
        StakeModelMode::Verification,
    )?;

    for capture in &captures {
        print_capture(capture);
    }

    session.update_counters(&response, tick_started);
    let validation = validate_response(&response, session, 0.0);
    print_response_report(&response, &validation, false);
    session.record_history(&response, &validation, tick_started, false);

    if let Some(reason) = validation.abort_reason {
        session.abort_reason = Some(reason);
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum StakeModelMode {
    Decision,
    Verification,
}

fn run_model(
    codex_bin: &str,
    policy: &str,
    session: &StakeAgentSession,
    captures: &[CapturedScreen],
    out_dir: &Path,
    mode: StakeModelMode,
) -> Result<StakeAgentResponse> {
    let response_path = out_dir.join(format!("stake-agent-response-{}.json", timestamp_millis()?));
    let schema_path = PathBuf::from("schemas").join("stake_agent.schema.json");
    let prompt = build_stake_agent_prompt(policy, session, captures, mode)?;

    let mut command = Command::new(codex_bin);
    command.arg("exec").arg("--skip-git-repo-check");

    for capture in captures {
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

    println!("stake-agent-response={}", response_path.display());
    let json = fs::read_to_string(&response_path)
        .with_context(|| format!("failed to read {}", response_path.display()))?;
    let response: StakeAgentResponse = serde_json::from_str(&json)
        .with_context(|| format!("invalid Stake agent response {}", response_path.display()))?;
    println!("{}", serde_json::to_string_pretty(&response)?);

    Ok(response)
}

fn build_stake_agent_prompt(
    policy: &str,
    session: &StakeAgentSession,
    captures: &[CapturedScreen],
    mode: StakeModelMode,
) -> Result<String> {
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

    let mode_instruction = match mode {
        StakeModelMode::Decision => {
            "DECISION MODE: read the screenshots, classify the blackjack phase, fill the state fields, and propose only the semantic click actions allowed by the policy."
        }
        StakeModelMode::Verification => {
            "VERIFICATION MODE: verify the result of the last executed action. Fill the state fields. Return an empty actions array unless a safety abort is required."
        }
    };

    Ok(format!(
        "You are running inside Tengan CUA on a {} desktop. \
Return only JSON matching schemas/stake_agent.schema.json. \
Use screenshot pixel coordinates, not desktop coordinates. \
Every executable action must include monitor_index, x, and y from the screenshot used. \
Use semantic actions only: click_chip, click_bet_circle, click_deal_now, click_undo, click_button, or observe. \
Do not return text typing, right-click, double-click, scrolling, or arbitrary desktop actions. \
If any safety condition, login/payment/2FA prompt, CAPTCHA, verification prompt, or unexpected dialog appears, set safety.abort=true and return no executable actions. \
When no hazards are visible, set safety.hazards to an empty array. \
{mode_instruction}\n\n\
POLICY FROM Agents.md:\n{policy}\n\n\
PERSISTED SESSION STATE:\n{}\n\n\
SCREENSHOTS:\n{screenshots}",
        platform_description(),
        serde_json::to_string_pretty(session)?
    ))
}

fn validate_response(
    response: &StakeAgentResponse,
    session: &StakeAgentSession,
    min_confidence: f32,
) -> ValidationOutcome {
    let mut abort_reason = session.abort_reason.clone();
    let mut rejected_reasons = Vec::new();

    if response.safety.abort {
        abort_reason = Some(
            response
                .safety
                .reason
                .clone()
                .unwrap_or_else(|| "model reported safety abort".to_string()),
        );
    }

    if !response.safety.hazards.is_empty() {
        abort_reason = Some(format!(
            "safety hazards detected: {}",
            response.safety.hazards.join(", ")
        ));
    }

    if response.phase == StakePhase::Abort {
        abort_reason = Some(
            response
                .safety
                .reason
                .clone()
                .unwrap_or_else(|| "phase is abort".to_string()),
        );
    }

    if response.confidence < min_confidence {
        rejected_reasons.push(format!(
            "confidence {:.2} is below minimum {:.2}",
            response.confidence, min_confidence
        ));
    }

    if let Some(reason) = hard_session_abort_reason(session, response) {
        abort_reason = Some(reason);
    }

    let semantic_actions = response
        .actions
        .iter()
        .filter(|action| action.kind != StakeActionKind::Observe)
        .cloned()
        .collect::<Vec<_>>();

    if abort_reason.is_none() {
        validate_phase_actions(response, &semantic_actions, &mut rejected_reasons);
    }

    let plan = if abort_reason.is_none() && rejected_reasons.is_empty() {
        match semantic_actions_to_plan(response, &semantic_actions) {
            Ok(plan) => plan,
            Err(err) => {
                rejected_reasons.push(err.to_string());
                empty_plan(response)
            }
        }
    } else {
        empty_plan(response)
    };

    ValidationOutcome {
        plan,
        abort_reason,
        rejected_reasons,
    }
}

fn hard_session_abort_reason(
    session: &StakeAgentSession,
    response: &StakeAgentResponse,
) -> Option<String> {
    if session
        .updated_at_millis
        .saturating_sub(session.started_at_millis)
        >= SESSION_LIMIT_MILLIS
    {
        return Some("session reached 60 minute cap".to_string());
    }

    if response
        .state
        .table
        .balance
        .is_some_and(|balance| balance < 5.0)
    {
        return Some("balance below $5 safety floor".to_string());
    }

    if session.loss_streak >= 5 {
        return Some("loss streak reached 5".to_string());
    }

    if session.win_streak >= 5 {
        return Some("win streak reached 5".to_string());
    }

    if session.negative_count_streak >= 3 {
        return Some("true count below -3 for 3 consecutive readable ticks".to_string());
    }

    if session.tengan_unreadable_ticks >= 12 {
        return Some("Tengan fields unreadable for 12 consecutive ticks".to_string());
    }

    None
}

fn validate_phase_actions(
    response: &StakeAgentResponse,
    actions: &[StakeAgentAction],
    rejected_reasons: &mut Vec<String>,
) {
    for action in actions {
        if let Err(reason) = validate_executable_shape(action) {
            rejected_reasons.push(reason);
        }
    }

    match response.phase {
        StakePhase::PreBet => validate_pre_bet(response, actions, rejected_reasons),
        StakePhase::Insurance => validate_insurance(actions, rejected_reasons),
        StakePhase::InHand => validate_hand_decision(actions, false, response, rejected_reasons),
        StakePhase::SplitHand => validate_hand_decision(actions, true, response, rejected_reasons),
        StakePhase::Dealing | StakePhase::Idle | StakePhase::Unknown => {
            if !actions.is_empty() {
                rejected_reasons.push(format!(
                    "phase {:?} is observe-only but proposed {} executable action(s)",
                    response.phase,
                    actions.len()
                ));
            }
        }
        StakePhase::Abort => {}
    }
}

fn validate_executable_shape(action: &StakeAgentAction) -> Result<(), String> {
    if action.monitor_index.is_none() || action.x.is_none() || action.y.is_none() {
        return Err(format!(
            "action {:?} is missing monitor_index/x/y",
            action.kind
        ));
    }

    match action.kind {
        StakeActionKind::Observe => Err("observe action should not be executable".to_string()),
        StakeActionKind::ClickButton
        | StakeActionKind::ClickChip
        | StakeActionKind::ClickBetCircle
        | StakeActionKind::ClickDealNow
        | StakeActionKind::ClickUndo => Ok(()),
    }
}

fn validate_pre_bet(
    response: &StakeAgentResponse,
    actions: &[StakeAgentAction],
    rejected_reasons: &mut Vec<String>,
) {
    if response
        .state
        .tengan
        .true_count
        .is_some_and(|true_count| true_count < 0.0)
        && !actions.is_empty()
    {
        rejected_reasons
            .push("PRE_BET true_count < 0 requires sitting out with no actions".to_string());
    }

    let mut chip_clicks = 0_u32;
    let mut main_bet_clicks = 0_u32;

    for action in actions {
        match action.kind {
            StakeActionKind::ClickChip => {
                if action.chip_value != Some(1) {
                    rejected_reasons.push(format!(
                        "PRE_BET only allows $1 chip, got {:?}",
                        action.chip_value
                    ));
                }
                chip_clicks += 1;
            }
            StakeActionKind::ClickBetCircle => {
                if action.bet_target != Some(BetTarget::Main) {
                    rejected_reasons.push(format!(
                        "PRE_BET only allows main bet circle, got {:?}",
                        action.bet_target
                    ));
                }
                main_bet_clicks += 1;
            }
            StakeActionKind::ClickDealNow => {
                let countdown = response
                    .state
                    .table
                    .countdown_seconds
                    .unwrap_or(f32::INFINITY);
                let bet_present = response.state.table.bet_placed.unwrap_or(false)
                    || main_bet_clicks > 0
                    || response.state.table.bet_amount.unwrap_or(0.0) > 0.0;
                if countdown > 3.0 || !bet_present {
                    rejected_reasons.push(
                        "DEAL NOW is only allowed when countdown <= 3 and a bet is placed"
                            .to_string(),
                    );
                }
            }
            StakeActionKind::ClickUndo => {}
            StakeActionKind::ClickButton | StakeActionKind::Observe => {
                rejected_reasons.push(format!("PRE_BET does not allow {:?}", action.kind));
            }
        }
    }

    if chip_clicks != main_bet_clicks {
        rejected_reasons.push(format!(
            "PRE_BET chip clicks ({chip_clicks}) must match main bet circle clicks ({main_bet_clicks})"
        ));
    }

    if main_bet_clicks > 5 {
        rejected_reasons.push("PRE_BET hard cap is $5 / five $1 clicks".to_string());
    }
}

fn validate_insurance(actions: &[StakeAgentAction], rejected_reasons: &mut Vec<String>) {
    if actions.len() != 1 {
        rejected_reasons.push(format!(
            "INSURANCE requires exactly one YES/NO action, got {}",
            actions.len()
        ));
        return;
    }

    let action = &actions[0];
    if action.kind != StakeActionKind::ClickButton
        || !matches!(
            action.button,
            Some(StakeButton::InsuranceYes | StakeButton::InsuranceNo)
        )
    {
        rejected_reasons.push(format!(
            "INSURANCE only allows insurance_yes/insurance_no, got {:?}/{:?}",
            action.kind, action.button
        ));
    }
}

fn validate_hand_decision(
    actions: &[StakeAgentAction],
    split: bool,
    response: &StakeAgentResponse,
    rejected_reasons: &mut Vec<String>,
) {
    if actions.len() != 1 {
        rejected_reasons.push(format!(
            "{} requires exactly one decision action, got {}",
            if split { "SPLIT_HAND" } else { "IN_HAND" },
            actions.len()
        ));
        return;
    }

    let action = &actions[0];
    if action.kind != StakeActionKind::ClickButton
        || !matches!(
            action.button,
            Some(StakeButton::Hit | StakeButton::Stand | StakeButton::Double | StakeButton::Split)
        )
    {
        rejected_reasons.push(format!(
            "hand decision only allows hit/stand/double/split, got {:?}/{:?}",
            action.kind, action.button
        ));
    }

    if split
        && response
            .state
            .table
            .total_bet
            .is_some_and(|total_bet| total_bet >= 10.0)
        && matches!(
            action.button,
            Some(StakeButton::Double | StakeButton::Split)
        )
    {
        rejected_reasons
            .push("SPLIT_HAND exposure cap blocks double/split at total_bet >= $10".to_string());
    }
}

fn semantic_actions_to_plan(
    response: &StakeAgentResponse,
    actions: &[StakeAgentAction],
) -> Result<ActionPlan> {
    let mut plan_actions = Vec::with_capacity(actions.len());

    for action in actions {
        let (monitor_index, x, y) = action_coordinates(action)?;
        plan_actions.push(Action::Click {
            monitor_index: Some(monitor_index),
            x,
            y,
        });
    }

    Ok(ActionPlan {
        summary: response.summary.clone(),
        confidence: response.confidence,
        actions: plan_actions,
    })
}

fn action_coordinates(action: &StakeAgentAction) -> Result<(usize, i32, i32)> {
    let monitor_index = action
        .monitor_index
        .ok_or_else(|| anyhow!("missing monitor_index"))?;
    let x = action.x.ok_or_else(|| anyhow!("missing x"))?;
    let y = action.y.ok_or_else(|| anyhow!("missing y"))?;
    Ok((monitor_index, x, y))
}

fn empty_plan(response: &StakeAgentResponse) -> ActionPlan {
    ActionPlan {
        summary: response.summary.clone(),
        confidence: response.confidence,
        actions: Vec::new(),
    }
}

fn print_response_report(
    response: &StakeAgentResponse,
    validation: &ValidationOutcome,
    execute_enabled: bool,
) {
    println!(
        "stake-agent: phase={:?} confidence={:.2} actions={} execute={}",
        response.phase,
        response.confidence,
        validation.plan.actions.len(),
        execute_enabled && validation.abort_reason.is_none()
    );

    for reason in &validation.rejected_reasons {
        println!("stake-agent: rejected: {reason}");
    }

    if let Some(reason) = &validation.abort_reason {
        println!("stake-agent: abort: {reason}");
    }
}

impl StakeAgentSession {
    fn new(now: u128) -> Self {
        Self {
            started_at_millis: now,
            updated_at_millis: now,
            tick_count: 0,
            last_phase: None,
            last_balance: None,
            loss_streak: 0,
            win_streak: 0,
            negative_count_streak: 0,
            tengan_unreadable_ticks: 0,
            abort_reason: None,
            history: Vec::new(),
        }
    }

    fn load_or_new(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new(timestamp_millis()?));
        }

        let json = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&json)
            .with_context(|| format!("invalid Stake agent state {}", path.display()))
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    fn update_counters(&mut self, response: &StakeAgentResponse, now: u128) {
        self.updated_at_millis = now;
        self.tick_count += 1;
        self.last_phase = Some(response.phase);

        if let Some(balance) = response.state.table.balance {
            if let Some(last_balance) = self.last_balance {
                if balance > last_balance + 0.001 {
                    self.win_streak += 1;
                    self.loss_streak = 0;
                } else if balance < last_balance - 0.001 {
                    self.loss_streak += 1;
                    self.win_streak = 0;
                }
            }
            self.last_balance = Some(balance);
        }

        match response.state.tengan.true_count {
            Some(true_count) if true_count < -3.0 => {
                self.negative_count_streak += 1;
            }
            Some(_) => {
                self.negative_count_streak = 0;
            }
            None => {}
        }

        if response.state.tengan.true_count.is_none()
            && response.state.tengan.recommended_action.is_none()
            && response.state.tengan.next_bet.is_none()
        {
            self.tengan_unreadable_ticks += 1;
        } else {
            self.tengan_unreadable_ticks = 0;
        }
    }

    fn record_history(
        &mut self,
        response: &StakeAgentResponse,
        validation: &ValidationOutcome,
        at_millis: u128,
        executed: bool,
    ) {
        let validation = if let Some(reason) = &validation.abort_reason {
            format!("abort: {reason}")
        } else if validation.rejected_reasons.is_empty() {
            "accepted".to_string()
        } else {
            format!("rejected: {}", validation.rejected_reasons.join("; "))
        };

        self.history.push(StakeTickHistory {
            tick: self.tick_count,
            at_millis,
            phase: response.phase,
            confidence: response.confidence,
            balance: response.state.table.balance,
            true_count: response.state.tengan.true_count,
            summary: response.summary.clone(),
            validation,
            executed,
        });

        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(0..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_bet_rejects_side_bet_targets() {
        let response = response_with_phase(
            StakePhase::PreBet,
            vec![
                click_action(StakeActionKind::ClickChip, None, Some(1), None),
                click_action(
                    StakeActionKind::ClickBetCircle,
                    None,
                    None,
                    Some(BetTarget::Pairs),
                ),
            ],
        );
        let session = session();

        let validation = validate_response(&response, &session, 0.0);

        assert!(validation.plan.actions.is_empty());
        assert!(
            validation
                .rejected_reasons
                .iter()
                .any(|reason| { reason.contains("only allows main bet circle") })
        );
    }

    #[test]
    fn pre_bet_rejects_bets_when_count_is_negative() {
        let mut response = response_with_phase(
            StakePhase::PreBet,
            vec![
                click_action(StakeActionKind::ClickChip, None, Some(1), None),
                click_action(
                    StakeActionKind::ClickBetCircle,
                    None,
                    None,
                    Some(BetTarget::Main),
                ),
            ],
        );
        response.state.tengan.true_count = Some(-0.1);
        let session = session();

        let validation = validate_response(&response, &session, 0.0);

        assert!(validation.plan.actions.is_empty());
        assert!(
            validation
                .rejected_reasons
                .iter()
                .any(|reason| { reason.contains("sitting out") })
        );
    }

    #[test]
    fn insurance_accepts_single_no_button() {
        let response = response_with_phase(
            StakePhase::Insurance,
            vec![click_action(
                StakeActionKind::ClickButton,
                Some(StakeButton::InsuranceNo),
                None,
                None,
            )],
        );
        let session = session();

        let validation = validate_response(&response, &session, 0.0);

        assert_eq!(validation.plan.actions.len(), 1);
        assert!(validation.rejected_reasons.is_empty());
    }

    #[test]
    fn idle_phase_rejects_executable_actions() {
        let response = response_with_phase(
            StakePhase::Idle,
            vec![click_action(
                StakeActionKind::ClickButton,
                Some(StakeButton::Stand),
                None,
                None,
            )],
        );
        let session = session();

        let validation = validate_response(&response, &session, 0.0);

        assert!(validation.plan.actions.is_empty());
        assert!(
            validation
                .rejected_reasons
                .iter()
                .any(|reason| { reason.contains("observe-only") })
        );
    }

    fn session() -> StakeAgentSession {
        StakeAgentSession::new(1)
    }

    fn response_with_phase(
        phase: StakePhase,
        actions: Vec<StakeAgentAction>,
    ) -> StakeAgentResponse {
        StakeAgentResponse {
            summary: "test".to_string(),
            confidence: 0.95,
            phase,
            state: StakeAgentState {
                tengan: TenganState {
                    true_count: Some(1.0),
                    run_count: Some(1),
                    decks_remaining: Some(6.0),
                    hands_played: Some(1),
                    recommended_action: Some("NONE".to_string()),
                    hand_state: None,
                    edge_percent: None,
                    next_bet: Some(1.0),
                    deviation_note: None,
                },
                table: TableState {
                    countdown_seconds: Some(10.0),
                    balance: Some(20.0),
                    total_bet: Some(0.0),
                    your_hand_value: None,
                    dealer_upcard: None,
                    kopioh_seat_label: Some("S1 kopioh".to_string()),
                    bet_placed: Some(false),
                    bet_amount: Some(0.0),
                    total_hands: Some(1),
                    active_hand_idx: Some(0),
                    result_text: None,
                },
            },
            safety: StakeSafety {
                abort: false,
                reason: None,
                hazards: Vec::new(),
            },
            actions,
        }
    }

    fn click_action(
        kind: StakeActionKind,
        button: Option<StakeButton>,
        chip_value: Option<u32>,
        bet_target: Option<BetTarget>,
    ) -> StakeAgentAction {
        StakeAgentAction {
            kind,
            monitor_index: Some(0),
            x: Some(100),
            y: Some(200),
            button,
            chip_value,
            bet_target,
            description: "click".to_string(),
        }
    }
}
