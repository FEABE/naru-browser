import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch


SPEC = importlib.util.spec_from_file_location(
    "naru_memory_report", Path(__file__).resolve().parents[1] / "scripts" / "measure-memory.py"
)
memory = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = memory
SPEC.loader.exec_module(memory)


def process(pid, ppid, rss, started="Fri Oct 9 01:02:03 2026", role="other"):
    return memory.Process(pid, ppid, rss, started, role)


class MemoryReportTests(unittest.TestCase):
    def collect(self, snapshots, **options):
        reader = Mock(side_effect=snapshots)
        return memory.collect(10, len(snapshots), 0.1, reader=reader,
                              sleeper=Mock(), clock=lambda: 0, **options)

    def test_ps_parser_classifies_helpers_without_retaining_commands(self):
        commands = [
            (10, 1, 2048, "/Applications/Light Browser.app/Contents/MacOS/light-browser --profile=/private/fake-profile"),
            (11, 10, 1024, "Helper --type=renderer https://example.test/private?token=fake-secret"),
            (12, 10, 512, "Helper --type=gpu-process"),
            (13, 10, 256, "Helper --type=utility --utility-sub-type=network.mojom.NetworkService"),
            (14, 10, 128, "Helper --type=utility --utility-sub-type=storage.mojom.StorageService"),
            (15, 10, 64, "Helper --type=utility --utility-sub-type=other.Service"),
        ]
        rows = "\n".join(f"{pid} {ppid} {rss} Fri Oct  9 01:02:03 2026 {command}"
                         for pid, ppid, rss, command in commands)
        parsed = memory.parse_ps(rows)
        self.assertEqual([p.role for p in parsed.values()], ["other", "renderer", "gpu", "network", "storage", "utility"])
        report = self.collect([parsed])
        self.assertEqual(report["samples"][0]["processes"][0]["role"], "browser")
        serialized = json.dumps(report)
        for secret in ("fake-profile", "fake-secret", "example.test", "Light Browser.app", "Fri Oct"):
            self.assertNotIn(secret, serialized)

    def test_descendants_exclude_other_instances_and_use_observed_total_peak(self):
        first = {p.pid: p for p in [process(10, 1, 2048), process(11, 10, 1024),
                                   process(12, 11, 512), process(20, 1, 900000), process(21, 20, 900000)]}
        second = {p.pid: p for p in [process(10, 1, 1024), process(11, 10, 4096), process(20, 1, 900000)]}
        report = self.collect([first, second])
        self.assertEqual([sample["rssSumMiB"] for sample in report["samples"]], [3.5, 5.0])
        self.assertEqual(report["peakObservedRssSumMiB"], 5.0)
        self.assertEqual([p["pid"] for p in report["samples"][0]["processes"]], [10, 11, 12])
        self.assertEqual(report["samples"][0]["processes"][2]["rssMiB"], 0.5)

    def test_exited_and_reused_main_pid_are_rejected(self):
        initial = {10: process(10, 1, 1024)}
        for later, code in [({}, "main_process_exited"),
                            ({10: process(10, 1, 1024, started="Fri Oct 9 01:02:04 2026")}, "main_pid_reused")]:
            with self.subTest(code=code), self.assertRaisesRegex(memory.MeasurementError, code):
                self.collect([initial, later])

    def test_malformed_ps_output_is_rejected_without_echoing_raw_data(self):
        for text in ("secret-content", "1 0 -1 Fri Oct 9 01:02:03 2026 secret-content"):
            with self.assertRaisesRegex(memory.MeasurementError, "^invalid_ps_output$"):
                memory.parse_ps(text)

    def test_footprint_units_do_not_parse_or_sum_per_process_peaks(self):
        for text, expected in [("512K", 0.5), ("92.3M", 92.3), ("1.5G", 1536)]:
            output = f"Physical footprint: {text}\nPhysical footprint (peak): 99G\n"
            self.assertEqual(memory.parse_footprint(output), expected)
        with self.assertRaisesRegex(memory.MeasurementError, "footprint_unavailable"):
            memory.parse_footprint("Physical footprint (peak): 99G")

    def test_failed_footprint_is_null_and_prevents_a_misleading_total(self):
        snapshot = {10: process(10, 1, 1024), 11: process(11, 10, 2048)}
        report = memory.collect(10, 1, 0.1, footprint=True, reader=lambda: snapshot,
                                clock=lambda: 0, sleeper=Mock(),
                                footprint_reader=Mock(side_effect=[0.5, memory.MeasurementError("footprint_timeout")]))
        rows = report["samples"][0]["processes"]
        self.assertEqual(rows[0]["physicalFootprintMiB"], 0.5)
        self.assertIsNone(rows[1]["physicalFootprintMiB"])
        self.assertEqual(rows[1]["footprintWarning"], "footprint_timeout")
        self.assertIsNone(report["footprintSumMiB"])
        self.assertEqual(report["peakObservedRssSumMiB"], 3.0)

    def test_child_pid_reuse_skips_vmmap_and_main_reuse_aborts(self):
        initial = {10: process(10, 1, 1024), 11: process(11, 10, 2048)}
        replaced_child = {10: initial[10], 11: process(11, 10, 4096, started="later")}
        vmmap = Mock(return_value=0.5)
        reader = Mock(side_effect=[initial, initial, initial, replaced_child, replaced_child])
        report = memory.collect(10, 1, 0.1, True, reader=reader, clock=lambda: 0, footprint_reader=vmmap)
        vmmap.assert_called_once_with(10, 8)
        self.assertEqual(report["samples"][0]["processes"][1]["footprintWarning"], "footprint_process_changed")
        with self.assertRaisesRegex(memory.MeasurementError, "main_pid_reused"):
            memory.collect(10, 1, 0.1, True, clock=lambda: 0,
                           reader=Mock(side_effect=[initial, {10: process(10, 1, 1024, started="later")}]),
                           footprint_reader=Mock())

    def test_expired_footprint_window_skips_remaining_processes(self):
        snapshot = {10: process(10, 1, 1024), 11: process(11, 10, 2048)}
        ticks = iter([0, 0, 0, 31, 31])
        vmmap = Mock()
        report = memory.collect(10, 1, 0.1, True, reader=lambda: snapshot,
                                clock=lambda: next(ticks), footprint_reader=vmmap)
        vmmap.assert_not_called()
        self.assertIsNone(report["footprintSumMiB"])
        self.assertTrue(all(row["footprintWarning"] == "footprint_budget_exhausted"
                            for row in report["samples"][0]["processes"]))

    def test_vmmap_warning_and_timeout_do_not_expose_stderr(self):
        result = subprocess.CompletedProcess([], 0, "Physical footprint: 92.3M", "private/path token=fake")
        with patch.object(memory.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(memory.MeasurementError, "^footprint_warning$"):
                memory.read_footprint(10, 8)
        partition = ("Can't examine target process's malloc zone PartitionAlloc_0x1, so memory analysis will be incomplete or incorrect.\n"
                     "Reason: failed to resolve canonical path for /x in dyld shared cache.\n\n")
        result = subprocess.CompletedProcess([], 0, "Physical footprint: 92.3M", partition)
        with patch.object(memory.subprocess, "run", return_value=result):
            self.assertEqual(memory.read_footprint(10, 8), 92.3)
        result = subprocess.CompletedProcess([], 0, "Physical footprint: 92.3M", partition + "other warning")
        with patch.object(memory.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(memory.MeasurementError, "^footprint_warning$"):
                memory.read_footprint(10, 8)
        with patch.object(memory.subprocess, "run", side_effect=subprocess.TimeoutExpired("vmmap", 8)):
            with self.assertRaisesRegex(memory.MeasurementError, "^footprint_timeout$"):
                memory.read_footprint(10, 8)

    def test_argument_bounds_and_existing_output_refusal(self):
        for kind, lower, upper in [(int, 1, 10), (float, 0.1, 5)]:
            parse = memory.bounded(kind, lower, upper)
            self.assertEqual(parse(str(lower)), lower)
            self.assertEqual(parse(str(upper)), upper)
            for value in ("0", "-1", "11", "nan", "inf", "text"):
                with self.assertRaises(argparse.ArgumentTypeError):
                    parse(value)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.json"
            output.write_text("preserved")
            with patch.object(memory.sys, "argv", ["measure-memory", "--pid", "10", "--output", str(output)]), \
                    patch.object(memory.sys, "platform", "darwin"), \
                    patch.object(memory, "collect") as collect, contextlib.redirect_stdout(io.StringIO()) as captured:
                self.assertEqual(memory.main(), 1)
                collect.assert_not_called()
                self.assertEqual(json.loads(captured.getvalue())["error"], "output_already_exists")
            self.assertEqual(output.read_text(), "preserved")


if __name__ == "__main__":
    unittest.main()
