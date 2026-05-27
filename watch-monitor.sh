#!/bin/sh
set -u

MONITOR=""
INTERVAL_SECONDS=10
CONTEXT_FILE=""
INSTRUCTION="Inspect this monitor and summarize important changes. Return an empty actions array unless action is explicitly required."
EXECUTE=0
ONCE=0
TRANSCRIPT_FILE=""
CODEX_BIN="codex"

usage() {
    cat <<'EOF'
Usage: sh ./watch-monitor.sh [options]

Continuously monitor one Linux or macOS display with the Tengan CUA Codex workflow.

Options:
  -m, --monitor <index>          Monitor index from `cargo run -- monitors` (default: primary)
  -i, --interval-seconds <sec>   Delay between monitor checks (default: 10)
  -c, --context-file <path>      Context file to reload each loop (default: ./context.txt)
  -n, --instruction <text>       Current task appended after the context
  -e, --execute                  Execute returned mouse/keyboard actions
      --once                     Run one check and exit
  -t, --transcript-file <path>   Tee console output to a transcript file
      --codex-bin <path>         Codex executable (default: codex)
  -h, --help                     Show this help

Examples:
  sh ./watch-monitor.sh
  sh ./watch-monitor.sh --monitor 1
  sh ./watch-monitor.sh --monitor 1 --interval-seconds 30 --instruction "Watch for error dialogs and report them."
  sh ./watch-monitor.sh --monitor 1 --execute --instruction "If a visible error dialog appears, click OK. Otherwise do nothing."
  sh ./watch-monitor.sh --monitor 1 --execute --transcript-file ./transcript.log
EOF
}

script_dir() {
    cd -P "$(dirname "$0")" >/dev/null 2>&1 && pwd
}

require_value() {
    if [ "$#" -lt 2 ]; then
        echo "Missing value for $1" >&2
        usage >&2
        exit 2
    fi
}

cleanup_transcript() {
    exec >/dev/null 2>&1
    wait "$TEE_PID" 2>/dev/null || true
}

stop_transcript() {
    trap - EXIT INT TERM
    cleanup_transcript
    exit 130
}

SCRIPT_DIR="$(script_dir)"
PROJECT_ROOT="$SCRIPT_DIR"

if [ -z "$CONTEXT_FILE" ]; then
    CONTEXT_FILE="$PROJECT_ROOT/context.txt"
fi

while [ "$#" -gt 0 ]; do
    case "$1" in
        -m|--monitor)
            require_value "$@"
            MONITOR="$2"
            shift 2
            ;;
        -i|--interval-seconds)
            require_value "$@"
            INTERVAL_SECONDS="$2"
            shift 2
            ;;
        -c|--context-file)
            require_value "$@"
            CONTEXT_FILE="$2"
            shift 2
            ;;
        -n|--instruction)
            require_value "$@"
            INSTRUCTION="$2"
            shift 2
            ;;
        -e|--execute)
            EXECUTE=1
            shift
            ;;
        --once)
            ONCE=1
            shift
            ;;
        -t|--transcript-file)
            require_value "$@"
            TRANSCRIPT_FILE="$2"
            shift 2
            ;;
        --codex-bin)
            require_value "$@"
            CODEX_BIN="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

CYAN="$(printf '\033[36m')"
DARK_CYAN="$(printf '\033[2;36m')"
YELLOW="$(printf '\033[33m')"
RESET="$(printf '\033[0m')"

if [ -n "$TRANSCRIPT_FILE" ]; then
    mkdir -p "$(dirname "$TRANSCRIPT_FILE")"
    TRANSCRIPT_FIFO="${TMPDIR:-/tmp}/tengan-cua-transcript-$$.fifo"
    mkfifo "$TRANSCRIPT_FIFO"
    tee -a "$TRANSCRIPT_FILE" < "$TRANSCRIPT_FIFO" &
    TEE_PID=$!
    exec > "$TRANSCRIPT_FIFO" 2>&1
    rm -f "$TRANSCRIPT_FIFO"
    trap cleanup_transcript EXIT
    trap stop_transcript INT TERM
fi

cd "$PROJECT_ROOT" || exit 1

if [ ! -f "$CONTEXT_FILE" ]; then
    cat > "$CONTEXT_FILE" <<'EOF'
You are monitoring this desktop screen.
Only report important changes.
Do not click, type, scroll, or move the mouse unless the user explicitly asks for an action.
Return an empty actions array unless an action is explicitly required.
EOF
fi

if [ -n "$MONITOR" ]; then
    printf '%bWatching monitor %s every %s seconds.%b\n' "$CYAN" "$MONITOR" "$INTERVAL_SECONDS" "$RESET"
else
    printf '%bWatching primary monitor every %s seconds.%b\n' "$CYAN" "$INTERVAL_SECONDS" "$RESET"
fi
printf '%bContext: %s%b\n' "$DARK_CYAN" "$CONTEXT_FILE" "$RESET"
if [ -n "$TRANSCRIPT_FILE" ]; then
    printf '%bTranscript file: %s%b\n' "$DARK_CYAN" "$TRANSCRIPT_FILE" "$RESET"
fi
if [ "$ONCE" -eq 0 ]; then
    printf '%bPress Ctrl+C to stop.%b\n' "$YELLOW" "$RESET"
fi

while true; do
    CONTEXT="$(cat "$CONTEXT_FILE")"
    PROMPT="${CONTEXT}

Current task: ${INSTRUCTION}"
    TIMESTAMP="$(date '+%Y-%m-%d %H:%M:%S')"

    if [ -n "$MONITOR" ]; then
        printf '\n%b[%s] Capturing monitor %s...%b\n' "$CYAN" "$TIMESTAMP" "$MONITOR" "$RESET"
    else
        printf '\n%b[%s] Capturing primary monitor...%b\n' "$CYAN" "$TIMESTAMP" "$RESET"
    fi

    if [ -n "$MONITOR" ]; then
        if [ "$EXECUTE" -eq 1 ]; then
            cargo run -- ask-codex "$PROMPT" --monitor "$MONITOR" --codex-bin "$CODEX_BIN" --execute
            STATUS=$?
        else
            cargo run -- ask-codex "$PROMPT" --monitor "$MONITOR" --codex-bin "$CODEX_BIN"
            STATUS=$?
        fi
    else
        if [ "$EXECUTE" -eq 1 ]; then
            cargo run -- ask-codex "$PROMPT" --codex-bin "$CODEX_BIN" --execute
            STATUS=$?
        else
            cargo run -- ask-codex "$PROMPT" --codex-bin "$CODEX_BIN"
            STATUS=$?
        fi
    fi

    if [ "$STATUS" -ne 0 ]; then
        printf '%bWarning: cargo exited with code %s%b\n' "$YELLOW" "$STATUS" "$RESET" >&2
    fi

    if [ "$ONCE" -eq 1 ]; then
        exit "$STATUS"
    fi

    sleep "$INTERVAL_SECONDS"
done
