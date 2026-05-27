# Tengan CUA

Rust helper for Windows desktop control where Codex CLI receives the screenshot.

## Flow

1. Capture the selected monitor with `xcap`.
2. Send that PNG to Codex CLI using `codex exec --image <png>`.
3. Ask Codex to return a structured action plan with screenshot-relative coordinates.
4. Optionally execute the plan with `enigo`.

The app sets Windows DPI awareness at startup so screenshot coordinates and mouse coordinates line up on scaled monitors.

## Commands

List monitor indexes:

```powershell
cargo run -- monitors
```

Capture the primary monitor:

```powershell
cargo run -- capture
```

Capture every monitor:

```powershell
cargo run -- capture --all-monitors
```

Ask Codex where to click, without moving the mouse:

```powershell
cargo run -- ask-codex "click the Save button"
```

Ask Codex and execute the returned action plan:

```powershell
cargo run -- ask-codex "click the Save button" --execute
```

When `--execute` runs actions, the app prints a colored transcript with each
mouse, keyboard, or scroll action before it is performed.

Use a specific monitor:

```powershell
cargo run -- ask-codex "click the search box" --monitor 1 --execute
```

Continuously monitor one monitor:

```powershell
.\watch-monitor.ps1 -Monitor 1
```

On macOS:

```sh
sh ./watch-monitor.sh --monitor 1
```

Continuously monitor and execute allowed actions with a transcript file:

```powershell
.\watch-monitor.ps1 -Monitor 1 -Execute -TranscriptFile .\transcript.log
```

On macOS:

```sh
sh ./watch-monitor.sh --monitor 1 --execute --transcript-file ./transcript.log
```

Send every monitor to Codex:

```powershell
cargo run -- ask-codex "what do you see on the screen" --all-monitors
```

Execute a previously saved plan:

```powershell
cargo run -- execute runs\codex-action-123.json --origin-x 0 --origin-y 0
```

Direct absolute click:

```powershell
cargo run -- click 540 320
```

## Targeting Model

Codex receives the screenshot file directly through `--image`. The prompt tells Codex:

- return JSON matching `schemas/codex_action.schema.json`
- use coordinates relative to the screenshot image
- include `monitor_index` for every coordinate action
- explain ambiguity instead of guessing

The Rust side adds the selected monitor's desktop origin before executing actions, so monitor-local screenshot coordinates become absolute desktop coordinates.
