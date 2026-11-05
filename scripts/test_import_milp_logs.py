import unittest

from import_milp_logs import solve_time, metrics

CHECK = '\nCheck SOL: Integrality 1 Constraints 1 Objective 1\n'


class LogImportTests(unittest.TestCase):
    def test_optimal_requires_solution_check(self):
        text = 'MIP status : solved\nSolve time : 2.15\n'
        self.assertIsNone(solve_time(text, 'copt'))
        self.assertEqual(solve_time(text + CHECK, 'copt'), 2.15)
        self.assertIsNone(solve_time(text + CHECK.replace('Constraints 1', 'Constraints 0'), 'copt'))

    def test_infeasible_is_solved_without_primal_solution(self):
        self.assertEqual(solve_time('Solution status : infeasible\nSolve time : 6.81\n', 'copt'), 6.81)
        self.assertEqual(solve_time('SCIP Status : problem is solved [infeasible]\nTotal Time : 3.25\n', 'xsmoo'), 3.25)

    def test_timeout_does_not_use_elapsed_time(self):
        self.assertIsNone(solve_time('Status Time limit reached\nTiming 7200.1\n' + CHECK, 'highs'))

    def test_fiberscip_requires_zero_gap_and_has_no_root_bound(self):
        text = 'SCIP Status : problem is solved\nTotal Time : 12.5\nGap : 0.000 %\n  Dual Bound : 302\n  nodes (total) : 1508\n' + CHECK
        self.assertEqual(solve_time(text, 'xsmoo'), 12.5)
        self.assertIsNone(solve_time(text.replace('0.000 %', '1.000 %'), 'xsmoo'))
        result = metrics(text, 'xsmoo')
        self.assertIsNone(result['root_dual'])
        self.assertEqual(result['nodes'], 1508)


if __name__ == '__main__':
    unittest.main()
