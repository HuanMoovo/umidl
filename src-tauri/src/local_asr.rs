//! Private Python/sherpa runtime and official local Chinese ONNX models.
//! Status and transcription never download; setup is an explicit user action.
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ctx::{command_for, kill_tree, Ctx};
use crate::subtitle::Cue;

const WORKER: &str = include_str!("asr_worker.py");
const RUNTIME_VERSION: &str = "2.0.0";
const MAX_OUTPUT: u64 = 64 * 1024 * 1024;

fn model_source(engine: &str) -> Result<&'static str> {
    match engine {
        "sensevoice" => Ok("sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"),
        "paraformer" => Ok("sherpa-onnx-paraformer-zh-2023-09-14"),
        _ => bail!("未知的本地识别引擎"),
    }
}

fn model_dir(data: &Path, engine: &str) -> Result<PathBuf> {
    model_source(engine)?;
    Ok(data.join("asr-models").join(engine))
}

fn runtime_python(data: &Path) -> PathBuf {
    data.join("asr-runtime").join(if cfg!(windows) { "Scripts/python.exe" } else { "bin/python" })
}

fn model_ready(directory: &Path, engine: &str) -> bool {
    let Ok(source) = model_source(engine) else { return false };
    let Ok(file) = File::open(directory.join("umi-asr-model.json")) else { return false };
    let Ok(manifest) = serde_json::from_reader::<_, Value>(file.take(64 * 1024)) else { return false };
    if manifest["engine"] != engine || manifest["source"] != source {
        return false;
    }
    ["model.int8.onnx", "tokens.txt", "silero_vad.onnx"].iter().all(|name| {
        let Ok(metadata) = fs::metadata(directory.join(name)) else { return false };
        metadata.is_file() && metadata.len() > 0
            && manifest["sizes"][*name].as_u64() == Some(metadata.len())
    })
}

struct ManagedChild(Child);

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            kill_tree(self.0.id());
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn runtime_ready(data: &Path) -> bool {
    let python = runtime_python(data);
    if !python.is_file() {
        return false;
    }
    let mut command = command_for(&python);
    command.args(["-I", "-c", &format!(
        "import sherpa_onnx,numpy; from importlib.metadata import version; assert version('sherpa-onnx') == '{RUNTIME_VERSION}'"
    )]).stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
    let Ok(child) = command.spawn() else { return false };
    let mut child = ManagedChild(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.0.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => return false,
        }
    }
}

fn status_at(data: &Path, engine: &str) -> Value {
    let directory = model_dir(data, engine).ok();
    let runtime = directory.is_some() && runtime_ready(data);
    let model = directory.as_ref().map(|path| model_ready(path, engine)).unwrap_or(false);
    let message = if directory.is_none() {
        "未知的本地识别引擎"
    } else if runtime && model {
        "已就绪，音频仅在本机处理"
    } else if !runtime {
        "请安装本地识别运行环境和模型（需要 64 位 Python 3.10–3.13）"
    } else {
        "请安装所选引擎的官方模型"
    };
    json!({
        "engine": engine, "runtime_ready": runtime, "model_ready": model,
        "ready": runtime && model, "message": message,
        "model_dir": directory.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default(),
    })
}

pub fn engine_status(ctx: &Ctx, engine: &str) -> Value {
    status_at(&ctx.dirs.data, engine)
}

fn worker_path(ctx: &Ctx) -> Result<PathBuf> {
    let digest = format!("{:x}", Sha256::digest(WORKER.as_bytes()));
    let directory = ctx.dirs.cache.join("local-asr");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("asr-worker-{}.py", &digest[..16]));
    if path.is_file() {
        if fs::read(&path)? != WORKER.as_bytes() {
            bail!("本地识别脚本缓存校验失败");
        }
        return Ok(path);
    }
    let temporary = directory.join(format!(".worker-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, WORKER)?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        if fs::read(&path).ok().as_deref() != Some(WORKER.as_bytes()) {
            return Err(error.into());
        }
    }
    Ok(path)
}

struct CapturedFiles {
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Drop for CapturedFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.stdout);
        let _ = fs::remove_file(&self.stderr);
    }
}

fn read_tail(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(16 * 1024)))?;
    let mut bytes = Vec::new();
    file.take(16 * 1024).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

fn run_captured(
    ctx: &Ctx,
    command: &mut Command,
    timeout: Duration,
    register_pid: &dyn Fn(u32),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>> {
    if is_cancelled() { bail!("任务已取消"); }
    let directory = ctx.dirs.cache.join("local-asr");
    fs::create_dir_all(&directory)?;
    let id = uuid::Uuid::new_v4();
    let files = CapturedFiles {
        stdout: directory.join(format!("{id}.out")),
        stderr: directory.join(format!("{id}.err")),
    };
    command.stdin(Stdio::null())
        .stdout(Stdio::from(File::create_new(&files.stdout)?))
        .stderr(Stdio::from(File::create_new(&files.stderr)?));
    let mut child = ManagedChild(command.spawn().context("无法启动本地识别进程")?);
    register_pid(child.0.id());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if is_cancelled() { bail!("任务已取消"); }
        if Instant::now() >= deadline { bail!("本地识别进程超时，请检查网络或重试"); }
        if fs::metadata(&files.stdout)?.len() > MAX_OUTPUT
            || fs::metadata(&files.stderr)?.len() > MAX_OUTPUT {
            bail!("本地识别输出超过大小限制");
        }
        if let Some(status) = child.0.try_wait()? { break status; }
        std::thread::sleep(Duration::from_millis(100));
    };
    if is_cancelled() { bail!("任务已取消"); }
    if !status.success() {
        let details = read_tail(&files.stderr).unwrap_or_default();
        bail!("本地识别进程失败：{}", if details.is_empty() { status.to_string() } else { details });
    }
    let mut output = Vec::new();
    File::open(&files.stdout)?.take(MAX_OUTPUT + 1).read_to_end(&mut output)?;
    if output.len() as u64 > MAX_OUTPUT { bail!("本地识别输出超过大小限制"); }
    Ok(output)
}

fn bootstrap_python(ctx: &Ctx) -> Result<PathBuf> {
    let probe = "import sys,struct; assert (3,10)<=sys.version_info[:2]<=(3,13) and struct.calcsize('P')==8; print(sys.executable)";
    let candidates: &[(&str, &[&str])] = if cfg!(windows) {
        &[("py", &["-3.10"]), ("py", &["-3.11"]), ("py", &["-3.12"]),
          ("py", &["-3.13"]), ("python", &[])]
    } else {
        &[("python3", &[]), ("python", &[])]
    };
    for (name, prefix) in candidates {
        let mut command = command_for(Path::new(name));
        command.args(*prefix).args(["-I", "-c", probe]);
        if let Ok(output) = run_captured(ctx, &mut command, Duration::from_secs(15), &|_| {}, &|| false) {
            let executable = String::from_utf8_lossy(&output).trim().to_string();
            let path = PathBuf::from(executable);
            if path.is_file() { return Ok(path); }
        }
    }
    bail!("请先安装 64 位 Python 3.10–3.13，再安装本地识别引擎")
}

pub fn install_engine(ctx: &Ctx, engine: &str) -> Result<()> {
    static INSTALL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    model_source(engine)?;
    let _guard = INSTALL_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner());
    let model = model_dir(&ctx.dirs.data, engine)?;
    let ready = runtime_ready(&ctx.dirs.data);
    if ready && model_ready(&model, engine) { return Ok(()); }
    let worker = worker_path(ctx)?;
    let python = if ready { runtime_python(&ctx.dirs.data) } else { bootstrap_python(ctx)? };
    let mut command = command_for(&python);
    command.arg("-I").arg(&worker)
        .arg(if ready { "install-model" } else { "install" })
        .args(["--engine", engine]).arg("--model-dir").arg(&model);
    if !ready {
        command.arg("--runtime-dir").arg(ctx.dirs.data.join("asr-runtime"));
    }
    run_captured(ctx, &mut command, Duration::from_secs(2400), &|_| {}, &|| false)?;
    if !runtime_ready(&ctx.dirs.data) || !model_ready(&model, engine) {
        bail!("本地识别引擎安装未完成，请重试");
    }
    Ok(())
}

fn parse_cue(value: &Value, index: usize) -> Result<Cue> {
    let start_ms = value["start_ms"].as_u64().ok_or_else(|| anyhow!("识别结果缺少有效的开始时间"))?;
    let end_ms = value["end_ms"].as_u64().ok_or_else(|| anyhow!("识别结果缺少有效的结束时间"))?;
    let text = value["text"].as_str().unwrap_or_default().trim();
    if end_ms <= start_ms || text.is_empty() { bail!("识别结果的时间范围或文本无效"); }
    Ok(Cue { index, start_ms, end_ms, text: text.to_string() })
}

pub fn transcribe(
    ctx: &Ctx,
    wav: &Path,
    engine: &str,
    language: &str,
    register_pid: &dyn Fn(u32),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Cue>> {
    if is_cancelled() { bail!("任务已取消"); }
    let model = model_dir(&ctx.dirs.data, engine)?;
    let status = engine_status(ctx, engine);
    if status["ready"] != true {
        bail!("{}", status["message"].as_str().unwrap_or("请先安装所选本地识别引擎"));
    }
    if !wav.is_file() { bail!("未找到待识别的音频文件"); }
    let worker = worker_path(ctx)?;
    let mut command = command_for(&runtime_python(&ctx.dirs.data));
    command.arg("-I").arg(worker).arg("transcribe").args(["--engine", engine])
        .arg("--model-dir").arg(model).arg("--wav").arg(wav)
        .args(["--language", if language.is_empty() { "auto" } else { language }]);
    let output = run_captured(ctx, &mut command, Duration::from_secs(24 * 3600), register_pid, is_cancelled)?;
    let mut cues = Vec::new();
    let mut completed = false;
    for line in output.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        if is_cancelled() { bail!("任务已取消"); }
        let value: Value = serde_json::from_slice(line).context("本地识别返回了无效的数据")?;
        if let Some(value) = value.get("cue") {
            let cue = parse_cue(value, cues.len() + 1)?;
            if cues.last().map(|last: &Cue| cue.start_ms < last.start_ms).unwrap_or(false) {
                bail!("本地识别时间戳顺序无效");
            }
            cues.push(cue);
        } else if value["done"] == true {
            completed = true;
        } else {
            bail!("本地识别返回了未知的数据");
        }
    }
    if !completed { bail!("本地识别未完整结束"); }
    if cues.is_empty() { bail!("没有识别到可生成字幕的语音"); }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unknown_engine_status_does_not_create_directories() {
        let root = std::env::temp_dir().join(format!("umi-asr-{}", uuid::Uuid::new_v4()));
        let status = status_at(&root, "../unexpected");
        assert_eq!(status["ready"], false);
        assert_eq!(status["runtime_ready"], false);
        assert_eq!(status["model_ready"], false);
        assert_eq!(status.as_object().unwrap().len(), 6);
        assert!(!root.exists());
    }

    #[test]
    fn cues_require_real_valid_timestamps_and_nonempty_text() {
        let valid = serde_json::json!({"start_ms":10200,"end_ms":12500,"text":" 你好 "});
        let cue = parse_cue(&valid, 2).unwrap();
        assert_eq!(cue.index, 2);
        assert_eq!(cue.start_ms, 10200);
        assert_eq!(cue.end_ms, 12500);
        assert_eq!(cue.text, "你好");
        for invalid in [
            serde_json::json!({"text":"missing timestamps"}),
            serde_json::json!({"start_ms":200,"end_ms":100,"text":"bad"}),
            serde_json::json!({"start_ms":100,"end_ms":200,"text":" "}),
            serde_json::json!({"start_ms":-1,"end_ms":200,"text":"negative"}),
        ] {
            assert!(parse_cue(&invalid, 1).is_err());
        }
    }

    #[test]
    fn partial_model_files_never_report_ready() {
        let root = std::env::temp_dir().join(format!("umi-asr-{}", uuid::Uuid::new_v4()));
        let model = model_dir(&root, "sensevoice").unwrap();
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(model.join("model.int8.onnx"), "partial").unwrap();
        assert!(!model_ready(&model, "sensevoice"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
