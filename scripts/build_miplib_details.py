#!/usr/bin/env python3
"""Merge per-instance JSONs from cache/miplib-details/ into a single
miplib-details.json file, ready to be committed to the mmghannam/mipviz-instances
repo and consumed by the frontend.

Usage:
    python scripts/build_miplib_details.py              # -> cache/miplib-details.json
    python scripts/build_miplib_details.py -o path.json
"""

import argparse
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE_DIR = os.path.join(ROOT, "cache", "miplib-details")
DEFAULT_OUT = os.path.join(ROOT, "cache", "miplib-details.json")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("-o", "--output", default=DEFAULT_OUT, help="Output path")
    args = ap.parse_args()

    if not os.path.isdir(CACHE_DIR):
        sys.exit(f"Cache directory not found: {CACHE_DIR}")

    merged = {}
    files = sorted(f for f in os.listdir(CACHE_DIR) if f.endswith(".json"))
    for fname in files:
        path = os.path.join(CACHE_DIR, fname)
        with open(path) as f:
            try:
                data = json.load(f)
            except json.JSONDecodeError as e:
                print(f"skip {fname}: {e}", file=sys.stderr)
                continue
        name = data.get("name") or fname[:-5]
        merged[name] = data

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "w") as f:
        json.dump(merged, f, indent=2, ensure_ascii=False, sort_keys=True)
    print(f"Wrote {len(merged)} instances to {args.output}")


if __name__ == "__main__":
    main()
