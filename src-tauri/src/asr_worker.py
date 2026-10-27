"""Managed, local-only sherpa-onnx worker. Imported code never installs anything.

Models are k2-fsa's ONNX exports; no downloaded Python/model code is executed.
Only the explicit ``install`` action accesses the network.
"""
import argparse
import json
import math
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
import uuid
import venv
import wave


RUNTIME_VERSION = "2.0.0"
NUMPY_VERSION = "2.2.6"
SAMPLE_RATE = 16000
RELEASE = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
MODELS = {
    "sensevoice": "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
    "paraformer": "sherpa-onnx-paraformer-zh-2023-09-14",
}
MODEL_FILES = ("model.int8.onnx", "tokens.txt", "silero_vad.onnx")
MANIFEST = "umi-asr-model.json"
MAX_DOWNLOAD = 600 * 1024 * 1024


def emit(value):
    print(json.dumps(value, ensure_ascii=False, allow_nan=False), flush=True)


def extract_model_archive(archive, destination, expected_root, max_unpacked=2 * 1024**3):
    """Stream two allowlisted data files, rejecting unsafe members everywhere."""
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    found = set()
    total = 0
    with tarfile.open(archive, mode="r|bz2") as source:
        for index, member in enumerate(source):
            name = member.name
            parts = PurePosixPath(name).parts
            if (index > 10000 or "\\" in name or ":" in name or name.startswith("/")
                    or ".." in parts or not parts or parts[0] != expected_root):
                raise ValueError("模型压缩包包含不安全的路径")
            if not (member.isdir() or member.isfile()) or member.size < 0:
                raise ValueError("模型压缩包包含链接或特殊文件")
            total += member.size
            if total > max_unpacked:
                raise ValueError("模型压缩包超过解压大小限制")
            if not member.isfile() or len(parts) != 2 or parts[1] not in MODEL_FILES[:2]:
                continue
            basename = parts[1]
            if basename in found:
                raise ValueError("模型压缩包包含重复文件")
            limit = 550 * 1024**2 if basename.endswith(".onnx") else 5 * 1024**2
            if not 0 < member.size <= limit:
                raise ValueError("模型文件大小无效")
            data = source.extractfile(member)
            if data is None:
                raise ValueError("模型文件无法解压")
            with (destination / basename).open("xb") as output:
                shutil.copyfileobj(data, output, length=1024 * 1024)
            if (destination / basename).stat().st_size != member.size:
                raise ValueError("模型压缩包不完整")
            found.add(basename)
    if found != set(MODEL_FILES[:2]):
        raise ValueError("模型压缩包缺少权重或词表")


def model_ready(model_dir, engine):
    try:
        directory = Path(model_dir)
        manifest = json.loads((directory / MANIFEST).read_text(encoding="utf-8"))
        if manifest.get("source") != MODELS[engine] or manifest.get("engine") != engine:
            return False
        for name in MODEL_FILES:
            path = directory / name
            size = path.stat().st_size
            if not path.is_file() or size <= 0 or size != manifest["sizes"][name]:
                return False
        return True
    except (OSError, KeyError, TypeError, ValueError):
        return False


def pcm_blocks(path, block_samples=512):
    if block_samples <= 0:
        raise ValueError("音频块大小无效")
    with wave.open(str(path), "rb") as source:
        if (source.getnchannels() != 1 or source.getsampwidth() != 2
                or source.getframerate() != SAMPLE_RATE or source.getcomptype() != "NONE"):
            raise ValueError("本地识别需要 16 kHz 单声道 PCM16 WAV 音频")
        expected_bytes = source.getnframes() * 2
        read_bytes = 0
        while True:
            data = source.readframes(block_samples)
            if not data:
                break
            if len(data) % 2:
                raise ValueError("WAV 音频数据不完整")
            read_bytes += len(data)
            yield data
        if read_bytes != expected_bytes:
            raise ValueError("WAV 音频被截断")


def result_cues(result, start_sample, num_samples, total_samples):
    """Token starts and VAD speech end define times; never divide text evenly."""
    text = str(result.get("text", "")).strip()
    if not text:
        return []
    tokens = result.get("tokens", [])
    timestamps = result.get("timestamps", [])
    duration = num_samples / SAMPLE_RATE
    if (not tokens or len(tokens) != len(timestamps) or start_sample < 0
            or num_samples <= 0 or total_samples <= start_sample):
        raise ValueError("识别结果缺少有效的词级时间戳，无法生成字幕")
    previous = -1.0
    for stamp in timestamps:
        if (not isinstance(stamp, (int, float)) or not math.isfinite(stamp)
                or stamp < 0 or stamp < previous or stamp > duration + 0.05):
            raise ValueError("识别结果包含无效的时间戳")
        previous = stamp
    segment_end = min(start_sample + num_samples, total_samples) / SAMPLE_RATE
    durations = result.get("durations", [])
    if durations:
        if len(durations) != len(tokens) or any(
                not isinstance(d, (int, float)) or not math.isfinite(d) or d < 0
                for d in durations):
            raise ValueError("识别结果包含无效的词持续时间")
        if durations[-1] > 0:
            segment_end = min(segment_end,
                              start_sample / SAMPLE_RATE + timestamps[-1] + durations[-1])
    # Exact text is retained for BPE/ITN outputs whose tokens do not render alike.
    can_split = text == "".join(tokens).strip()
    groups = []
    first = 0
    if can_split:
        for i in range(len(tokens) - 1):
            boundary = (tokens[i].endswith(tuple("。！？!?；;")) or i - first >= 39
                        or timestamps[i + 1] - timestamps[first] >= 6)
            if boundary and timestamps[i + 1] > timestamps[first]:
                groups.append((first, i + 1, timestamps[i + 1]))
                first = i + 1
    groups.append((first, len(tokens), segment_end - start_sample / SAMPLE_RATE))
    cues = []
    for begin, end, end_time in groups:
        start_ms = round((start_sample / SAMPLE_RATE + timestamps[begin]) * 1000)
        end_ms = round((start_sample / SAMPLE_RATE + end_time) * 1000)
        content = "".join(tokens[begin:end]).strip() if can_split else text
        if content:
            if end_ms <= start_ms:
                raise ValueError("识别结果时间范围为空，无法生成字幕")
            cues.append({"start_ms": start_ms, "end_ms": end_ms, "text": content})
    return cues


class HttpsOnlyRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        if not newurl.startswith("https://"):
            raise ValueError("模型下载拒绝非 HTTPS 重定向")
        return super().redirect_request(request, fp, code, msg, headers, newurl)


def download_file(url, destination, max_bytes=MAX_DOWNLOAD):
    if not url.startswith(RELEASE):
        raise ValueError("只允许官方模型下载地址")
    destination = Path(destination)
    if destination.exists():
        raise FileExistsError("模型下载目标已存在")
    opener = urllib.request.build_opener(HttpsOnlyRedirect())
    deadline = time.monotonic() + 1200
    for attempt in range(3):
        # GitHub's signed release redirects can be cached past their expiry.
        fresh_url = url + "?umi_download=" + uuid.uuid4().hex
        request = urllib.request.Request(fresh_url, headers={
            "User-Agent": "Umidl-local-ASR/1", "Cache-Control": "no-cache"})
        partial = destination.with_name(destination.name + "." + uuid.uuid4().hex + ".part")
        try:
            with opener.open(request, timeout=60) as response, partial.open("xb") as output:
                declared = int(response.headers.get("Content-Length", 0))
                if declared > max_bytes:
                    raise ValueError("模型下载超过大小限制")
                received = 0
                while True:
                    if time.monotonic() > deadline:
                        raise TimeoutError("模型下载超时，请检查网络后重试")
                    data = response.read(1024 * 1024)
                    if not data:
                        break
                    received += len(data)
                    if received > max_bytes:
                        raise ValueError("模型下载超过大小限制")
                    output.write(data)
                if received == 0 or (declared and received != declared):
                    raise ValueError("模型下载不完整")
            partial.rename(destination)
            return
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            if attempt == 2 or time.monotonic() > deadline:
                raise
        finally:
            partial.unlink(missing_ok=True)


def runtime_python(runtime_dir):
    return Path(runtime_dir) / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def install_runtime(runtime_dir):
    if not (3, 10) <= sys.version_info[:2] <= (3, 13):
        raise ValueError("请先安装 64 位 Python 3.10–3.13，再安装本地识别引擎")
    runtime_dir = Path(runtime_dir)
    python = runtime_python(runtime_dir)
    if not python.is_file():
        venv.EnvBuilder(with_pip=True).create(runtime_dir)
    # An interrupted venv creation can leave Python present before pip is installed.
    # Bootstrap from Python's bundled wheels, only inside this app-owned runtime.
    subprocess.run([str(python), "-I", "-m", "ensurepip", "--upgrade"],
                   check=True, stdout=sys.stderr, timeout=120)
    # -I / --isolated exclude user site packages and global pip configuration.
    command = [str(python), "-I", "-m", "pip", "--isolated", "--disable-pip-version-check",
               "install", "--no-cache-dir", "--only-binary=:all:",
               "--index-url", "https://pypi.org/simple",
               f"sherpa-onnx=={RUNTIME_VERSION}", f"numpy=={NUMPY_VERSION}"]
    subprocess.run(command, check=True, stdout=sys.stderr, timeout=1800)
    subprocess.run([str(python), "-I", "-c",
                    "import sherpa_onnx,numpy; from importlib.metadata import version; "
                    f"assert version('sherpa-onnx') == '{RUNTIME_VERSION}'"],
                   check=True, stdout=sys.stderr, timeout=60)
    return python


def create_recognizer(model_dir, engine, language="auto"):
    import sherpa_onnx
    model_dir = Path(model_dir)
    common = {"tokens": str(model_dir / "tokens.txt"), "num_threads": 2,
              "provider": "cpu", "debug": False}
    if engine == "sensevoice":
        if language not in ("auto", "zh", "en", "ja", "ko", "yue"):
            raise ValueError("SenseVoice 支持自动、中文、英文、日文、韩文和粤语识别")
        return sherpa_onnx.OfflineRecognizer.from_sense_voice(
            model=str(model_dir / "model.int8.onnx"), language=language, use_itn=True, **common)
    if engine == "paraformer":
        if language not in ("auto", "zh", "en"):
            raise ValueError("Paraformer 支持中文和英文识别")
        return sherpa_onnx.OfflineRecognizer.from_paraformer(
            paraformer=str(model_dir / "model.int8.onnx"), **common)
    raise ValueError("未知的本地识别引擎")


def create_vad(model_dir):
    import sherpa_onnx
    config = sherpa_onnx.VadModelConfig()
    config.silero_vad.model = str(Path(model_dir) / "silero_vad.onnx")
    config.silero_vad.threshold = 0.5
    config.silero_vad.min_silence_duration = 0.35
    config.silero_vad.min_speech_duration = 0.15
    config.silero_vad.max_speech_duration = 8
    config.sample_rate = SAMPLE_RATE
    config.num_threads = 1
    config.provider = "cpu"
    return sherpa_onnx.VoiceActivityDetector(config, buffer_size_in_seconds=35)


def install_model(model_dir, engine):
    if model_ready(model_dir, engine):
        return
    model_dir = Path(model_dir)
    model_dir.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".asr-install-", dir=model_dir.parent) as temp:
        stage = Path(temp)
        archive = stage / "model.tar.bz2"
        download_file(RELEASE + MODELS[engine] + ".tar.bz2", archive)
        extract_model_archive(archive, stage / "model", MODELS[engine])
        download_file(RELEASE + "silero_vad.onnx", stage / "model/silero_vad.onnx", 10 * 1024**2)
        # Load both models before publishing readiness, so truncated/invalid ONNX fails setup.
        create_recognizer(stage / "model", engine)
        create_vad(stage / "model")
        sizes = {name: (stage / "model" / name).stat().st_size for name in MODEL_FILES}
        manifest = {"engine": engine, "source": MODELS[engine], "sizes": sizes}
        (stage / "model" / MANIFEST).write_text(json.dumps(manifest), encoding="utf-8")
        if model_dir.exists():
            # Keep an incomplete prior install recoverable until publication succeeds.
            previous = stage / "previous"
            model_dir.rename(previous)
            try:
                (stage / "model").rename(model_dir)
            except BaseException:
                previous.rename(model_dir)
                raise
        else:
            (stage / "model").rename(model_dir)


def recognize_segment(recognizer, samples, start_sample, total_samples):
    stream = recognizer.create_stream()
    stream.accept_waveform(SAMPLE_RATE, samples)
    recognizer.decode_stream(stream)
    result = json.loads(str(stream.result))
    return result_cues(result, start_sample, len(samples), total_samples)


def recognize(wav, model_dir, engine, language):
    import numpy as np
    if not model_ready(model_dir, engine):
        raise ValueError("请先安装所选本地识别引擎")
    with wave.open(str(wav), "rb") as source:
        total_samples = source.getnframes()
    recognizer = create_recognizer(model_dir, engine, language)
    vad = create_vad(model_dir)
    # VAD is reset every 30 seconds as a hard memory/recognizer input bound.
    # These are real sample boundaries; token times are offset into the original file.
    offset = 0
    consumed = 0
    chunk_samples = SAMPLE_RATE * 30
    def drain():
        while not vad.empty():
            segment = vad.front
            samples = segment.samples
            start = offset + segment.start
            for cue in recognize_segment(recognizer, samples, start, total_samples):
                emit({"cue": cue})
            vad.pop()
    for data in pcm_blocks(wav):
        samples = np.frombuffer(data, dtype="<i2").astype(np.float32) / 32768
        if len(samples) < 512:
            samples = np.pad(samples, (0, 512 - len(samples)))
        vad.accept_waveform(samples)
        consumed += len(data) // 2
        drain()
        if consumed >= chunk_samples:
            vad.flush()
            drain()
            offset += consumed
            consumed = 0
            vad = create_vad(model_dir)
    vad.flush()
    drain()
    emit({"done": True})


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("install", "install-model", "transcribe"))
    parser.add_argument("--engine", choices=tuple(MODELS), required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--runtime-dir", type=Path)
    parser.add_argument("--wav", type=Path)
    parser.add_argument("--language", default="auto")
    args = parser.parse_args()
    if args.action == "install":
        if args.runtime_dir is None:
            parser.error("--runtime-dir is required")
        python = install_runtime(args.runtime_dir)
        subprocess.run([str(python), "-I", str(Path(__file__).resolve()), "install-model",
                        "--engine", args.engine, "--model-dir", str(args.model_dir)],
                       check=True, timeout=1200)
    elif args.action == "install-model":
        install_model(args.model_dir, args.engine)
        emit({"installed": True})
    else:
        if args.wav is None:
            parser.error("--wav is required")
        recognize(args.wav, args.model_dir, args.engine, args.language)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"本地识别失败：{error}", file=sys.stderr, flush=True)
        sys.exit(1)
