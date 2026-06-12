#!/bin/sh
set -eu

PROJECT_ROOT="/Users/kennethphang/Projects/tengan-cua"
PYTHON_BIN="${PYTHON_BIN:-/opt/homebrew/bin/python3}"

export PATH="/Users/kennethphang/.nvm/versions/node/v22.22.2/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export BRIDGE_PORT="${BRIDGE_PORT:-18790}"
export MONITOR="${MONITOR:-0}"
export EXECUTE_ACTIONS="${EXECUTE_ACTIONS:-0}"
export TENGAN_CUA_BIN="${TENGAN_CUA_BIN:-$PROJECT_ROOT/target/release/tengan-cua}"

if [ ! -x "$PYTHON_BIN" ]; then
    PYTHON_BIN="$(command -v python3)"
fi

mkdir -p "$PROJECT_ROOT/runs/launchd"
cd "$PROJECT_ROOT"

exec "$PYTHON_BIN" "$PROJECT_ROOT/cua-bridge.py"
