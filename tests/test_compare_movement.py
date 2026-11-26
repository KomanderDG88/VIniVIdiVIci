"""Regression checks for the movement comparator using synthetic logs only."""
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('compare_movement', Path(__file__).resolve().parents[1] / 'tools/compare-movement.py')
compare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare)

class CompareMovementTest(unittest.TestCase):
    def test_window_boundaries_inputs_and_memory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            log = root / 'stderr.log'
            log.write_text('\n'.join([
                '2026-01-01T00:00:00Z INFO scripted key down key=76 at=540.0',
                '2026-01-01T00:00:02Z INFO frame times: 2 frames in 1.0 s: frame each: 100 200',
                '2026-01-01T00:00:03Z INFO scripted key let go after its hold key=76 at=540.0 reads=3',
                '2026-01-01T00:00:10Z INFO scripted key let go after its hold key=87 at=546.0 reads=0',
                '2026-01-01T00:04:41Z INFO frame times: 1 frames in 2.0 s: frame each: 9999',
            ]), encoding='utf8')
            (root / 'memory.jsonl').write_text(json.dumps({'ram_mb': 1024, 'process_private_gib': 4.5}) + '\n', encoding='utf8')
            (root / 'result.json').write_text(json.dumps({'reason': 'memory_guard'}), encoding='utf8')
            out = compare.measure(log)
            self.assertEqual(out['frames'], 2)
            self.assertEqual(out['fps'], 2.0)
            self.assertEqual(out['median_ms'], 150)
            self.assertEqual(out['inputs']['76']['missed'], 0)
            self.assertEqual(out['inputs']['87']['missed'], 1)
            self.assertEqual(out['run_end_reason'], 'memory_guard')
            self.assertEqual(out['min_free_ram_mb'], 1024)
            self.assertEqual(out['max_private_gib'], 4.5)

if __name__ == '__main__':
    unittest.main()
