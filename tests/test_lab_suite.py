"""Synthetic runner tests; no game files, GPU or Rust compilation required."""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
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

    def test_settings_names_values_and_reserved_switches_are_checked(self):
        for name in ('VICTORY_lower', 'OTHER_SWITCH', 'VICTORY_', 'VICTORY_A B'):
            with self.assertRaisesRegex(ValueError, 'name'):
                lab.case_settings({'name': 'c', 'settings': {name: '1'}})
        for value in ('a b', 'x;y', '"q"', "it's", 'v' * 65, True, 1.5, None, ['1']):
            with self.assertRaisesRegex(ValueError, 'VICTORY_X'):
                lab.case_settings({'name': 'c', 'settings': {'VICTORY_X': value}})
        for name in ('VICTORY_HEAP_SYNC_TABLES', 'VICTORY_INDIRECT_WINDOW', 'VICTORY_PROFILE',
                     'VICTORY_PROFILE_OUT', 'VICTORY_PROFILEX', 'VICTORY_FRAME_LOG', 'VICTORY_CAPTURE', 'VICTORY_KEYS',
                     'VICTORY_CARD_METER', 'VICTORY_STACKS', 'VICTORY_CONFIG', 'VICTORY_LOG'):
            with self.assertRaisesRegex(ValueError, name):
                lab.case_settings({'name': 'c', 'settings': {name: '1'}})
        with self.assertRaises(ValueError):
            lab.case_settings({'name': 'c', 'settings': ['VICTORY_X=1']})
        self.assertEqual(lab.case_settings({'name': 'c', 'settings': {'VICTORY_X': -3, 'VICTORY_Y': ''}}),
                         {'VICTORY_X': '-3', 'VICTORY_Y': ''})

    def test_settings_argument_is_ordered_and_absent_without_settings(self):
        settings = lab.case_settings({'name': 'c', 'settings': {'VICTORY_B': 2, 'VICTORY_A': 'x', 'VICTORY_C': 'a,b:1.5'}})
        self.assertEqual(lab.settings_argument(settings), 'VICTORY_A=x;VICTORY_B=2;VICTORY_C=a,b:1.5')
        self.assertEqual(lab.case_settings({'name': 'c'}), {})
        self.assertIsNone(lab.settings_argument({}))

    def runtime_case(self, settings, launched, keys=None, launched_keys=None, folders=None, launched_folders=None):
        """runtime_checks over a fake runner: returns (commands run, result rows)."""
        root = Path(tempfile.mkdtemp(dir=self.root))  # one per call: a test may run several cases
        binary = root / 'victory.exe'
        binary.write_bytes(b'synthetic')
        runner = root / 'runner.ps1'
        runner.write_text('# synthetic')
        case = {'name': 'one', 'binary': str(binary), 'commit': 'synthetic',
                'sha256': hashlib.sha256(b'synthetic').hexdigest(), 'max_seconds': 120}
        if settings is not None:
            case['settings'] = settings
        if keys is not None:
            case['keys'] = keys
        if folders is not None:
            case['run_folders'] = folders
        manifest = root / 'manifest.json'
        lab.write_json(manifest, {'runner': str(runner), 'lease_script': 'unused', 'cases': [case]})
        commands = []

        def fake_run(command, cwd, output, timeout, grace=180):
            commands.append(command)
            label = command[command.index('-Label') + 1]
            run = Path(command[command.index('-RunRoot') + 1]) / (label + '-1')
            run.mkdir(parents=True)
            launch = {'commit': 'synthetic', 'sha256': 'synthetic', 'settings': launched}
            if launched_keys is not None:
                launch['keys'] = launched_keys
            if launched_folders is not None:
                # '{run}' stands for the run's own directory, as a runner would declare it.
                launch['run_folders'] = {name: path.replace('{run}', str(run)) for name, path in launched_folders.items()}
            lab.write_json(run / 'launch.json', launch)
            lab.write_json(run / 'result.json', {'reason': 'time_limit', 'exit_code': -1})
            (run / 'stderr.log').write_text('')
            (run / 'memory.jsonl').write_text(json.dumps({'ram_mb': 1024, 'process_private_gib': 2}) + '\n')
            return {'status': 'passed', 'exit_code': 0}

        saved = lab.run_command, lab.gpu_lease
        lab.run_command, lab.gpu_lease = fake_run, lambda script, owner: contextlib.nullcontext()
        try:
            args = argparse.Namespace(manifest=str(manifest), case=None, repo=root)
            rows = lab.runtime_checks(args, root / 'out', 'synthetic-owner')
        finally:
            lab.run_command, lab.gpu_lease = saved
        return commands, rows

    def test_settings_reach_the_runner_in_one_argument_and_must_be_applied(self):
        commands, rows = self.runtime_case({'VICTORY_TEXTURE_NO_WAIT': 1, 'VICTORY_JOIN_PARTS': '1'},
                                           {'VICTORY_JOIN_PARTS': '1', 'VICTORY_TEXTURE_NO_WAIT': '1', 'VICTORY_FRAME_LOG': '1'})
        command = commands[0]
        self.assertEqual(command[command.index('-Settings') + 1], 'VICTORY_JOIN_PARTS=1;VICTORY_TEXTURE_NO_WAIT=1')
        self.assertEqual(rows[0]['requested_settings'], {'VICTORY_JOIN_PARTS': '1', 'VICTORY_TEXTURE_NO_WAIT': '1'})
        self.assertEqual(rows[0]['status'], 'inconclusive')
        # Asked for and not declared by the runner: failed, with the setting named.
        _, rows = self.runtime_case({'VICTORY_TEXTURE_NO_WAIT': '1'}, {'VICTORY_FRAME_LOG': '1'})
        self.assertEqual(rows[0]['status'], 'failed')
        self.assertIn('VICTORY_TEXTURE_NO_WAIT', rows[0]['reason'])
        _, rows = self.runtime_case({'VICTORY_TEXTURE_NO_WAIT': '1'}, {'VICTORY_TEXTURE_NO_WAIT': '0'})
        self.assertEqual(rows[0]['status'], 'failed')

    def test_a_case_without_settings_adds_no_argument(self):
        commands, rows = self.runtime_case(None, {'VICTORY_FRAME_LOG': '1'})
        self.assertNotIn('-Settings', commands[0])
        self.assertEqual(rows[0]['requested_settings'], {})
        self.assertEqual(rows[0]['status'], 'inconclusive')

    def test_repeated_case_names_are_refused_before_any_launch(self):
        binary = self.root / 'victory.exe'
        binary.write_bytes(b'synthetic')
        case = {'name': 'same', 'binary': str(binary), 'commit': 'synthetic',
                'sha256': hashlib.sha256(b'synthetic').hexdigest()}
        manifest = self.root / 'manifest.json'
        lab.write_json(manifest, {'runner': str(binary), 'lease_script': 'unused',
                                  'cases': [dict(case, settings={'VICTORY_A': '1'}), dict(case, settings={'VICTORY_A': '2'})]})
        launched = []
        saved = lab.run_command
        lab.run_command = lambda *a, **k: launched.append(a) or {'status': 'passed'}
        try:
            for selected in (None, 'same'):
                with self.assertRaisesRegex(ValueError, 'same'):
                    lab.runtime_checks(argparse.Namespace(manifest=str(manifest), case=selected, repo=self.root),
                                       self.root / 'out', 'synthetic-owner')
        finally:
            lab.run_command = saved
        self.assertEqual(launched, [])

    def lease_helper(self):
        """A claim/release helper that records the last action asked of it."""
        helper = self.root / 'lease.py'
        helper.write_text('import sys\nfrom pathlib import Path\n'
                          'Path(__file__).with_suffix(".state").write_text(sys.argv[1])\n')
        return helper

    def test_a_timeout_ends_the_whole_tree_and_the_lease_is_released(self):
        marker = self.root / 'grandchild-wrote.txt'
        grandchild = 'import time; from pathlib import Path; time.sleep(2); Path(' + repr(str(marker)) + ').write_text("late")'
        parent = 'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",' + repr(grandchild) + ']); time.sleep(30)'
        helper = self.lease_helper()
        with lab.gpu_lease(helper, 'synthetic-owner') as lease:
            row = lab.run_command([sys.executable, '-c', parent], self.root, self.root / 'tree', .8, grace=10)
            self.assertFalse(lab.left_running(row, lease))
        self.assertEqual(row['status'], 'timeout')
        self.assertTrue(row['tree_gone'])
        self.assertEqual(row['tree']['still_present'], [])
        if os.name == 'nt':
            self.assertTrue(row['tree']['job'])
            self.assertGreaterEqual(len(row['tree']['pids_seen']), 2, 'the grandchild was not seen in the job')
        self.assertEqual(helper.with_suffix('.state').read_text(), 'release')
        time.sleep(2.2)
        self.assertFalse(marker.exists(), 'the grandchild outlived the timeout')

    def test_processes_left_running_keep_the_lease_taken(self):
        helper = self.lease_helper()
        saved = lab.ProcessTree.present
        lab.ProcessTree.present = lambda tree: {424242: 'running'}  # as if a process of the tree would not go
        try:
            with self.assertRaises(lab.LeaseHeld) as held:
                with lab.gpu_lease(helper, 'synthetic-owner') as lease:
                    row = lab.run_command([sys.executable, '-c', 'pass'], self.root, self.root / 'stuck', 5, grace=.3)
                    self.assertFalse(row['tree_gone'])
                    self.assertGreaterEqual(row['tree']['waited_seconds'], .3)
                    self.assertTrue(lab.left_running(row, lease))
        finally:
            lab.ProcessTree.present = saved
        self.assertEqual(row['status'], 'error')
        self.assertEqual(helper.with_suffix('.state').read_text(), 'claim', 'the lease was released')
        message = str(held.exception)
        for part in ('synthetic-owner', '424242', 'running', 'release --owner synthetic-owner'):
            self.assertIn(part, message)
        self.assertEqual(row['tree']['why'], {'424242': 'running'})

    def test_an_ended_process_is_gone_only_once_its_memory_is(self):
        limit = lab.EXITED_MEMORY_LIMIT
        self.assertEqual(lab.still_there(False, 1 << 20), 'running')
        self.assertEqual(lab.still_there(True, 3 << 30), 'exited, memory still held: 3072 MB')
        self.assertEqual(lab.still_there(True, limit), f'exited, memory still held: {limit >> 20} MB')
        self.assertIsNone(lab.still_there(True, limit - 1), 'an ended process with little memory, held open by someone')
        self.assertEqual(lab.still_there(True, None), 'exited, memory unknown')

    @unittest.skipUnless(os.name == 'nt', 'ended processes are inspected on Windows')
    def test_an_ended_process_still_holding_gb_keeps_the_lease_taken(self):
        helper = self.lease_helper()
        real = lab.process_state
        lab.process_state = lambda pid: (lambda s: s and (s[0], True, 3 << 30))(real(pid))  # ended, 3 GB still held
        try:
            with self.assertRaises(lab.LeaseHeld) as held:
                with lab.gpu_lease(helper, 'synthetic-owner') as lease:
                    row = lab.run_command([sys.executable, '-c', 'pass'], self.root, self.root / 'held', 5, grace=.3)
                    self.assertTrue(lab.left_running(row, lease))
        finally:
            lab.process_state = real
        self.assertFalse(row['tree_gone'])
        self.assertEqual(set(row['tree']['why'].values()), {'exited, memory still held: 3072 MB'})
        self.assertEqual(helper.with_suffix('.state').read_text(), 'claim', 'the lease was released')
        self.assertIn('memory still held', str(held.exception))

    @unittest.skipUnless(os.name == 'nt', 'creation times identify processes on Windows')
    def test_a_reused_pid_is_not_one_of_ours(self):
        me = os.getpid()
        created = lab.process_created(me)
        self.assertIsNotNone(created)
        tree = lab.ProcessTree()
        tree.seen = {me: created + 1}  # the same PID, another process
        self.assertEqual(tree.present(), {})
        tree.seen = {me: created}
        self.assertEqual(tree.present(), {me: 'running'})
        ended = subprocess.Popen([sys.executable, '-c', 'pass'])
        ended.wait()  # ended, while Popen still holds it open, with its memory given back: gone
        created, finished, memory = lab.process_state(ended.pid)
        self.assertTrue(finished)
        self.assertLess(memory, lab.EXITED_MEMORY_LIMIT)
        tree.seen = {ended.pid: created}
        self.assertEqual(tree.present(), {})

    def test_a_normal_command_does_not_wait_for_nothing(self):
        row = lab.run_command([sys.executable, '-c', 'pass'], self.root, self.root / 'quick', 5, grace=60)
        self.assertEqual(row['status'], 'passed')
        self.assertTrue(row['tree_gone'])
        self.assertLess(row['tree']['waited_seconds'], 1)

    def test_a_runtime_timeout_is_never_passed_nor_inconclusive(self):
        binary = self.root / 'victory.exe'
        binary.write_bytes(b'synthetic')
        case = {'name': 'slow', 'binary': str(binary), 'commit': 'synthetic',
                'sha256': hashlib.sha256(b'synthetic').hexdigest(), 'max_seconds': 120}
        manifest = self.root / 'manifest.json'
        lab.write_json(manifest, {'runner': str(binary), 'lease_script': 'unused', 'cases': [case, dict(case, name='next')]})
        ran = []
        saved = lab.run_command, lab.gpu_lease
        lab.run_command = lambda command, *a, **k: ran.append(command) or {'status': 'timeout', 'exit_code': 1, 'tree_gone': True}
        lab.gpu_lease = lambda script, owner: contextlib.nullcontext()
        try:
            rows = lab.runtime_checks(argparse.Namespace(manifest=str(manifest), case=None, repo=self.root),
                                      self.root / 'out', 'synthetic-owner')
        finally:
            lab.run_command, lab.gpu_lease = saved
        self.assertEqual([row['status'] for row in rows], ['timeout'])
        self.assertEqual(len(ran), 1, 'a case ran after a timeout')

    def test_invalid_settings_stop_before_any_launch(self):
        with self.assertRaisesRegex(ValueError, 'VICTORY_STACKS'):
            self.runtime_case({'VICTORY_STACKS': '1'}, {})
        self.assertFalse(list(self.root.glob('*/out')), 'a case ran before its settings were checked')

    def test_keys_are_checked_as_the_layer_reads_them(self):
        good = ('150:Enter', '150:Enter,430:W~5', '430:W~5*3/12', '0:a', '1.5:Esc*2', '10:Tab*100/0.4',
                '300:L~0.2*2/0.4', '90:9~60')
        for keys in good:
            self.assertEqual(lab.case_keys({'name': 'c', 'keys': keys}, 3600), keys)
        self.assertIsNone(lab.case_keys({'name': 'c'}, 600))
        bad = ('', ' 150:Enter', '150:Enter, 430:W', '150 :Enter', 'Enter', '150:', ':Enter', '150:Space', '150:WW',
               '150:W~0.1', '150:W~61', '150:W~', '150:W*0', '150:W*101', '150:W*2/0.3', '150:W~5*2/5', '150:W~12*2',
               '150:W/3', '-1:W', '1e2:W', 'nan:W', '150:W;calc', '150:"W"', '150:Enter,,430:W', '1:W' + ',1:W' * 70,
               150, ['150:Enter'])
        for keys in bad:
            with self.assertRaises(ValueError, msg=repr(keys)):
                lab.case_keys({'name': 'c', 'keys': keys}, 3600)
        # A press that would end after the session is over was never pressed.
        self.assertEqual(lab.case_keys({'name': 'c', 'keys': '595:W~5'}, 600), '595:W~5')
        for keys in ('596:W~5', '600:Enter', '430:W~5*20/12'):
            with self.assertRaisesRegex(ValueError, 'ends after'):
                lab.case_keys({'name': 'c', 'keys': keys}, 600)

    def test_keys_reach_the_runner_and_must_be_applied(self):
        keys = '30:Enter,60:W~5*3/12'
        commands, rows = self.runtime_case(None, {}, keys=keys, launched_keys=keys)
        command = commands[0]
        self.assertEqual(command[command.index('-Keys') + 1], keys)
        self.assertEqual(rows[0]['requested_keys'], keys)
        self.assertEqual(rows[0]['runtime']['keys'], keys)
        self.assertEqual(rows[0]['status'], 'inconclusive')
        # Asked for and not declared by the runner, or declared otherwise: failed.
        for launched in (None, '30:Enter'):
            _, rows = self.runtime_case(None, {}, keys=keys, launched_keys=launched)
            self.assertEqual(rows[0]['status'], 'failed')
            self.assertIn('keys', rows[0]['reason'])
        # A case without keys adds no argument and asks for nothing.
        commands, rows = self.runtime_case(None, {})
        self.assertNotIn('-Keys', commands[0])
        self.assertIsNone(rows[0]['requested_keys'])

    def test_invalid_keys_stop_before_any_launch(self):
        with self.assertRaisesRegex(ValueError, 'ends after'):
            self.runtime_case(None, {}, keys='150:Enter')  # the case runs 120 seconds
        with self.assertRaisesRegex(ValueError, 'Unknown key'):
            self.runtime_case(None, {}, keys='30:Space')
        self.assertFalse(list(self.root.glob('*/out')), 'a case ran before its keys were checked')

    def test_run_folders_are_plain_names_for_switches_nothing_else_governs(self):
        self.assertEqual(lab.case_run_folders({'name': 'c', 'run_folders': {'VICTORY_SHOT': 'shot', 'VICTORY_A': 'a-1'}}, {}),
                         {'VICTORY_A': 'a-1', 'VICTORY_SHOT': 'shot'})
        self.assertEqual(lab.case_run_folders({'name': 'c'}, {}), {})
        for folder in ('', 'Shot', 'a/b', 'a\\b', '..', '.', 'C:shot', 'a b', '-a', 'x' * 33, 1, None):
            with self.assertRaisesRegex(ValueError, 'not a path', msg=repr(folder)):
                lab.case_run_folders({'name': 'c', 'run_folders': {'VICTORY_SHOT': folder}}, {})
        for name in ('victory_shot', 'OTHER', 'VICTORY_KEYS', 'VICTORY_PROFILE_OUT', 'VICTORY_LOG'):
            with self.assertRaises(ValueError, msg=name):
                lab.case_run_folders({'name': 'c', 'run_folders': {name: 'shot'}}, {})
        # Not both a setting and a run folder, and not one folder for two switches.
        with self.assertRaisesRegex(ValueError, 'governed elsewhere'):
            lab.case_run_folders({'name': 'c', 'run_folders': {'VICTORY_SHOT': 'shot'}}, {'VICTORY_SHOT': 'x'})
        with self.assertRaisesRegex(ValueError, 'share'):
            lab.case_run_folders({'name': 'c', 'run_folders': {'VICTORY_A': 'same', 'VICTORY_B': 'same'}}, {})
        with self.assertRaises(ValueError):
            lab.case_run_folders({'name': 'c', 'run_folders': ['VICTORY_SHOT=shot']}, {})

    def test_run_folders_reach_the_runner_and_must_be_inside_the_run(self):
        asked = {'VICTORY_SHOT': 'shot'}
        commands, rows = self.runtime_case(None, {}, folders=asked, launched_folders={'VICTORY_SHOT': '{run}/shot'})
        command = commands[0]
        self.assertEqual(command[command.index('-RunFolders') + 1], 'VICTORY_SHOT=shot')
        self.assertEqual(rows[0]['requested_run_folders'], asked)
        self.assertEqual(rows[0]['status'], 'inconclusive')
        # Not declared, another name, or a folder that is not the run's own: failed.
        for launched in (None, {}, {'VICTORY_SHOT': '{run}/other'}, {'VICTORY_SHOT': '{run}/deeper/shot'},
                         {'VICTORY_SHOT': str(self.root / 'shot')}, {'VICTORY_SHOT': 'shot'}):
            _, rows = self.runtime_case(None, {}, folders=asked, launched_folders=launched)
            self.assertEqual(rows[0]['status'], 'failed', msg=repr(launched))
            self.assertIn('VICTORY_SHOT', rows[0]['reason'])
        commands, rows = self.runtime_case(None, {})
        self.assertNotIn('-RunFolders', commands[0])
        self.assertEqual(rows[0]['requested_run_folders'], {})

    def test_invalid_run_folders_stop_before_any_launch(self):
        with self.assertRaisesRegex(ValueError, 'not a path'):
            self.runtime_case(None, {}, folders={'VICTORY_SHOT': 'C:/shots'})
        self.assertFalse(list(self.root.glob('*/out')), 'a case ran before its run folders were checked')


if __name__ == '__main__':
    unittest.main()
