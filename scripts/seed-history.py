#!/usr/bin/env python3
"""Write a deterministic paste-buff history file for fair benchmarking.

Usage: python3 -I scripts/seed-history.py OUT.json [--entries N]
"""
import argparse
import hashlib
import json
import os
import random
import struct
import time
import zlib

SNIPPETS = [
    "4155 Cheyenne Court\nWaukee, Iowa 50263",
    "https://docs.rs/gpui/latest/gpui/",
    "cargo build --release && ./target/release/paste-buff-gpui",
    "fn main() {\n    println!(\"hello from paste-buff\");\n}",
    "SELECT id, text, copied_at_ms FROM history ORDER BY copied_at_ms DESC LIMIT 50;",
    "The quick brown fox jumps over the lazy dog. " * 12,
    "jacob@roush.io",
    "https://github.com/tauri-apps/tauri/releases",
    "ssh -L 11434:localhost:11434 win-ollama",
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor "
    "incididunt ut labore et dolore magna aliqua. " * 6,
    "{\"model\": \"claude-fable-5-1\", \"max_tokens\": 1024}",
    "TODO: compare footprint after 100 cycles",
    "#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct Entry { pub id: u64 }",
    "openssl rand -hex 32",
    "Meeting notes\n- memory is the headline\n- cycles reveal leaks\n- release builds only",
]
APPS = ["Safari", "Terminal", "Zed", "Slack", "Firefox Developer Edition", "Mail", None]


def png_bytes(width: int, height: int) -> bytes:
    """A gradient PNG with no dependencies, roughly screenshot-sized."""
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            rows += bytes((x * 255 // width, y * 255 // height, (x + y) % 256))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(rows), 6))
            + chunk(b"IEND", b""))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--entries", type=int, default=200)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--no-media", action="store_true", help="text entries only")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    now_ms = int(time.time() * 1000)
    span_ms = 23 * 60 * 60 * 1000
    entries = []
    if not args.no_media:
        image_dir = os.path.join(os.path.dirname(os.path.abspath(args.out)), "images")
        os.makedirs(image_dir, exist_ok=True)
        png = png_bytes(1600, 1000)
        image_path = os.path.join(image_dir, hashlib.sha1(png).hexdigest()[:16] + ".png")
        with open(image_path, "wb") as f:
            f.write(png)
        entries.append({
            "id": args.entries + 2, "text": "Image 1600×1000", "copied_at_ms": now_ms,
            "times_copied": 1, "source_app": "Screenshot",
            "content": {"kind": "image", "file": image_path, "width": 1600, "height": 1000,
                        "bytes": len(png)},
        })
        files = ["/Applications/Safari.app", "/System/Library/CoreServices/Finder.app"]
        entries.append({
            "id": args.entries + 1, "text": "\n".join(files), "copied_at_ms": now_ms - 30_000,
            "times_copied": 1, "source_app": "Finder",
            "content": {"kind": "files", "paths": files},
        })
    for i in range(args.entries):
        base = SNIPPETS[i % len(SNIPPETS)]
        text = f"{base}\n#{i}" if i >= len(SNIPPETS) else base
        entries.append({
            "id": i + 1,
            "text": text,
            "copied_at_ms": now_ms - 60_000 - int(span_ms * (i / max(1, args.entries))),
            "times_copied": rng.choice([1, 1, 1, 2, 3]),
            "source_app": rng.choice(APPS),
        })
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump({"version": 1, "entries": entries}, f, ensure_ascii=False)
    print(f"wrote {len(entries)} entries to {args.out}")


if __name__ == "__main__":
    main()
