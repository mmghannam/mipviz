#!/usr/bin/env python3
"""Fill the July 2026 details JSON with metrics parsed from the split logs.

Reads the times-only static/benchmark-2026-07-details.json (produced by
scrape_milp_res.py) and the per-instance logs under static/logs-2026-07/, then
writes the derived metrics (nodes, presolve size, LP/root bounds, final primal)
back into that JSON — the same shape extract_benchmark_details.py produces for
the February run.

The four solvers shared with February (copt, optverse, scip_spx, highs) reuse
the serial parsers from extract_benchmark_details.py. The three new solvers use
their own format:
    scip_conc — concurrent SCIP (compact time|mem|dual|primal|gap table)
    xsmoo     — Smoothie / FiberSCIP-cpx (SCIP-style summary blocks)
    highs_p   — parallel HiGHS (wider B&B table)

Run scripts/split_milp_logs.py for every solver first.
"""

import gzip
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
from extract_benchmark_details import (  # noqa: E402
    parse_float, parse_copt, parse_optverse, parse_scip, parse_highs,
)

STATIC = os.path.join(os.path.dirname(__file__), '..', 'static')
LOGS_DIR = os.path.join(STATIC, 'logs-2026-07')
DETAILS_PATH = os.path.join(STATIC, 'benchmark-2026-07-details.json')

DETAIL_KEYS = ['nodes', 'presolved_rows', 'presolved_cols', 'lp_dual',
               'lp_primal', 'root_dual', 'root_primal', 'final_primal']


def _presolved_from_scip_block(text):
    """Rows/cols from a SCIP 'Presolved Problem :' summary block."""
    m = re.search(
        r'Presolved Problem\s*:\s*\n\s*Variables\s*:\s*(\d+).*?\n\s*Constraints\s*:\s*(\d+)',
        text)
    if m:
        return int(m.group(2)), int(m.group(1))  # rows, cols
    return None, None


def parse_scip_conc(text):
    nodes = presolved_rows = presolved_cols = None
    lp_dual = lp_primal = root_dual = root_primal = final_primal = None

    m = re.search(r'Solving Nodes\s*:\s*(\d+)', text)
    if m:
        nodes = int(m.group(1))

    presolved_rows, presolved_cols = _presolved_from_scip_block(text)

    # Root LP relaxation bound, printed in the statistics footer.
    m = re.search(r'First LP value\s*:\s*(\S+)', text)
    if m:
        lp_dual = parse_float(m.group(1))

    # Compact concurrent table: " 10.2s|1141M| 1.510000e+02 | 4.530000e+02 | ..."
    for line in text.splitlines():
        m = re.match(r'^\s*[\d.]+s\|\s*\S+\|\s*(\S+)\s*\|\s*(\S+)\s*\|', line)
        if not m:
            continue
        db = parse_float(m.group(1))
        pb = parse_float(m.group(2))
        if db is not None:
            root_dual = db
            root_primal = pb

    m = re.search(r'^Primal Bound\s*:\s*(\S+)', text, re.MULTILINE)
    if m:
        final_primal = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_xsmoo(text):
    nodes = presolved_rows = presolved_cols = None
    lp_dual = lp_primal = root_dual = root_primal = final_primal = None

    presolved_rows, presolved_cols = _presolved_from_scip_block(text)

    m = re.search(r'^\s*Primal Bound\s*:\s*(\S+)', text, re.MULTILINE)
    if m:
        final_primal = parse_float(m.group(1))
    m = re.search(r'^\s*Dual Bound\s*:\s*(\S+)', text, re.MULTILINE)
    if m:
        root_dual = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_highs_p(text):
    nodes = presolved_rows = presolved_cols = None
    lp_dual = lp_primal = root_dual = root_primal = final_primal = None

    m = re.search(r'^\s*Nodes\s+(\d+)', text, re.MULTILINE)
    if m:
        nodes = int(m.group(1))

    # "Presolve reductions: rows 488(-88); columns 4733(-13647); nonzeros ..."
    m = re.search(r'Presolve reductions:\s*rows\s+(\d+).*?columns\s+(\d+)', text)
    if m:
        presolved_rows = int(m.group(1))
        presolved_cols = int(m.group(2))

    m = re.search(r'^\s*Primal bound\s+(\S+)', text, re.MULTILINE)
    if m:
        final_primal = parse_float(m.group(1))

    # Parallel B&B table, root rows have Proc == 0:
    # "[Src] Proc InQueue Leaves Expl%  BestBound  BestSol  Gap  Cuts InLp Confl LpIters Time"
    for line in text.splitlines():
        m = re.match(
            r'^\s*(?:[A-Za-z]\s+)?0\s+\d+\s+\d+\s+[\d.]+%\s+(\S+)\s+(\S+)\s+\S+\s+(\d+)\s+(\d+)\s+',
            line)
        if not m:
            continue
        db = parse_float(m.group(1))
        pb = parse_float(m.group(2))
        cuts = int(m.group(3))
        inlp = int(m.group(4))
        if db is not None and lp_dual is None and cuts == 0 and inlp == 0:
            lp_dual = db
            lp_primal = pb
        if db is not None:
            root_dual = db
            root_primal = pb

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


PARSERS = {
    'copt': parse_copt,
    'optverse': parse_optverse,
    'scip_spx': parse_scip,
    'scip_conc': parse_scip_conc,
    'xsmoo': parse_xsmoo,
    'highs': parse_highs,
    'highs_p': parse_highs_p,
}


def main():
    with open(DETAILS_PATH) as f:
        data = json.load(f)

    solvers = [k for k in PARSERS]
    coverage = {s: {k: 0 for k in DETAIL_KEYS} for s in solvers}
    counts = {s: 0 for s in solvers}

    for inst_key, entry in data.items():
        base = inst_key.replace('.mps.gz', '')
        for solver in solvers:
            if solver not in entry:
                continue
            log_path = os.path.join(LOGS_DIR, solver, base + '.txt.gz')
            if not os.path.exists(log_path):
                continue
            counts[solver] += 1
            try:
                with gzip.open(log_path, 'rt', errors='replace') as f:
                    text = f.read()
                vals = PARSERS[solver](text)
            except Exception as e:
                print(f'  warn: {solver}/{base}: {e}', file=sys.stderr)
                continue
            for k, v in zip(DETAIL_KEYS, vals):
                entry[solver][k] = v
                if v is not None:
                    coverage[solver][k] += 1

    with open(DETAILS_PATH, 'w') as f:
        json.dump(data, f, separators=(',', ':'))

    print(f'Updated {DETAILS_PATH}')
    for s in solvers:
        n = counts[s] or 1
        c = coverage[s]
        print(f'  {s:10s} logs={counts[s]:3d}  nodes={c["nodes"]:3d} '
              f'presolve={c["presolved_rows"]:3d} lp_dual={c["lp_dual"]:3d} '
              f'root_dual={c["root_dual"]:3d} primal={c["final_primal"]:3d}')


if __name__ == '__main__':
    main()
