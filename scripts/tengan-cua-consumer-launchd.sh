#!/bin/sh
set -eu

GYNE_ROOT="/Users/kennethphang/Projects/gyne-agent"
PROJECT_ROOT="/Users/kennethphang/Projects/tengan-cua"

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

if [ -f "$GYNE_ROOT/.env" ]; then
    set -a
    . "$GYNE_ROOT/.env"
    set +a
fi

export TASK_STREAM="${TASK_STREAM:-openclaw:tasks}"
export RESULT_STREAM="${RESULT_STREAM:-openclaw:results}"
export CONSUMER_NAME="tengan-cua-consumer"
export CONSUMER_TASK_STREAM="${TASK_STREAM}:${CONSUMER_NAME}"
export OPENCLAW_BASE_URL="http://127.0.0.1:18790/v1"
export OPENCLAW_CHAT_COMPLETIONS_URL="http://127.0.0.1:18790/v1/chat/completions"
export OPENCLAW_GATEWAY_TOKEN="local-bridge"

mkdir -p "$PROJECT_ROOT/runs/launchd"
cd "$GYNE_ROOT"

if command -v nc >/dev/null 2>&1; then
    i=0
    while [ "$i" -lt 30 ]; do
        if nc -z 127.0.0.1 18790 >/dev/null 2>&1; then
            break
        fi
        i=$((i + 1))
        sleep 1
    done
fi

exec "$GYNE_ROOT/target/release/consumer"
