"""Offline safety/timestamp tests: python scripts/test_asr_worker.py."""
import importlib.util
import io
import math
from pathlib import Path
import tarfile
import tempfile
import types
import unittest
import subprocess
from unittest.mock import patch
import urllib.error
import wave


SOURCE = Path(__file__).resolve().parents[1] / "src-tauri/src/asr_worker.py"
if SOURCE.exists():
    spec = importlib.util.spec_from_file_location("asr_worker", SOURCE)
    worker = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(worker)
else:
    worker = types.SimpleNamespace()


def archive(path, files):
    with tarfile.open(path, "w:bz2") as out:
        for name, value in files:
            member = tarfile.TarInfo(name)
            if isinstance(value, bytes):
                member.size = len(value)
                out.addfile(member, io.BytesIO(value))
            else:
                member.type = tarfile.SYMTYPE
                member.linkname = value
                out.addfile(member)


class WorkerTests(unittest.TestCase):
    def api(self, name):
        fn = getattr(worker, name, None)
        self.assertTrue(callable(fn), f"missing local ASR behavior: {name}")
        return fn

    def test_only_required_model_data_is_extracted(self):
        extract = self.api("extract_model_archive")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "model.tar.bz2"
            archive(source, [("official/model.int8.onnx", b"model"),
                             ("official/tokens.txt", b"tokens"),
                             ("official/arbitrary.py", b"raise RuntimeError()")])
            extract(source, root / "out", "official")
            self.assertEqual((root / "out/model.int8.onnx").read_bytes(), b"model")
            self.assertEqual((root / "out/tokens.txt").read_bytes(), b"tokens")
            self.assertFalse((root / "out/arbitrary.py").exists())

    def test_traversal_and_links_are_rejected(self):
        extract = self.api("extract_model_archive")
        for name, data in [("../outside", b"x"), ("official/../../outside", b"x"),
                           ("C:/outside", b"x"), ("official\\..\\outside", b"x"),
                           ("official/model.int8.onnx", "../../outside")]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                source = root / "model.tar.bz2"
                archive(source, [(name, data)])
                with self.assertRaises(ValueError):
                    extract(source, root / "out", "official")
                self.assertFalse((root / "outside").exists())

    def test_archive_missing_weights_and_size_bombs_fail(self):
        extract = self.api("extract_model_archive")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "model.tar.bz2"
            archive(source, [("official/tokens.txt", b"tokens")])
            with self.assertRaises(ValueError):
                extract(source, root / "out", "official")
            archive(source, [("official/model.int8.onnx", b"12345"),
                             ("official/tokens.txt", b"tokens")])
            with self.assertRaises(ValueError):
                extract(source, root / "out", "official", max_unpacked=4)

    def test_token_time_offset_is_real_and_final_end_uses_vad(self):
        cues = self.api("result_cues")(
            {"text": "你好。再见", "tokens": ["你", "好", "。", "再", "见"],
             "timestamps": [0.2, 0.6, 0.9, 1.4, 1.9]},
            start_sample=160000, num_samples=40000, total_samples=240000)
        self.assertEqual(cues, [
            {"start_ms": 10200, "end_ms": 11400, "text": "你好。"},
            {"start_ms": 11400, "end_ms": 12500, "text": "再见"}])

    def test_missing_or_invalid_recognizer_timestamps_are_rejected(self):
        convert = self.api("result_cues")
        for stamps in [[], [0.2], [0.6, 0.2], [-1, 0.2], [0.2, math.nan], [0.2, 20]]:
            with self.subTest(stamps=stamps), self.assertRaises(ValueError):
                convert({"text": "你好", "tokens": ["你", "好"], "timestamps": stamps},
                        start_sample=0, num_samples=32000, total_samples=32000)

    def test_silent_results_do_not_create_subtitles(self):
        convert = self.api("result_cues")
        self.assertEqual(convert({"text": "", "tokens": [], "timestamps": []},
                                 start_sample=0, num_samples=32000, total_samples=32000), [])

    def test_english_and_itn_text_preserves_recognizer_spacing(self):
        cues = self.api("result_cues")(
            {"text": "Hello world", "tokens": ["Hello", "world"],
             "timestamps": [0.2, 0.8], "durations": [0.3, 0.4]},
            start_sample=0, num_samples=32000, total_samples=32000)
        self.assertEqual(cues, [{"start_ms": 200, "end_ms": 1200, "text": "Hello world"}])

    def test_native_result_string_contract_retains_sample_offset(self):
        # sherpa's C++ result exposes JSON through __str__, not as_json_string().
        class NativeResult:
            def __str__(self):
                return '{"text":"你好","tokens":["你","好"],"timestamps":[0.2,0.5]}'
        class Stream:
            result = NativeResult()
            def accept_waveform(self, rate, samples):
                if rate != 16000 or len(samples) != 16000:
                    raise ValueError("incorrect upstream audio contract")
        class Recognizer:
            def create_stream(self):
                return Stream()
            def decode_stream(self, stream):
                pass
        cues = self.api("recognize_segment")(Recognizer(), [0.0] * 16000, 32000, 64000)
        self.assertEqual(cues, [{"start_ms": 2200, "end_ms": 3000, "text": "你好"}])

    def test_pcm_reader_bounds_reads_and_rejects_wrong_audio(self):
        read = self.api("pcm_blocks")
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "test.wav"
            with wave.open(str(source), "wb") as out:
                out.setnchannels(1)
                out.setsampwidth(2)
                out.setframerate(16000)
                out.writeframes(b"\x01\x00" * 16003)
            blocks = list(read(source, block_samples=512))
            self.assertEqual(sum(len(block) // 2 for block in blocks), 16003)
            self.assertLessEqual(max(map(len, blocks)), 1024)
            with wave.open(str(source), "wb") as out:
                out.setnchannels(2)
                out.setsampwidth(2)
                out.setframerate(16000)
                out.writeframes(b"\0" * 16)
            with self.assertRaises(ValueError):
                list(read(source))

    def test_model_readiness_requires_manifest_and_complete_files(self):
        ready = self.api("model_ready")
        with tempfile.TemporaryDirectory() as temp:
            self.assertFalse(ready(Path(temp), "sensevoice"))
            (Path(temp) / "model.int8.onnx").write_bytes(b"fake")
            (Path(temp) / "tokens.txt").write_text("a 0\n")
            (Path(temp) / "silero_vad.onnx").write_bytes(b"fake")
            self.assertFalse(ready(Path(temp), "sensevoice"))

    def test_expired_official_redirect_retries_fresh_url_without_partial_publication(self):
        download = self.api("download_file")
        urls = []
        class Opener:
            def open(self, request, timeout):
                urls.append(request.full_url)
                if len(urls) == 1:
                    raise urllib.error.HTTPError(request.full_url, 403, "jwt expired", None, None)
                response = io.BytesIO(b"data")
                response.headers = {"Content-Length": "4"}
                return response
        with tempfile.TemporaryDirectory(dir=SOURCE.parent) as temp:
            path = Path(temp) / "model.onnx"
            with patch.object(worker.urllib.request, "build_opener", return_value=Opener()):
                download(worker.RELEASE + "silero_vad.onnx", path, 8)
            self.assertEqual(path.read_bytes(), b"data")
            self.assertEqual(len(urls), 2)
            self.assertNotEqual(urls[0], urls[1])

    def test_install_repairs_private_runtime_interrupted_before_pip_setup(self):
        install = self.api("install_runtime")
        actual_run = subprocess.run
        with tempfile.TemporaryDirectory(dir=SOURCE.parent) as temp:
            runtime = Path(temp) / "runtime"
            worker.venv.EnvBuilder(with_pip=False).create(runtime)
            python = worker.runtime_python(runtime)
            self.assertNotEqual(actual_run([str(python), "-I", "-m", "pip", "--version"],
                                          capture_output=True).returncode, 0)

            def offline_run(command, **kwargs):
                # Keep real local bootstrapping; omit network downloads and native imports.
                if "install" in command or "-c" in command:
                    return subprocess.CompletedProcess(command, 0)
                return actual_run(command, **kwargs)

            with patch.object(worker.subprocess, "run", side_effect=offline_run):
                self.assertEqual(install(runtime), python)
            result = actual_run([str(python), "-I", "-m", "pip", "--version"], capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))


if __name__ == "__main__":
    unittest.main()
