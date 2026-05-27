# Tengan CUA

Rust helper for desktop control on Windows, Ubuntu/Linux, and macOS where Codex
CLI receives the screenshot.

## Flow

1. Capture the selected monitor with `xcap`.
2. Send that PNG to Codex CLI using `codex exec --image <png>`.
3. Ask Codex to return a structured action plan with screenshot-relative coordinates.
4. Optionally execute the plan with `enigo`.

The app captures screenshot pixel dimensions separately from desktop coordinate
dimensions. That keeps clicks aligned on Windows scaling, macOS Retina displays,
and Ubuntu/Linux fractional scaling.

## Platform Setup

Install Rust and the Codex CLI on every platform.

Ubuntu/Debian build dependencies:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libclang-dev libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev libegl-dev libxkbcommon-dev
```

Linux notes:

- X11 is the most reliable input-control path.
- Wayland screenshot capture uses the desktop portal or compositor support and
  may prompt for permission.
- Wayland input simulation depends on compositor support. If execution is
  blocked, use an X11 session for `--execute`.

macOS notes:

- Grant Screen Recording permission to the terminal app running `cargo`.
- Grant Accessibility permission before using `--execute`.

## Commands

List monitor indexes:

```sh
cargo run -- monitors
```

Capture the primary monitor:

```sh
cargo run -- capture
```

Capture every monitor:

```sh
cargo run -- capture --all-monitors
```

Ask Codex where to click, without moving the mouse:

```sh
cargo run -- ask-codex "click the Save button"
```

Ask Codex and execute the returned action plan:

```sh
cargo run -- ask-codex "click the Save button" --execute
```

When `--execute` runs actions, the app prints a colored transcript with each
mouse, keyboard, or scroll action before it is performed.

Run the Stake/Tengan domain agent once in dry-run mode:

```sh
cargo run -- stake-agent --monitor 1 --once
```

Run the Stake/Tengan domain agent and execute only validated click actions:

```sh
cargo run -- stake-agent --monitor 1 --execute
```

The Stake agent loads `Agents.md`, asks Codex for structured game state plus
semantic actions using `schemas/stake_agent.schema.json`, validates hard
guardrails before execution, persists session counters in
`runs/stake-agent-state.json`, and performs a verification observation after
executed actions. Use `--reset-state` after a stopped session.

Use a specific monitor:

```sh
cargo run -- ask-codex "click the search box" --monitor 1 --execute
```

Continuously monitor the primary monitor on Linux/macOS:

```sh
sh ./watch-monitor.sh
```

Continuously monitor a specific monitor on Linux/macOS:

```sh
sh ./watch-monitor.sh --monitor 1
```

Continuously monitor a specific monitor on Windows:

```powershell
.\watch-monitor.ps1 -Monitor 1
```

Continuously monitor and execute allowed actions with a transcript file on
Linux/macOS:

```sh
sh ./watch-monitor.sh --execute --transcript-file ./transcript.log
```

Continuously monitor and execute allowed actions with a transcript file on
Windows:

```powershell
.\watch-monitor.ps1 -Monitor 1 -Execute -TranscriptFile .\transcript.log
```

Send every monitor to Codex:

```sh
cargo run -- ask-codex "what do you see on the screen" --all-monitors
```

Execute a previously saved plan:

```sh
cargo run -- execute runs/codex-action-123.json --origin-x 0 --origin-y 0
```

For saved plans from a scaled display, reuse the `scale=(x, y)` values printed
with the screenshot:

```sh
cargo run -- execute runs/codex-action-123.json --origin-x 0 --origin-y 0 --scale-x 2 --scale-y 2
```

Direct absolute click:

```sh
cargo run -- click 540 320
```

## Targeting Model

Codex receives the screenshot file directly through `--image`. The prompt tells Codex:

- return JSON matching `schemas/codex_action.schema.json`
- use coordinates relative to the screenshot image pixels
- include `monitor_index` for every coordinate action
- explain ambiguity instead of guessing

The Rust side converts screenshot pixel coordinates to desktop coordinates using
the captured image size and monitor desktop size, then adds the selected
monitor's desktop origin before executing actions.
