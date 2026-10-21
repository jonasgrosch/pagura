"""Check orchestration without running Cargo, Node, fixtures or conformance."""
import contextlib
import io
import shlex
import subprocess
import sys
import unittest
from unittest.mock import call, patch

import check


class CommandPlanTests(unittest.TestCase):
    def test_programmatic_jobs_reject_nonpositive_and_noninteger_values(self):
        for jobs in (True, False, 0, -1, 1.5, '2'):
            with self.subTest(jobs=jobs), self.assertRaises(ValueError):
                check.planned_commands(jobs)

    def test_default_commands_keep_existing_concurrency_and_rustdoc_policy(self):
        commands = check.planned_commands()
        self.assertEqual(commands[-1], ([sys.executable, "tools/conformance.py"], {}))
        self.assertIn((['cargo', 'test', '--locked', '--offline', '--workspace', '--doc'], {}), commands)
        self.assertIn((['cargo', 'doc', '--locked', '--offline', '--workspace', '--no-deps'],
                       {'RUSTDOCFLAGS': '-D warnings'}), commands)
        for _, environment in commands:
            self.assertNotIn("CARGO_BUILD_JOBS", environment)
            self.assertNotIn("RUST_TEST_THREADS", environment)

    def test_jobs_reach_nested_builds_and_the_conformance_worker_option(self):
        for jobs in (1, 3):
            with self.subTest(jobs=jobs):
                commands = check.planned_commands(jobs)
                for _, environment in commands:
                    self.assertEqual(environment['CARGO_BUILD_JOBS'], str(jobs))
                    self.assertEqual(environment['RUST_TEST_THREADS'], str(jobs))
                self.assertEqual(commands[-1][0],
                                 [sys.executable, 'tools/conformance.py', '--jobs', str(jobs)])
                rustdoc = next(environment for command, environment in commands
                               if command[:2] == ['cargo', 'doc'])
                self.assertEqual(rustdoc['RUSTDOCFLAGS'], '-D warnings')

    def test_plans_do_not_mutate_each_other_or_the_default(self):
        baseline = check.planned_commands()
        changed = check.planned_commands(1)
        changed[0][0].append('not-a-real-option')
        changed[0][1]['RUSTDOCFLAGS'] = 'not-a-real-flag'
        self.assertEqual(check.planned_commands(), baseline)
        self.assertNotIn('not-a-real-option', check.planned_commands(2)[0][0])
        self.assertNotIn('RUSTDOCFLAGS', check.planned_commands(2)[0][1])

    def test_display_round_trips_paths_arguments_and_environment_values(self):
        command = ['/some path/python', 'tools/check.py', "literal 'quotes' and $HOME"]
        environment = {'RUSTDOCFLAGS': '-D warnings'}
        self.assertEqual(shlex.split(check.describe_command(command, environment)),
                         ['env', 'RUSTDOCFLAGS=-D warnings', *command])


class CheckMainTests(unittest.TestCase):
    def test_dry_run_prints_the_complete_plan_without_running_any_command(self):
        output = io.StringIO()
        with patch.object(check.subprocess, 'run') as run, contextlib.redirect_stdout(output):
            self.assertEqual(check.main(['--jobs', '1', '--dry-run']), 0)
        run.assert_not_called()
        lines = output.getvalue().splitlines()
        self.assertIn(str(check.ROOT), lines[0])
        self.assertIn('inherited', lines[1])
        actual = [shlex.split(line.removeprefix('Planned: '))
                  for line in lines if line.startswith('Planned: ')]
        expected = []
        for command, environment in check.planned_commands(1):
            expected.append(['env', *[f'{key}={value}' for key, value in sorted(environment.items())],
                             *command])
        self.assertEqual(actual, expected)
        self.assertNotIn('All checks passed', output.getvalue())

    def test_invalid_jobs_exit_before_execution(self):
        for value in ('0', '-1', 'many', '1.5'):
            with self.subTest(value=value), patch.object(check.subprocess, 'run') as run:
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    check.main(['--jobs', value])
                self.assertEqual(error.exception.code, 2)
                run.assert_not_called()

    def test_execution_keeps_order_cwd_and_overrides_inherited_concurrency(self):
        inherited = {'KEEP_FOR_CHILD': 'yes', 'CARGO_BUILD_JOBS': '8', 'RUST_TEST_THREADS': '9'}
        with patch.dict(check.os.environ, inherited, clear=True), \
                patch.object(check.subprocess, 'run') as run, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(check.main(['--jobs', '2']), 0)
        expected = [call(command, cwd=check.ROOT, check=True, env={**inherited, **environment})
                    for command, environment in check.planned_commands(2)]
        self.assertEqual(run.call_args_list, expected)

    def test_default_execution_preserves_inherited_concurrency(self):
        inherited = {'CARGO_BUILD_JOBS': '4', 'RUST_TEST_THREADS': '5'}
        with patch.dict(check.os.environ, inherited, clear=True), \
                patch.object(check.subprocess, 'run') as run, contextlib.redirect_stdout(io.StringIO()):
            check.main([])
        for invocation in run.call_args_list:
            self.assertEqual(invocation.kwargs['env']['CARGO_BUILD_JOBS'], '4')
            self.assertEqual(invocation.kwargs['env']['RUST_TEST_THREADS'], '5')
        self.assertEqual(run.call_args_list[-1].args[0], [sys.executable, 'tools/conformance.py'])

    def test_first_failure_propagates_and_stops_later_checks(self):
        failure = subprocess.CalledProcessError(7, [sys.executable, 'tools/fixtures.py', '--check'])
        output = io.StringIO()
        with patch.object(check.subprocess, 'run', side_effect=[None, failure]) as run, \
                contextlib.redirect_stdout(output):
            with self.assertRaises(subprocess.CalledProcessError) as error:
                check.main(['--jobs', '1'])
        self.assertIs(error.exception, failure)
        self.assertEqual(run.call_count, 2)
        self.assertTrue(all(invocation.kwargs['check'] for invocation in run.call_args_list))
        self.assertNotIn('All checks passed', output.getvalue())

    def test_help_does_not_claim_runner_reservation_or_control_browser_workers(self):
        output = io.StringIO()
        with patch.object(check.subprocess, 'run') as run, contextlib.redirect_stdout(output):
            with self.assertRaises(SystemExit) as error:
                check.main(['--help'])
        self.assertEqual(error.exception.code, 0)
        run.assert_not_called()
        help_text = ' '.join(output.getvalue().split())
        self.assertIn('does not reserve a runner', help_text)
        self.assertIn('Host/browser checks still create worker threads', help_text)


if __name__ == '__main__':
    unittest.main()
