#!/usr/bin/env python3
"""Demo MCP stdio server for Astrid (MCP `stdio` transport).

Speaks JSON-RPC 2.0 on stdin/stdout: `initialize` handshake,
`notifications/initialized`, `tools/list`, `tools/call`. Ships three tools:

* time_now   — current local time (no arguments)
* new_uuid   — a fresh UUIDv4 (no arguments)
* echo_back  — echoes {message} back

Wire it in .astrid/config.toml:

    [mcp]
    enabled = true
    [[mcp.servers]]
    name = "demo"
    command = "python3"
    args = ["/absolute/path/to/examples/demo-mcp-server.py"]
"""

import json
import sys
import uuid
from datetime import datetime

PROTOCOL_VERSION = "2024-11-05"

TOOLS = [
    {
        "name": "time_now",
        "description": "Current local time (ISO-8601 with timezone offset)",
        "inputSchema": {"type": "object", "additionalProperties": False},
    },
    {
        "name": "new_uuid",
        "description": "Generate a fresh UUIDv4 string",
        "inputSchema": {"type": "object", "additionalProperties": False},
    },
    {
        "name": "echo_back",
        "description": "Echo the given message back with a prefix",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
        },
    },
]


def send(msg_id, result=None, error=None):
    resp = {"jsonrpc": "2.0", "id": msg_id}
    if error is not None:
        resp["error"] = error
    else:
        resp["result"] = result
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()


def handle(msg):
    method = msg.get("method", "")
    if "id" not in msg:
        return  # notification (e.g. notifications/initialized) — nothing to answer
    msg_id = msg["id"]

    if method == "initialize":
        return send(msg_id, {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "astrid-demo", "version": "0.1.0"},
        })
    if method == "tools/list":
        return send(msg_id, {"tools": TOOLS})
    if method == "tools/call":
        name = msg["params"]["name"]
        args = msg["params"].get("arguments", {})
        if name == "time_now":
            text = datetime.now().astimezone().isoformat(timespec="seconds")
        elif name == "new_uuid":
            text = str(uuid.uuid4())
        elif name == "echo_back":
            text = "echo: " + str(args.get("message", ""))
        else:
            return send(msg_id, error={
                "code": -32602,
                "message": f"Unknown tool: {name}",
            })
        return send(msg_id, {"content": [{"type": "text", "text": text}], "isError": False})
    # ping and anything else: an empty result keeps lenient clients happy
    return send(msg_id, {})


def main():
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        handle(msg)


if __name__ == "__main__":
    main()
