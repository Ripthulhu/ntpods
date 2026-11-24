#!/usr/bin/env python3
"""Verify this client's pinned dongle protocol exports without network access."""
import hashlib
import json
from pathlib import Path
import sys


def digest(path):
    return hashlib.sha256(path.read_text(encoding="utf-8").encode("utf-8")).hexdigest()


def check(root):
    lock = json.loads((root / "dongle-protocol.lock.json").read_text(encoding="utf-8"))
    if lock.get("version") != 1 or lock.get("repository") != "https://github.com/Ripthulhu/airpods-dongle":
        raise ValueError("Unsupported dongle protocol lock")
    files = lock["files"]
    if len(files) != 3:
        raise ValueError("Expected generated bindings, vectors and verifier")
    for name, expected in files.items():
        path = (root / name).resolve()
        if not path.is_relative_to(root.resolve()) or digest(path) != expected:
            raise ValueError(f"Dongle protocol export changed: {name}; regenerate from the pinned firmware specification")


if __name__ == "__main__":
    try:
        check(Path(__file__).resolve().parents[1])
    except (OSError, ValueError, KeyError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    print("Pinned dongle protocol bindings and vectors verified")
