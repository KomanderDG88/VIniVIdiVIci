import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('elapsed', Path(__file__).resolve().parents[1] / 'tools/movement-elapsed-window.py')
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)


class ElapsedWindowTest(unittest.TestCase):
    def test_uses_actual_event_time_and_timezone(self):
        with tempfile.TemporaryDirectory() as folder:
            run = Path(folder)
            (run / 'launch.json').write_text(json.dumps({'started': '2026-11-26T18:00:00+01:00'}), encoding='utf8')
            (run / 'stderr.log').write_text('2026-11-26T17:09:14.000000Z INFO scripted key down key=76 at=540\n', encoding='utf8')
            result = mod.window(run)
        self.assertEqual(result['start_seconds'], 554)
        self.assertEqual(result['end_seconds'], 834)

    def test_rejects_missing_event(self):
        with tempfile.TemporaryDirectory() as folder:
            run = Path(folder)
            (run / 'launch.json').write_text(json.dumps({'started': '2026-11-26T18:00:00+01:00'}), encoding='utf8')
            (run / 'stderr.log').write_text('', encoding='utf8')
            with self.assertRaises(ValueError):
                mod.window(run)


if __name__ == '__main__':
    unittest.main()
