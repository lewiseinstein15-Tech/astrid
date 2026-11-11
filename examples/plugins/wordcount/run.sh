#!/usr/bin/env bash
# Astrid plugin contract: the tool arguments arrive as ONE JSON string in $1
# and must be forwarded explicitly. Any interpreter works; this one uses
# python3 for JSON parsing.
exec python3 -c '
import json, sys
d = json.loads(sys.argv[1]) if len(sys.argv) > 1 else {}
text = str(d.get("text", ""))
words = len(text.split())
chars = len(text)
print(f"wordcount: {words} word(s), {chars} character(s)")
' "$1"
