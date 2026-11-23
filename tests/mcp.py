"""Exercise the stdio MCP server against a headless Wayland session."""

import base64
from io import BytesIO
import json
from pathlib import Path
import subprocess
import sys

from PIL import Image

from smoke import HERE, run_session, wait_for

BIN = Path("target/debug").resolve()


def call_tools(directory, calls, protocol_version="2025-11-25"):
    def message(method, params, request_id=None):
        payload = {"jsonrpc": "2.0", "method": method, "params": params}
        if request_id is not None:
            payload["id"] = request_id
        return json.dumps(payload)

    messages = [
        message("initialize", {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": {"name": "rscu-mcp-smoke", "version": "1"},
        }, 0),
        message("notifications/initialized", {}),
        message("tools/list", {}, 1),
    ]
    messages.extend(
        message("tools/call", {"name": name, "arguments": args}, index)
        for index, (name, args) in enumerate(calls, 2)
    )
    process = subprocess.run(
        [BIN / "rscu-mcp", "--socket", directory / "control"],
        input="\n".join(messages) + "\n", text=True, capture_output=True, timeout=10,
    )
    assert process.returncode == 0, process.stderr
    replies = [json.loads(line) for line in process.stdout.splitlines()]
    assert [reply["id"] for reply in replies] == list(range(len(calls) + 2)), replies
    assert replies[0]["result"]["protocolVersion"] == protocol_version
    names = {tool["name"] for tool in replies[1]["result"]["tools"]}
    assert {"windows", "screenshot", "click", "key"} <= names
    return replies[2:]


def metadata(reply):
    assert not reply["result"].get("isError"), reply
    return json.loads(reply["result"]["content"][0]["text"])


def check(ctl, directory):
    window = wait_for(lambda: ctl("windows"))[0]["id"]
    windows, frame = call_tools(directory, [
        ("windows", {}),
        ("screenshot", {"window": window, "crop": [20, 30, 5, 6]}),
    ])
    assert metadata(windows)[0]["id"] == window
    captured = metadata(frame)
    image = frame["result"]["content"][1]
    assert image["type"] == "image" and image["mimeType"] == "image/png"
    with Image.open(BytesIO(base64.b64decode(image["data"], validate=True))) as png:
        assert png.size == (5, 6)
    unchanged, clicked, invalid, unknown = call_tools(directory, [
        ("screenshot", {"window": window, "crop": [20, 30, 5, 6], "since": captured["image"]}),
        ("click", {"image": captured["image"], "x": 1, "y": 1}),
        ("click", {"image": captured["image"], "x": 99, "y": 99}),
        ("stop", {}),
    ], protocol_version="2024-11-05")
    assert metadata(unchanged)["unchanged"] and len(unchanged["result"]["content"]) == 1
    assert metadata(clicked)["sent"] is True
    assert invalid["result"]["isError"]
    assert unknown["error"]["code"] == -32602
    wait_for(lambda: "press 1 1" in (directory / "events").read_text()
             if (directory / "events").exists() else False)


if __name__ == "__main__":
    run_session(
        BIN / "rscu", "pixman",
        lambda directory: ["--wayland", sys.executable, str(HERE / "mouse_client.py"),
                           str(directory / "events")],
        check,
    )
    print("Passed MCP stdio, Wayland window listing, PNG, unchanged capture and GTK click checks")
