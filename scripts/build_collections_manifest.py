#!/usr/bin/env python3
"""Build the browser-facing collection manifest without using GitHub's API."""

import argparse
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("collections_dir", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    collections = []
    for path in sorted(args.collections_dir.glob("*.txt")):
        instances = [
            line.strip()
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]
        collections.append({"name": path.stem, "instances": instances})

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(collections, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
