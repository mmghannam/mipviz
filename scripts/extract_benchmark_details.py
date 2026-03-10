#!/usr/bin/env python3
"""Extract nodes and presolve size from solver log files.

Reads gzipped logs from static/logs/{solver}/ and writes
static/benchmark-12threads-details.json with per-solver metrics.
"""

import gzip
import json
import os
import re
import sys

LOGS_DIR = os.path.join(os.path.dirname(__file__), '..', 'static', 'logs')
OUT_PATH = os.path.join(os.path.dirname(__file__), '..', 'static', 'benchmark-12threads-details.json')
TIMES_PATH = os.path.join(os.path.dirname(__file__), '..', 'static', 'benchmark-12threads.json')

SOLVERS = ['highs', 'scip_spx', 'scipc_cpx', 'copt', 'optverse']


def parse_float(s):
    """Parse a float from solver output, returning None for non-numeric values."""
    s = s.strip()
    if s in ('--', '-', 'inf', '-inf', '+inf', 'Inf', '-Inf', '+Inf', 'Large'):
        return None
    try:
        v = float(s)
        if v != v or v == float('inf') or v == float('-inf'):
            return None
        return v
    except ValueError:
        return None


def parse_highs(text):
    nodes = None
    presolved_rows = None
    presolved_cols = None
    lp_dual = None
    lp_primal = None
    root_dual = None
    root_primal = None

    # "Nodes             9"
    m = re.search(r'^\s*Nodes\s+(\d+)', text, re.MULTILINE)
    if m:
        nodes = int(m.group(1))

    # "Solving MIP model with:\n   336 rows\n   6130 cols"
    m = re.search(r'Solving MIP model with:\s*\n\s*(\d+)\s+rows\s*\n\s*(\d+)\s+cols', text)
    if m:
        presolved_rows = int(m.group(1))
        presolved_cols = int(m.group(2))

    # HiGHS B&B table lines (node 0):
    # Capture: BestBound, BestSol, Cuts, LpIters
    bb_lines = re.findall(
        r'^\s*[A-Z ]?\s*0\s+0\s+\d+\s+[\d.]+%\s+(\S+)\s+(\S+)\s+\S+\s+(\d+)\s+\d+\s+\d+\s+(\d+)',
        text, re.MULTILINE
    )
    for bound_s, sol_s, cuts_s, lpiters_s in bb_lines:
        db = parse_float(bound_s)
        pb = parse_float(sol_s)
        cuts = int(cuts_s)
        lpiters = int(lpiters_s)
        if db is not None and lp_dual is None and lpiters > 0 and cuts == 0:
            lp_dual = db
            lp_primal = pb
        if db is not None:
            root_dual = db
            root_primal = pb

    # Final primal bound: "  Primal bound      26374"
    final_primal = None
    m = re.search(r'^\s*Primal bound\s+(\S+)', text, re.MULTILINE)
    if m:
        final_primal = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_scip(text):
    nodes = None
    presolved_rows = None
    presolved_cols = None
    lp_dual = None
    lp_primal = None
    root_dual = None
    root_primal = None

    # "Solving Nodes      :        362"
    m = re.search(r'Solving Nodes\s*:\s*(\d+)', text)
    if m:
        nodes = int(m.group(1))

    # "presolved problem has 6170 variables (...)  and 342 constraints"
    m = re.search(r'presolved problem has\s+(\d+)\s+variables.*?and\s+(\d+)\s+constraints', text)
    if m:
        presolved_cols = int(m.group(1))
        presolved_rows = int(m.group(2))

    # SCIP B&B table lines at node 1 (root):
    # time | node | left | LPiter | LPit/n | mem | mdpt | vars | cons | rows | cuts | sepa | confs | strbr | dualbound | primalbound | gap | compl
    # "  1.2s|     1 |     0 | 10368 |     - |   104M |   0 |6170 | 662 | 342 |   0 |  0 | 320 |   0 | 2.587761e+04 |      --      |    Inf | unknown"
    # We split each node-1 line by | and extract fields by position
    for line in text.splitlines():
        m = re.match(r'^\s*[\*o ]?\s*[\d.]+s\|\s+1\s+\|', line)
        if not m:
            continue
        parts = line.split('|')
        if len(parts) < 18:
            continue
        cuts_s = parts[10].strip()
        sepa_s = parts[11].strip()
        dual_s = parts[14].strip()
        prim_s = parts[15].strip()
        db = parse_float(dual_s)
        pb = parse_float(prim_s)
        cuts = int(cuts_s) if cuts_s.isdigit() else -1
        sepa = int(sepa_s) if sepa_s.isdigit() else -1
        # LP relaxation: first line at root with cuts=0 and sepa=0
        if db is not None and lp_dual is None and cuts == 0 and sepa == 0:
            lp_dual = db
            lp_primal = pb
        if db is not None:
            root_dual = db
            root_primal = pb

    # Final primal bound: "Primal Bound       : +2.63740000000000e+04 (13 solutions)"
    final_primal = None
    m = re.search(r'^Primal Bound\s*:\s*(\S+)', text, re.MULTILINE)
    if m:
        final_primal = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_copt(text):
    nodes = None
    presolved_rows = None
    presolved_cols = None
    lp_dual = None
    lp_primal = None
    root_dual = None
    root_primal = None

    # "Solve node      : 186"
    m = re.search(r'Solve node\s*:\s*(\d+)', text)
    if m:
        nodes = int(m.group(1))

    # "The presolved problem has:\n    341 rows, 6083 columns"
    m = re.search(r'The presolved problem has:\s*\n\s*(\d+)\s+rows,\s*(\d+)\s+columns', text)
    if m:
        presolved_rows = int(m.group(1))
        presolved_cols = int(m.group(2))

    # COPT B&B table lines at node 0:
    # "         0         1      --       0 -5.584800e+04            --     Inf  0.34s"  (pre-LP, IntInf=0)
    # "         0         1      --     222  2.587761e+04            --     Inf  0.42s"  (LP relaxation)
    # Columns: Nodes Active LPit/n IntInf BestBound BestSolution Gap Time
    bb_lines = re.findall(
        r'^\s*[H* ]?\s*0\s+\d+\s+\S+\s+(\d+)\s+(\S+)\s+(\S+)\s+',
        text, re.MULTILINE
    )
    for intinf_s, bound_s, sol_s in bb_lines:
        intinf = int(intinf_s)
        db = parse_float(bound_s)
        pb = parse_float(sol_s)
        # LP relaxation: first line with IntInf > 0 (LP has been solved)
        if db is not None and lp_dual is None and intinf > 0:
            lp_dual = db
            lp_primal = pb
        if db is not None:
            root_dual = db
            root_primal = pb

    # Final primal bound: "Best solution   : 26374.000000000"
    final_primal = None
    m = re.search(r'Best solution\s*:\s*(\S+)', text)
    if m:
        final_primal = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_optverse(text):
    nodes = None
    presolved_rows = None
    presolved_cols = None
    lp_dual = None
    lp_primal = None
    root_dual = None
    root_primal = None

    # "  Node                 623"  or in summary table "Node  623"
    m = re.search(r'Node\s+(\d+)', text)
    if m:
        nodes = int(m.group(1))

    # "After presolve:\n  345 rows, 6260 columns"
    m = re.search(r'After presolve:\s*\n\s*(\d+)\s+rows,\s*(\d+)\s+columns', text)
    if m:
        presolved_rows = int(m.group(1))
        presolved_cols = int(m.group(2))

    # Optverse B&B table lines at Solved=0 (root node):
    # "     0.5s         0          0       --   0.000000e+00        --          --"  (pre-LP)
    # "     1.6s         0          0       --   2.587761e+04   2.686400e+04    3.67%"
    # Columns: Time Solved Open It/Node BestBound BestSol Gap
    bb_lines = re.findall(
        r'^\s*[H ]?\s*[\d.]+s\s+0\s+\d+\s+\S+\s+(\S+)\s+(\S+)\s+',
        text, re.MULTILINE
    )
    for bound_s, sol_s in bb_lines:
        db = parse_float(bound_s)
        pb = parse_float(sol_s)
        # LP relaxation: first non-zero dual bound (skip trivial 0 bound)
        if db is not None and lp_dual is None and db != 0.0:
            lp_dual = db
            lp_primal = pb
        if db is not None:
            root_dual = db
            root_primal = pb

    # Final primal bound: "  Best solution        2.637400000000e+04"
    final_primal = None
    m = re.search(r'Best solution\s+(\S+)', text)
    if m:
        final_primal = parse_float(m.group(1))

    return nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal


def parse_instance_stats(logs_dir, base_name):
    """Extract original problem stats from whichever solver log is available.

    Returns dict with rows, cols, nonzeros, binary, integer, continuous or None values.
    Tries SCIP first (cleanest format), then COPT, then HiGHS, then Optverse.
    """
    stats = {'rows': None, 'cols': None, 'nonzeros': None,
             'binary': None, 'integer': None, 'continuous': None}

    # Try HiGHS first (has all stats including nonzeros):
    # "MIP name has 426 rows; 7195 cols; 52121 nonzeros; 7195 integer variables (7195 binary)"
    log_path = os.path.join(logs_dir, 'highs', base_name + '.txt.gz')
    if os.path.exists(log_path):
        try:
            with gzip.open(log_path, 'rt', errors='replace') as f:
                text = f.read()
            m = re.search(
                r'has\s+(\d+)\s+rows;\s*(\d+)\s+cols;\s*(\d+)\s+nonzeros;\s*(\d+)\s+integer variables\s+\((\d+)\s+binary\)',
                text
            )
            if m:
                stats['rows'] = int(m.group(1))
                stats['cols'] = int(m.group(2))
                stats['nonzeros'] = int(m.group(3))
                total_int = int(m.group(4))
                stats['binary'] = int(m.group(5))
                stats['integer'] = total_int - stats['binary']
                stats['continuous'] = stats['cols'] - total_int
                return stats
        except Exception:
            pass

    # Fallback: SCIP "original problem has 7195 variables (7195 bin, 0 int, 0 cont) and 426 constraints"
    for solver in ['scip_spx', 'scipc_cpx']:
        log_path = os.path.join(logs_dir, solver, base_name + '.txt.gz')
        if not os.path.exists(log_path):
            continue
        try:
            with gzip.open(log_path, 'rt', errors='replace') as f:
                text = f.read()
            m = re.search(
                r'original problem has\s+(\d+)\s+variables\s+\((\d+)\s+bin,\s*(\d+)\s+int,\s*(\d+)\s+cont\)\s+and\s+(\d+)\s+constraints',
                text
            )
            if m:
                stats['cols'] = int(m.group(1))
                stats['binary'] = int(m.group(2))
                stats['integer'] = int(m.group(3))
                stats['continuous'] = int(m.group(4))
                stats['rows'] = int(m.group(5))
                return stats
        except Exception:
            continue

    return stats


PARSERS = {
    'highs': parse_highs,
    'scip_spx': parse_scip,
    'scipc_cpx': parse_scip,
    'copt': parse_copt,
    'optverse': parse_optverse,
}


def main():
    # Load the existing times JSON to know which instances to expect
    with open(TIMES_PATH) as f:
        times_data = json.load(f)

    result = {}

    for instance_key in times_data:
        base_name = instance_key.replace('.mps.gz', '')
        result[instance_key] = {
            '_stats': parse_instance_stats(LOGS_DIR, base_name)
        }

        for solver in SOLVERS:
            time_val = times_data[instance_key].get(solver)
            entry = {'time': time_val, 'nodes': None, 'presolved_rows': None, 'presolved_cols': None,
                     'lp_dual': None, 'lp_primal': None, 'root_dual': None, 'root_primal': None,
                     'final_primal': None}

            log_path = os.path.join(LOGS_DIR, solver, base_name + '.txt.gz')
            if os.path.exists(log_path):
                try:
                    with gzip.open(log_path, 'rt', errors='replace') as f:
                        text = f.read()
                    parser = PARSERS[solver]
                    nodes, presolved_rows, presolved_cols, lp_dual, lp_primal, root_dual, root_primal, final_primal = parser(text)
                    entry['nodes'] = nodes
                    entry['presolved_rows'] = presolved_rows
                    entry['presolved_cols'] = presolved_cols
                    entry['lp_dual'] = lp_dual
                    entry['lp_primal'] = lp_primal
                    entry['root_dual'] = root_dual
                    entry['root_primal'] = root_primal
                    entry['final_primal'] = final_primal
                except Exception as e:
                    print(f'  Warning: failed to parse {log_path}: {e}', file=sys.stderr)

            result[instance_key][solver] = entry

        # Compute best known objective (minimum final_primal across solvers for minimization)
        primals = [result[instance_key][s]['final_primal'] for s in SOLVERS
                   if result[instance_key][s]['final_primal'] is not None]
        result[instance_key]['_stats']['obj_sense'] = 'minimize'  # all MIPLIB instances are minimization
        result[instance_key]['_stats']['best_obj'] = min(primals) if primals else None

    # Stats
    total = 0
    nodes_found = 0
    presolve_found = 0
    lp_dual_found = 0
    root_dual_found = 0
    for inst in result.values():
        for key, solver_data in inst.items():
            if key == '_stats':
                continue
            total += 1
            if solver_data['nodes'] is not None:
                nodes_found += 1
            if solver_data['presolved_rows'] is not None:
                presolve_found += 1
            if solver_data['lp_dual'] is not None:
                lp_dual_found += 1
            if solver_data['root_dual'] is not None:
                root_dual_found += 1

    print(f'Parsed {total} solver-instance pairs')
    print(f'  Nodes found: {nodes_found}/{total}')
    print(f'  Presolve found: {presolve_found}/{total}')
    print(f'  LP dual bound found: {lp_dual_found}/{total}')
    print(f'  Root dual bound found: {root_dual_found}/{total}')

    with open(OUT_PATH, 'w') as f:
        json.dump(result, f, separators=(',', ':'))

    print(f'Written to {OUT_PATH}')


if __name__ == '__main__':
    main()
