#!/usr/bin/env python3
"""Build a times-only benchmark details JSON from a Mittelmann .res table.

Mittelmann's MILP benchmark page (https://plato.asu.edu/ftp/milp.html) links a
per-instance results table (e.g. milp_tables/12threads.res). That table has only
solve times — no solver logs — so this produces a details JSON in the same shape
the pages consume, with every derived metric (nodes, presolve, LP bounds) left
null. Per-instance problem stats (_stats) are borrowed from an existing full
details JSON when available, since the underlying MIP is unchanged.

Usage:
    python3 scripts/scrape_milp_res.py \
        --res 12threads.res \
        --out static/benchmark-2026-07-details.json \
        --stats-from static/benchmark-12threads-details.json

Download the .res first, e.g.:
    curl -s https://plato.asu.edu/ftp/milp_tables/12threads.res -o 12threads.res
"""

import argparse
import json
import re
import sys

# Map a .res column header to our internal solver key. The July 2026 table uses
# these headers; extend this map when Mittelmann's lineup changes.
COLUMN_TO_KEY = {
    'COPT': 'copt',
    'FiberSCIP-cpx': 'xsmoo',   # listed on the page as XSMOO / Smoothie[FiberSCIP-cpx+HiGHS]
    'HiGHS': 'highs',
    'HiGHSp': 'highs_p',
    'optverse': 'optverse',
    'SCIP-spx': 'scip_spx',
    'SCIP-conc': 'scip_conc',
}

# Values in the table that mean "no solve time" (unsolved within the time limit
# or a failure). All map to null.
NON_TIMES = {'timeout', 'abort', 'fail', 'error', 'memlimit', '--', '-'}

DETAIL_KEYS = ['nodes', 'presolved_rows', 'presolved_cols',
               'lp_dual', 'lp_primal', 'root_dual', 'root_primal', 'final_primal']


def parse_res(path):
    """Parse a Mittelmann .res table into {instance_key: {solver_key: time_or_none}}."""
    with open(path, errors='replace') as f:
        lines = f.readlines()

    header = None
    columns = None
    result = {}

    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        # Header row: "  Name | COPT | FiberSCIP-cpx | ..."
        if header is None and stripped.startswith('Name') and '|' in line:
            header = line
            columns = [c.strip() for c in line.split('|')[1:] if c.strip()]
            unknown = [c for c in columns if c not in COLUMN_TO_KEY]
            if unknown:
                print('Warning: unmapped columns (ignored): ' + ', '.join(unknown),
                      file=sys.stderr)
            continue
        if columns is None:
            continue
        # Data rows start with an instance name (p_<name>); skip separators/footers.
        if not stripped.startswith('p_'):
            continue
        parts = stripped.split()
        raw_name = parts[0]
        values = parts[1:]
        if len(values) != len(columns):
            print(f'Warning: {raw_name} has {len(values)} values for '
                  f'{len(columns)} columns; skipping', file=sys.stderr)
            continue
        inst_key = raw_name[2:] + '.mps.gz'  # strip "p_" prefix
        row = {}
        for col, val in zip(columns, values):
            key = COLUMN_TO_KEY.get(col)
            if key is None:
                continue
            row[key] = parse_time(val)
        result[inst_key] = row

    if header is None:
        raise SystemExit('Could not find a header row (line starting with "Name") in ' + path)
    return result, [COLUMN_TO_KEY[c] for c in columns if c in COLUMN_TO_KEY]


def parse_time(val):
    v = val.strip()
    if v.lower() in NON_TIMES:
        return None
    try:
        f = float(v)
        return int(f) if f.is_integer() else f
    except ValueError:
        return None


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--res', required=True, help='path to the Mittelmann .res table')
    ap.add_argument('--out', required=True, help='output details JSON path')
    ap.add_argument('--stats-from', default=None,
                    help='existing details JSON to borrow per-instance _stats from')
    args = ap.parse_args()

    times, solver_keys = parse_res(args.res)

    stats_src = {}
    if args.stats_from:
        with open(args.stats_from) as f:
            src = json.load(f)
        stats_src = {k: v.get('_stats') for k, v in src.items() if isinstance(v, dict)}

    out = {}
    stats_matched = 0
    for inst_key, row in times.items():
        entry = {}
        stats = stats_src.get(inst_key)
        if stats is not None:
            stats_matched += 1
            entry['_stats'] = stats
        else:
            entry['_stats'] = {'rows': None, 'cols': None, 'nonzeros': None,
                               'binary': None, 'integer': None, 'continuous': None,
                               'obj_sense': 'minimize', 'best_obj': None}
        for key in solver_keys:
            solver_entry = {'time': row.get(key)}
            for dk in DETAIL_KEYS:
                solver_entry[dk] = None
            entry[key] = solver_entry
        out[inst_key] = entry

    with open(args.out, 'w') as f:
        json.dump(out, f, separators=(',', ':'))

    print(f'Parsed {len(out)} instances, {len(solver_keys)} solvers: {", ".join(solver_keys)}')
    print(f'  _stats matched from source: {stats_matched}/{len(out)}')
    print(f'Written to {args.out}')

    if args.stats_from:
        missing = sorted(set(stats_src) - set(times))
        extra = sorted(set(times) - set(stats_src))
        if missing:
            print(f'  In source but not in .res ({len(missing)}): ' + ', '.join(missing[:10]) +
                  ('...' if len(missing) > 10 else ''), file=sys.stderr)
        if extra:
            print(f'  In .res but not in source ({len(extra)}): ' + ', '.join(extra[:10]) +
                  ('...' if len(extra) > 10 else ''), file=sys.stderr)


if __name__ == '__main__':
    main()
