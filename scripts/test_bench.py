"""Run with python -m unittest discover -s scripts -p test_bench.py."""

import os
import sys
import time
import tempfile
import unittest
from unittest.mock import patch

import bench


class BenchmarkTest(unittest.TestCase):
    def test_elapsed_per_draw_uses_unrounded_draw_totals(self):
        with tempfile.TemporaryDirectory(prefix="herdr-bench-test-") as root:
            with open(os.path.join(root, "frames.jsonl"), "w") as f:
                f.write('{"window":100,"frame_ms":800}\n')
                f.write('{"window":1,"frame_ms":0.02}\n')
            with patch.object(bench, "ROOT", root):
                result = bench.frames_since(1)
        self.assertEqual(result["window"], 1)
        self.assertEqual(result["render_to_paint_ms"], 0.02)

    @unittest.skipUnless(sys.platform == "win32", "native Windows process accounting")
    def test_native_cpu_and_memory_account_for_real_work(self):
        before, rss = bench.windows_process(os.getpid())
        until = time.monotonic() + 0.15
        while time.monotonic() < until:
            pass
        after, _ = bench.windows_process(os.getpid())
        self.assertGreater(after, before)
        self.assertGreater(rss, 0)
        self.assertEqual(bench.windows_process(0xFFFFFFFF), (None, None))


if __name__ == "__main__":
    unittest.main()
