import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class WindowSampleTest(unittest.TestCase):
    def test_filters_window_and_reports_batch_medians(self):
        script = Path(__file__).resolve().parents[1] / 'tools/summarize-window-sample.py'
        with tempfile.TemporaryDirectory() as folder:
            run = Path(folder)
            (run / 'launch.json').write_text(json.dumps({'started': '2026-11-26T18:00:00+01:00'}), encoding='utf8')
            def line(second, copied, same):
                return (f'2026-11-26T17:00:{second:02d}.000000Z INFO '
                        f'title memory windows with no card side took per frame: {copied} MB copied, 0.0 MB left out; '
                        f'of what they took, {same} MB held the bytes the window last copied for their 4 KiB page, '
                        '1.0 MB other bytes, 2.0 MB nothing to compare with; prints 0.0 MB '
                        '(VICTORY_WINDOW_SKIP_SAME=sample: copied again, one 4 KiB page in 16 compared)')
            (run / 'stderr.log').write_text('\n'.join([line(3, 99, 99), line(10, 10, 2), line(20, 30, 6), line(27, 99, 99)]), encoding='utf8')
            result = subprocess.run([sys.executable, str(script), str(run), '0', '30'], capture_output=True, text=True, check=True)
        data = json.loads(result.stdout)
        self.assertEqual(data['log_batches'], 2)
        self.assertEqual(data['median_mb_per_frame_by_batch']['copied'], 20)
        self.assertEqual(data['median_mb_per_frame_by_batch']['same_estimate'], 4)


if __name__ == '__main__':
    unittest.main()
