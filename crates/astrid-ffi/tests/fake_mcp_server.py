#!/usr/bin/env python3
"""Minimal stdio MCP server for tests: handshake + tools/list + tools/call.

Speaks JSON-RPC 2.0, one object per line, the MCP stdio transport. Serves a
single `echo` tool so a probe can assert a real handshake end to end.
"""
import json
import sys

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the input text back",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    }
]


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    method = req.get("method")
    rid = req.get("id")
    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": rid,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake-demo", "version": "1.0.0"},
            },
        })
    elif method == "notifications/initialized":
        pass  # a notification: no response
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        args = (req.get("params", {}) or {}).get("arguments", {}) or {}
        send({
            "jsonrpc": "2.0",
            "id": rid,
            "result": {
                "content": [{"type": "text", "text": f"echo: {args.get('text', '')}"}],
                "isError": False,
            },
        })
