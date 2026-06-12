#!/usr/bin/env python3
"""Chat-completions bridge for the gyne-agent consumer.

Accepts POST /v1/chat/completions, runs `tengan-cua ask-codex` against the
selected monitor with the task text as the instruction, and returns the
resulting action plan shaped like a chat completion (the format the
openclaw:results consumers already expect).

Environment:
  BRIDGE_PORT       Port to listen on (default: 18790)
  MONITOR           Monitor index to capture (default: primary monitor)
  EXECUTE_ACTIONS   Set to 1 to pass --execute so validated actions are
                    performed. Default: dry-run (observe and plan only).
  TENGAN_CUA_BIN    Path to the tengan-cua binary
                    (default: ./target/release/tengan-cua next to this file)
"""

import json
import os
import re
import subprocess
import sys
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PORT = int(os.environ.get("BRIDGE_PORT", "18790"))
MONITOR = os.environ.get("MONITOR", "").strip()
EXECUTE = os.environ.get("EXECUTE_ACTIONS", "").strip() == "1"
BIN = os.environ.get("TENGAN_CUA_BIN", str(ROOT / "target" / "release" / "tengan-cua"))

PLAN_LINE = re.compile(r"^plan=(.+)$", re.MULTILINE)


def run_ask_codex(instruction):
    cmd = [BIN, "ask-codex", instruction]
    if MONITOR:
        cmd += ["--monitor", MONITOR]
    if EXECUTE:
        cmd += ["--execute"]
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=600,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"tengan-cua exited with {proc.returncode}: "
            f"{(proc.stderr or proc.stdout)[-500:]}"
        )
    match = PLAN_LINE.search(proc.stdout)
    if not match:
        raise RuntimeError(f"no plan path in tengan-cua output: {proc.stdout[-500:]}")
    with open(match.group(1).strip(), "r", encoding="utf-8") as fh:
        return json.load(fh)


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path.rstrip("/") != "/v1/chat/completions":
            self.send_json(404, {"error": "not found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
            messages = body.get("messages", [])
            instruction = next(
                (
                    m.get("content", "")
                    for m in reversed(messages)
                    if m.get("role") == "user" and m.get("content")
                ),
                "",
            ).strip()
            if not instruction:
                self.send_json(400, {"error": "no user message content"})
                return

            print(f"[bridge] task: {instruction[:120]!r}", flush=True)
            plan = run_ask_codex(instruction)
            summary = plan.get("summary", "")
            response = {
                "id": f"tengan-cua-{uuid.uuid4()}",
                "object": "chat.completion",
                "created": int(time.time()),
                "model": body.get("model", "openclaw"),
                "choices": [
                    {
                        "index": 0,
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": summary},
                    }
                ],
                "output_text": summary,
                "actions": plan.get("actions", []),
                "confidence": plan.get("confidence"),
                "executed": EXECUTE,
            }
            self.send_json(200, response)
            print(
                f"[bridge] done: confidence={plan.get('confidence')} "
                f"actions={len(plan.get('actions', []))} executed={EXECUTE}",
                flush=True,
            )
        except Exception as err:  # noqa: BLE001 - report any failure to the consumer
            print(f"[bridge] error: {err}", file=sys.stderr, flush=True)
            self.send_json(500, {"error": str(err)})

    def send_json(self, status, payload):
        data = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        print(f"[bridge] {self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    mode = "EXECUTE" if EXECUTE else "dry-run"
    monitor = MONITOR or "primary"
    print(
        f"[bridge] listening on 127.0.0.1:{PORT} monitor={monitor} mode={mode} bin={BIN}",
        flush=True,
    )
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
