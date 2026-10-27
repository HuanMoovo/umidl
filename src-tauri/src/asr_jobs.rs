use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};

use crate::asr::AsrRequest;
use crate::models::{SubtitleTask, TaskStatus};
use crate::{AppState, AppStateInner};

#[tauri::command]
pub async fn asr_engine_status(state: State<'_, AppState>, engine: String) -> Result<serde_json::Value, String> {
    if !matches!(engine.as_str(), "sensevoice" | "paraformer") { return Err("未知的本地识别引擎".into()); }
    let context = crate::build_ctx(&state.0, None);
    tauri::async_runtime::spawn_blocking(move || crate::local_asr::engine_status(&context, &engine)).await.map_err(crate::err)
}

#[tauri::command]
pub async fn install_asr_engine(app: AppHandle, state: State<'_, AppState>, engine: String) -> Result<(), String> {
    if !matches!(engine.as_str(), "sensevoice" | "paraformer") { return Err("未知的本地识别引擎".into()); }
    let context = crate::build_ctx(&state.0, Some(&app));
    let progress_name = format!("asr:{engine}");
    let _ = app.emit("tool://progress", serde_json::json!({"name":progress_name,"percent":0,"message":"正在安装本地识别运行环境与模型…"}));
    let result = tauri::async_runtime::spawn_blocking(move || crate::local_asr::install_engine(&context, &engine))
        .await.map_err(crate::err)?.map_err(crate::err);
    let _ = app.emit("tool://progress", serde_json::json!({"name":progress_name,"percent":if result.is_ok(){100}else{0},"message":if result.is_ok(){"本地识别引擎已就绪"}else{"安装未完成，请检查错误后重试"}}));
    result
}

#[tauri::command]
pub async fn start_asr(app: AppHandle, state: State<'_, AppState>, req: AsrRequest) -> Result<SubtitleTask, String> {
    req.validate().map_err(crate::err)?;
    if !Path::new(&req.subtitle.video_path).is_file() { return Err("输入文件不存在，请重新选择文件".into()); }
    let state = state.0.clone();
    let context = crate::build_ctx(&state, None);
    context.tools.ffmpeg().map_err(crate::err)?;
    if req.engine != "cloud" {
        let engine = req.engine.clone();
        let status = tauri::async_runtime::spawn_blocking(move || crate::local_asr::engine_status(&context, &engine)).await.map_err(crate::err)?;
        if status["ready"] != true { return Err(status["message"].as_str().unwrap_or("请先安装中文识别组件").into()); }
    }
    let task = crate::subtitle::new_subtitle_task(&req.subtitle, &req.task_model(), crate::ctx::now_ms());
    state.db.upsert_subtitle(&task, Some(&req.persisted_metadata())).map_err(crate::err)?;
    let _ = app.emit("subtitle://update", &task);
    let id = task.id.clone();
    std::thread::spawn(move || run_asr_job(state, app, req, id));
    Ok(task)
}

fn output_path(context: &crate::ctx::Ctx, req: &AsrRequest) -> anyhow::Result<PathBuf> {
    let source = Path::new(&req.subtitle.video_path);
    let directory = req.subtitle.output_dir.as_deref().filter(|value| !value.trim().is_empty())
        .map(PathBuf::from).unwrap_or_else(|| source.parent().map(Path::to_path_buf).unwrap_or_else(|| context.download_dir()));
    std::fs::create_dir_all(&directory)?;
    let tag = crate::subtitle::subtitle_product_tag(&req.subtitle);
    let extension = format!("{tag}.{}.{}", req.engine, req.subtitle.output_format);
    Ok(crate::converter::reserve_output_path(source, &directory, &extension)?)
}

fn redact_error(request: &AsrRequest, error: &str) -> String {
    let clean = request.cloud.as_ref().map(|config| config.redact(error)).unwrap_or_else(|| error.to_string());
    clean.chars().take(1500).collect()
}

fn run_asr_job(state: Arc<AppStateInner>, app: AppHandle, request: AsrRequest, id: String) {
    if state.is_removed(&id) { return; }
    state.mark_running(&id);
    let context = crate::build_ctx(&state, Some(&app));
    let mut task = state.db.get_subtitle(&id).ok().flatten()
        .unwrap_or_else(|| crate::subtitle::new_subtitle_task(&request.subtitle, &request.task_model(), crate::ctx::now_ms()));
    let directory = context.dirs.cache.join("asr-work").join(&id);
    let wav = directory.join("audio.wav");
    let mut reserved: Option<PathBuf> = None;
    let result = (|| -> anyhow::Result<PathBuf> {
        anyhow::ensure!(!state.is_cancelled(&id), "任务已取消");
        std::fs::create_dir_all(&directory)?;
        task.status = TaskStatus::Extracting;
        task.progress = 2.0;
        crate::persist_subtitle(&state, &task);
        let _ = app.emit("subtitle://update", &task);
        let extraction = crate::subtitle::extract_audio(&context, &request.subtitle.video_path, &wav,
            |pid| state.register_pid(&id, pid), || state.is_cancelled(&id))?;
        state.take_pid(&id);
        anyhow::ensure!(!state.is_cancelled(&id), "任务已取消");
        anyhow::ensure!(extraction.exit_code == 0 && !extraction.killed && wav.is_file(), "音频提取失败：{}", extraction.stderr_tail);
        task.status = TaskStatus::Transcribing;
        task.progress = 10.0;
        crate::persist_subtitle(&state, &task);
        let _ = app.emit("subtitle://update", &task);
        let cues = if request.engine == "cloud" {
            crate::cloud_asr::transcribe(&context, &wav, request.cloud.as_ref().ok_or_else(|| anyhow::anyhow!("云端识别配置缺失"))?,
                &request.subtitle.language, request.subtitle.translate_to.as_deref(), &|| state.is_cancelled(&id))?
        } else {
            crate::local_asr::transcribe(&context, &wav, &request.engine, &request.subtitle.language,
                &|pid| state.register_pid(&id, pid), &|| state.is_cancelled(&id))?
        };
        anyhow::ensure!(!state.is_cancelled(&id), "任务已取消");
        anyhow::ensure!(!state.is_removed(&id), "任务已删除");
        crate::asr::validate_cues(&cues)?;
        let destination = output_path(&context, &request)?;
        reserved = Some(destination.clone());
        let title = Path::new(&request.subtitle.video_path).file_stem().and_then(|s| s.to_str()).unwrap_or("subtitle");
        crate::asr::write_output(&destination, &cues, &request.subtitle.output_format, title)?;
        Ok(destination)
    })();
    state.take_pid(&id);
    state.unmark_running(&id);
    let canceled = state.is_cancelled(&id);
    state.clear_cancel(&id);
    let _ = std::fs::remove_file(&wav);
    let _ = std::fs::remove_dir(&directory);
    if state.is_removed(&id) { return; }
    match result {
        Ok(path) if !canceled => {
            task.status = TaskStatus::Done;
            task.progress = 100.0;
            task.error = None;
            task.subtitle_path = Some(path.to_string_lossy().into_owned());
            if state.settings_snapshot().open_folder_when_done { let _ = crate::reveal_path(path.to_string_lossy().into_owned()); }
            if state.settings_snapshot().notify_on_finish { crate::notify_finish("Umidl · 字幕生成完成", path.file_name().and_then(|s| s.to_str()).unwrap_or("字幕")); }
        }
        result => {
            if let Some(path) = reserved {
                if path.metadata().map(|m| m.len() == 0).unwrap_or(false) { let _ = std::fs::remove_file(path); }
            }
            task.status = if canceled { TaskStatus::Canceled } else { TaskStatus::Error };
            task.error = if canceled { None } else { result.err().map(|error| redact_error(&request, &error.to_string())) };
        }
    }
    crate::persist_subtitle(&state, &task);
    let _ = app.emit("subtitle://update", &task);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_errors_never_expose_the_live_cloud_key() {
        let req: AsrRequest = serde_json::from_value(serde_json::json!({"engine":"cloud", "subtitle":{"video_path":"test.wav"},
            "cloud":{"base_url":"https://example.test/v1","model":"whisper-1","api_key":"synthetic-private-key"}})).unwrap();
        let error = redact_error(&req, "upstream echoed synthetic-private-key while failing");
        assert!(!error.contains("synthetic-private-key"));
        assert!(error.contains("[REDACTED]"));
    }

    #[test]
    fn vendor_errors_redact_each_kind_of_credential() {
        let req: AsrRequest = serde_json::from_value(serde_json::json!({"engine":"cloud", "subtitle":{"video_path":"test.wav"},
            "cloud":{"provider":"tencent", "base_url":"https://asr.cloud.tencent.com", "model":"16k_zh", "api_key":"secret-key-1", "secret_key":"secret-key-2", "secret_id":"secret-id-3", "app_id":"secret-app-4"}})).unwrap();
        let cleaned = redact_error(&req, "secret-key-1 secret-key-2 secret-id-3 secret-app-4");
        for secret in ["secret-key-1", "secret-key-2", "secret-id-3", "secret-app-4"] {
            assert!(!cleaned.contains(secret), "credential was not removed");
        }
    }
}
