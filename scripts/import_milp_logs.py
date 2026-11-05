#!/usr/bin/env python3
"""Import a MILP snapshot directly from Mittelmann's downloaded solver logs.

Example: python3 scripts/import_milp_logs.py --downloads ~/Downloads --version 2026-10
Times come from solver summaries, rather than the published .res table. Only
proved optimal solutions with successful solution checks, or proved infeasible
instances, receive a solve time.
The version manifest is maintained separately so labels reflect the actual builds.
"""
import argparse
import gzip
import json
import math
from pathlib import Path
import re

from extract_2026_07_details import PARSERS, DETAIL_KEYS
from split_milp_logs import split

STATIC = Path(__file__).resolve().parent.parent / 'static'
FILES = {'copt': 'copt', 'fscip': 'xsmoo', 'highs': 'highs',
         'highsp': 'highs_p', 'optverse': 'optverse', 'scip': 'scip_spx',
         'scipco': 'scip_conc'}


def solve_time(text, solver):
    patterns = {
        'copt': (r'^MIP status\s*:\s*solved\s*$', r'^Solve time\s*:\s*([\d.]+)'),
        'optverse': (r'^\s*Status\s+Optimal solution found\s*$', r'^\s*Time\s+([\d.]+)'),
        'highs': (r'^\s*Status\s+Optimal\s*$', r'^\s*Timing\s+([\d.]+)'),
        'highs_p': (r'^\s*Status\s+Optimal\s*$', r'^\s*Timing\s+([\d.]+)'),
        'scip_spx': (r'^SCIP Status\s*:\s*problem is solved \[optimal solution found\]', r'^Solving Time \(sec\)\s*:\s*([\d.]+)'),
        'scip_conc': (r'^SCIP Status\s*:\s*problem is solved \[optimal solution found\]', r'^Solving Time \(sec\)\s*:\s*([\d.]+)'),
        'xsmoo': (r'^SCIP Status\s*:\s*problem is solved\s*$', r'^Total Time\s*:\s*([\d.]+)'),
    }
    status, timing = patterns[solver]
    infeasible_patterns = {
        'copt': r'^Solution status\s*:\s*infeasible\s*$',
        'optverse': r'^\s*Status\s+Problem is infeasible\s*$',
        'highs': r'^\s*Status\s+Infeasible\s*$',
        'highs_p': r'^\s*Status\s+Infeasible\s*$',
    }
    infeasible = re.search(infeasible_patterns.get(solver,
        r'^SCIP Status\s*:\s*problem is solved \[infeasible\]'), text, re.MULTILINE)
    if not infeasible and not re.search(status, text, re.MULTILINE):
        return None
    if not infeasible and not re.search(r'^Check SOL: Integrality 1 Constraints 1 Objective 1\s*$', text, re.MULTILINE):
        return None
    # FiberSCIP's generic "solved" status needs a zero-gap check as well.
    if solver == 'xsmoo' and not infeasible and not re.search(r'^Gap\s*:\s*0(?:\.0*)?\s*%', text, re.MULTILINE):
        return None
    match = re.search(timing, text, re.MULTILINE)
    if match is None:
        raise ValueError(f'{solver}: optimal summary has no solve time')
    value = float(match[1])
    return value if math.isfinite(value) and 0 <= value < 7200 else None


def metrics(text, solver):
    values = dict(zip(DETAIL_KEYS, PARSERS[solver](text)))
    # Concurrent progress is not confined to the root node. FiberSCIP's final
    # dual bound is also not a root bound. Do not expose either as such.
    if solver in ('scip_conc', 'xsmoo'):
        values['root_dual'] = values['root_primal'] = None
    if solver == 'xsmoo':
        match = re.search(r'^\s*nodes \(total\)\s*:\s*(\d+)', text, re.MULTILINE)
        values['nodes'] = int(match[1]) if match else None
    return {'time': solve_time(text, solver), **values}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--downloads', type=Path, required=True)
    ap.add_argument('--version', required=True)
    ap.add_argument('--stats-from', type=Path, default=STATIC / 'benchmark-2026-07-details.json')
    args = ap.parse_args()
    if not re.fullmatch(r'\d{4}-\d{2}', args.version):
        ap.error('--version must be YYYY-MM')
    stats = json.loads(args.stats_from.read_text())
    archives = {}
    for filename, solver in FILES.items():
        matches = list(args.downloads.glob(f'modified.{filename}.*threads.7200s.out.gz'))
        if len(matches) != 1:
            ap.error(f'expected exactly one {filename} archive; found {len(matches)}')
        archives[solver] = matches[0]
    out_dir = 'logs-' + args.version
    data = {key: {'_stats': entry['_stats']} for key, entry in stats.items()}
    for solver, archive in archives.items():
        count, sizes = split(archive, solver, out_dir)
        expected = {key.removesuffix('.mps.gz') for key in data}
        if count != len(expected) or set(sizes) != expected:
            raise ValueError(f'{solver}: logs do not match the {len(expected)} benchmark instances')
        solved = 0
        for key, entry in data.items():
            path = STATIC / out_dir / solver / (key.removesuffix('.mps.gz') + '.txt.gz')
            with gzip.open(path, 'rt', errors='replace') as f:
                entry[solver] = metrics(f.read(), solver)
            solved += entry[solver]['time'] is not None
        print(f'{solver}: {count} logs, {solved} solved', flush=True)
    output = STATIC / f'benchmark-{args.version}-details.json'
    output.write_text(json.dumps(data, separators=(',', ':'), allow_nan=False) + '\n')
    print(f'Written {output}')


if __name__ == '__main__':
    main()
