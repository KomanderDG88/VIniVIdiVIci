"""Synthetic runner tests; no game files, GPU or Rust compilation required."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('lab_suite', Path(__file__).resolve().parents[1] / 'tools/lab_suite.py')
lab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lab)


class SuiteTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def test_exit_code_and_timeout_are_not_success(self):
        failed = lab.run_command([sys.executable, '-c', 'raise SystemExit(7)'], self.root, self.root / 'failed', 5)
        self.assertEqual(failed['exit_code'], 7)
        self.assertEqual(failed['status'], 'failed')
        timed = lab.run_command([sys.executable, '-c', 'import time; time.sleep(30)'], self.root, self.root / 'timeout', .1)
        self.assertEqual(timed['status'], 'timeout')
        self.assertIsNotNone(timed['exit_code'])

    def test_arguments_are_not_shell_commands(self):
        text = 'literal ; & $(not-a-command) spaces'
        result = lab.run_command([sys.executable, '-c', 'import sys; print(sys.argv[1])', text], self.root, self.root / 'args', 5)
        self.assertEqual(result['status'], 'passed')
        self.assertEqual((self.root / 'args/stdout.log').read_text().strip(), text)

    def test_timeout_stops_the_owned_descendant_too(self):
        marker = self.root / 'orphan-wrote.txt'
        descendant = 'import time; from pathlib import Path; time.sleep(1); Path(' + repr(str(marker)) + ').write_text("orphan")'
        parent = 'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",' + repr(descendant) + ']); time.sleep(30)'
        result = lab.run_command([sys.executable, '-c', parent], self.root, self.root / 'tree', .3)
        self.assertEqual(result['status'], 'timeout')
        time.sleep(1.1)
        self.assertFalse(marker.exists(), 'Timed-out runner left its child running')

    def test_output_inside_any_checkout_is_rejected(self):
        repo = self.root / 'repo'
        repo.mkdir()
        (repo / '.git').mkdir()
        with self.assertRaises(ValueError):
            lab.outside_repository(repo / 'ignored', repo)
        other = self.root / 'other'
        other.mkdir()
        (other / '.git').write_text('gitdir: elsewhere')
        with self.assertRaises(ValueError):
            lab.outside_repository(other / 'reports', repo)

    def test_rust_failure_and_ignored_counts(self):
        log = self.root / 'rust.log'
        log.write_text('test result: ok. 2 passed; 0 failed; 3 ignored;\n'
                       'test result: FAILED. 4 passed; 1 failed; 0 ignored;\n')
        self.assertEqual(lab.rust_results([log]), {'passed': 6, 'failed': 1, 'ignored': 3, 'summaries': 2})
        log.write_text('no test summary')
        self.assertEqual(lab.rust_results([log])['passed'], 0)

    def test_busy_gpu_never_enters_test_body(self):
        helper = self.root / 'busy.py'
        helper.write_text('raise SystemExit(1)\n')
        entered = False
        with self.assertRaises(RuntimeError):
            with lab.gpu_lease(helper, 'synthetic-owner'):
                entered = True
        self.assertFalse(entered)

    def test_gpu_lease_releases_when_check_raises(self):
        helper = self.root / 'lease.py'
        helper.write_text('import sys\nfrom pathlib import Path\n'
                          'Path(__file__).with_suffix(".state").write_text(sys.argv[1])\n')
        with self.assertRaisesRegex(RuntimeError, 'test failure'):
            with lab.gpu_lease(helper, 'synthetic-owner'):
                raise RuntimeError('test failure')
        self.assertEqual(helper.with_suffix('.state').read_text(), 'release')

    def runtime_fixture(self, lines, reason='time_limit'):
        run = self.root / 'run'
        run.mkdir()
        lab.write_json(run / 'launch.json', {'commit': 'synthetic', 'sha256': 'synthetic', 'console': 'series-s'})
        lab.write_json(run / 'result.json', {'reason': reason, 'exit_code': -1})
        (run / 'stderr.log').write_text(lines)
        (run / 'memory.jsonl').write_text(json.dumps({'ram_mb': 1024, 'process_private_gib': 2}) + '\n')
        return run

    def test_menu_fps_cannot_be_called_gameplay(self):
        run = self.runtime_fixture('2026-01-01T00:00:01Z 30 fps, 33 ms/frame, 13 draws/frame, dropped 0\n'
                                   '2026-01-01T00:00:02Z frame times: 2 frames in 0.1 s: each: 33 34\n')
        result = lab.runtime_summary(run)
        self.assertIsNone(result['high_draw_scene_candidate'])
        self.assertIsNone(result['logged_frame_fps'])
        self.assertEqual(result['player_control'], 'unverified')

    def test_scene_candidate_is_not_visual_proof_and_bad_batches_are_excluded(self):
        run = self.runtime_fixture('2026-01-01T00:00:01Z 2 fps, 500 ms/frame, 800 draws/frame, dropped 7\n'
                                   '2026-01-01T00:00:02Z frame times: 2 frames in 2.0 s: each: 10 10\n'
                                   '2026-01-01T00:00:04Z frame times: 2 frames in 1.0 s: each: 400 600\n'
                                   '2026-01-01T00:00:05Z frame times: 3 frames in 1.0 s: each: 100 200\n')
        result = lab.runtime_summary(run)
        self.assertEqual(result['logged_frame_fps'], 2)
        self.assertEqual(result['frames_sampled_after_candidate'], 2)
        self.assertEqual(result['malformed_batches'], 1)
        self.assertFalse(result['comparison_eligible'])
        self.assertEqual(result['visual_quality'], 'unverified')
        self.assertEqual(result['max_dropped_draws'], 7)


if __name__ == '__main__':
    unittest.main()
