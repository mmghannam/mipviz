#!/usr/bin/env python3
"""Split Mittelmann concatenated solver logs into per-instance .txt.gz files.

Mittelmann's milp_log12/ directory holds one big concatenated log per solver,
e.g. modified.copt.12threads.7200s.out.gz. Each instance's section starts with a
line like:

    @01 modified/p_30n20b8.mps.gz ===========

This splits such a file into static/<out-dir>/<solver>/<instance>.txt.gz, one gz
per instance (matching the layout the benchmark pages expect), starting at that
instance's @01 line and ending just before the next one.

Usage:
    python3 scripts/split_milp_logs.py \
        --in  modified.copt.12threads.7200s.out.gz \
        --solver copt \
        --out-dir logs-2026-07

Filename -> solver key for the July 2026 run:
    copt -> copt, fscip -> xsmoo, highs -> highs, highsp -> highs_p,
    optverse -> optverse, scip -> scip_spx, scipco -> scip_conc
"""

import argparse
import gzip
import os
import re
import sys

HEADER_RE = re.compile(r'^@01\s+modified/p_(.+?)\.mps\.gz')
STATIC_DIR = os.path.join(os.path.dirname(__file__), '..', 'static')


def split(in_path, solver, out_dir, dry_run=False):
    out_root = os.path.join(STATIC_DIR, out_dir, solver)
    if not dry_run:
        os.makedirs(out_root, exist_ok=True)

    current_name = None
    buf = []
    written = 0
    sizes = {}

    def flush():
        nonlocal written
        if current_name is None:
            return
        data = ''.join(buf)
        sizes[current_name] = len(data.encode('utf-8', 'replace'))
        if not dry_run:
            out_path = os.path.join(out_root, current_name + '.txt.gz')
            with gzip.open(out_path, 'wt', encoding='utf-8', errors='replace') as f:
                f.write(data)
        written += 1

    with gzip.open(in_path, 'rt', errors='replace') as f:
        for line in f:
            m = HEADER_RE.match(line)
            if m:
                flush()
                current_name = m.group(1)
                buf = [line]
            elif current_name is not None:
                buf.append(line)
            # lines before the first @01 (machine preamble) are dropped
    flush()

    return written, sizes


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--in', dest='in_path', required=True, help='concatenated .out.gz')
    ap.add_argument('--solver', required=True, help='internal solver key (e.g. copt)')
    ap.add_argument('--out-dir', required=True, help='dir under static/ (e.g. logs-2026-07)')
    ap.add_argument('--dry-run', action='store_true', help='measure only, write nothing')
    args = ap.parse_args()

    written, sizes = split(args.in_path, args.solver, args.out_dir, args.dry_run)
    total = sum(sizes.values())
    top = sorted(sizes.items(), key=lambda kv: kv[1], reverse=True)[:8]
    print(f'{args.solver}: {written} instances, {total/1e6:.1f} MB uncompressed total'
          + (' (dry run)' if args.dry_run else ''))
    print('  largest: ' + ', '.join(f'{n} {s/1e6:.1f}MB' for n, s in top))
    if written != 240:
        print(f'  WARNING: expected 240 instances, got {written}', file=sys.stderr)


if __name__ == '__main__':
    main()
