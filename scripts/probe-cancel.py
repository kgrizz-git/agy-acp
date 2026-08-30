#!/usr/bin/env python3
"""Check that cancelling a turn really stops the command agy is running.

The unit tests in `src/proc.rs` prove the kill mechanism against a synthetic
process tree; this drives the real adapter against the real `agy`, which is how
the original defect was found and how the fix was confirmed. Nothing here runs in
CI: it needs `agy` in PATH, working auth, and about two minutes.

    cargo build
    python3 scripts/probe-cancel.py

It asks agy to run `sleep 45 && touch <marker>`, approves the permission request,
cancels mid-command, and prints the process tree before and after. A pass is: no
`sleep 45` after the cancel, and no marker file 50 seconds later. A fail is
silent in the transcript and visible only here — the host is told "cancelled"
either way.
"""

import json
import pathlib
import shutil
import subprocess
import sys
import threading
import time

REPO = pathlib.Path(__file__).resolve().parent.parent
BINARY = REPO / "target/debug/agy-acp"
WORKSPACE = pathlib.Path("/tmp/agy-cancel-probe")
COMMAND_SECONDS = 45


def process_tree(tag):
    print(f"--- {tag} ---", flush=True)
    ps = subprocess.run(
        ["ps", "-eo", "pid,ppid,pgid,command"], capture_output=True, text=True
    ).stdout
    for line in ps.splitlines():
        if ("agy " in line or "sleep 45" in line or "agy-acp" in line) and "grep" not in line:
            print(line[:130], flush=True)


def main():
    if not BINARY.exists():
        sys.exit(f"build it first: {BINARY} does not exist")
    shutil.rmtree(WORKSPACE, ignore_errors=True)
    WORKSPACE.mkdir()
    marker = WORKSPACE / "marker.txt"

    adapter = subprocess.Popen(
        [str(BINARY), "--permission-prompts"],
        cwd=WORKSPACE,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    def send(message):
        adapter.stdin.write(json.dumps(message) + "\n")
        adapter.stdin.flush()

    state = {"session": None, "tool_seen": False}

    def read_adapter():
        for line in adapter.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            method = message.get("method")
            if method == "session/request_permission":
                options = message["params"]["options"]
                allow = next(
                    (o for o in options if o["kind"] == "allow_once"), options[0]
                )
                print("approving:", json.dumps(message["params"].get("toolCall", {}))[:120], flush=True)
                send({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "result": {"outcome": {"outcome": "selected", "optionId": allow["optionId"]}},
                })
            elif method == "session/update":
                if "tool_call" in json.dumps(message["params"]):
                    state["tool_seen"] = True
            elif message.get("id") == 2:
                state["session"] = message["result"]["sessionId"]
            elif message.get("id") == 3:
                print("prompt result:", json.dumps(message.get("result")), flush=True)

    threading.Thread(target=read_adapter, daemon=True).start()

    send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
          "params": {"protocolVersion": 1, "clientCapabilities": {}}})
    send({"jsonrpc": "2.0", "id": 2, "method": "session/new",
          "params": {"cwd": str(WORKSPACE), "mcpServers": []}})
    deadline = time.time() + 30
    while state["session"] is None and time.time() < deadline:
        time.sleep(0.2)
    if state["session"] is None:
        sys.exit("no session")

    send({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {
        "sessionId": state["session"],
        "prompt": [{"type": "text", "text":
                    f"Run exactly this one shell command and then stop: "
                    f"sleep {COMMAND_SECONDS} && touch {marker} . Do not do anything else."}],
    }})

    deadline = time.time() + 150
    while not state["tool_seen"] and time.time() < deadline:
        time.sleep(0.5)
    if not state["tool_seen"]:
        sys.exit("agy never started the command")
    time.sleep(4)
    process_tree("before cancel")

    send({"jsonrpc": "2.0", "id": 4, "method": "session/cancel",
          "params": {"sessionId": state["session"]}})
    time.sleep(3)
    process_tree("after cancel")

    time.sleep(COMMAND_SECONDS + 5)
    process_tree("after the command would have finished")
    adapter.kill()
    print("marker exists:", marker.exists(), flush=True)
    sys.exit(0 if not marker.exists() else "FAIL: the cancelled command ran to completion")


if __name__ == "__main__":
    main()
