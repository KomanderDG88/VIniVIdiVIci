import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('cpu_summary', Path(__file__).resolve().parents[1] / 'tools/summarize-cpu-processes.py')
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)


class CpuSummaryTest(unittest.TestCase):
    def test_enclosed_intervals_weighting_and_censored_names(self):
        rows = [
            {'seconds': 10, 'interval_seconds': 10, 'top': [{'name': 'a', 'logical_cores': 10}]},
            {'seconds': 20, 'interval_seconds': 10, 'top': [{'name': 'a', 'logical_cores': 2}]},
            {'seconds': 40, 'interval_seconds': 20, 'top': [{'name': 'b', 'logical_cores': 3}]},
            {'seconds': 45, 'interval_seconds': 5, 'top': [{'name': 'a', 'logical_cores': 10}]},
        ]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'cpu.jsonl'
            path.write_text('\n'.join(json.dumps(row) for row in rows), encoding='utf8')
            result = mod.summarize(path, 10, 40)
        values = {r['name']: r for r in result['processes']}
        self.assertEqual(result['samples'], 2)
        self.assertEqual(result['sampled_seconds'], 30)
        self.assertEqual(values['a']['mean_logical_cores_lower_bound'], 0.667)
        self.assertEqual(values['b']['mean_logical_cores_lower_bound'], 2.0)


if __name__ == '__main__':
    unittest.main()
