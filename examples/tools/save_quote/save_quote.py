#!/usr/bin/env python3
"""Prompt Box tool: append a quote to quotes.sqlite next to this script.

Input (stdin, JSON): {"arguments": {"quote": ..., "author": ..., "source": ...},
                      "prompt": "<the whole prompt text>"}
Output (stdout, JSON): {"message": "...", "replace_prompt": "..."} (both optional)
Exit non-zero with a message on stderr to report failure.
"""
import json
import pathlib
import sqlite3
import sys

payload = json.load(sys.stdin)
args = payload.get("arguments", {})
quote = (args.get("quote") or "").strip()
if not quote:
    sys.exit("no quote in the arguments")

db = pathlib.Path(__file__).with_name("quotes.sqlite")
con = sqlite3.connect(db)
con.execute(
    "CREATE TABLE IF NOT EXISTS quotes ("
    " id INTEGER PRIMARY KEY, quote TEXT NOT NULL, author TEXT, source TEXT,"
    " prompt TEXT, saved_at TEXT DEFAULT CURRENT_TIMESTAMP)"
)
con.execute(
    "INSERT INTO quotes (quote, author, source, prompt) VALUES (?, ?, ?, ?)",
    (quote, args.get("author"), args.get("source"), payload.get("prompt")),
)
con.commit()
count = con.execute("SELECT COUNT(*) FROM quotes").fetchone()[0]
who = f" ({args['author']})" if args.get("author") else ""
print(json.dumps({"message": f"saved \"{quote[:40]}\"{who}; {count} in the database"}))
