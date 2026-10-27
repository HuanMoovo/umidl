//! Shared subtitle output validation for additional recognition engines.
use std::path::Path;
use crate::subtitle::Cue;
use crate::cloud_asr::CloudAsrConfig;
use crate::models::SubtitleRequest;

#[derive(Clone, serde::Deserialize)]
pub struct AsrRequest {
    pub engine: String,
    pub subtitle: SubtitleRequest,
    #[serde(default)]
    pub cloud: Option<CloudAsrConfig>,
}

impl AsrRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(["srt", "vtt", "ass", "txt", "json"].contains(&self.subtitle.output_format.as_str()), "不支持的字幕输出格式");
        let language = self.subtitle.language.trim();
        match self.engine.as_str() {
            "sensevoice" | "paraformer" => {
                anyhow::ensure!(self.subtitle.translate_to.as_deref().unwrap_or("").is_empty(), "本地中文模型只支持原文转写，翻译请使用 Whisper");
                let languages: &[&str] = if self.engine == "sensevoice" { &["auto", "zh", "en", "ja", "ko", "yue"] } else { &["auto", "zh"] };
                anyhow::ensure!(languages.contains(&language), "所选本地模型不支持该识别语言");
            }
            "cloud" => {
                let config = self.cloud.as_ref().ok_or_else(|| anyhow::anyhow!("请填写云端识别配置"))?;
                crate::cloud_asr::validate_config(config)?;
                if matches!(config.provider.as_str(), "tencent" | "baidu" | "xunfei") {
                    crate::cloud_vendor_cn::validate_config(config, language)?;
                }
                if let Some(target) = self.subtitle.translate_to.as_deref().filter(|s| !s.is_empty()) {
                    anyhow::ensure!(target == "en" && matches!(config.provider.as_str(), "compatible" | "openai") && config.model.trim() == "whisper-1", "所选云端模型不支持翻译成英文");
                }
                anyhow::ensure!(language.len() <= 16 && language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'), "识别语言无效");
            }
            _ => anyhow::bail!("未知的语音识别引擎"),
        }
        Ok(())
    }

    pub fn task_model(&self) -> String {
        match (&*self.engine, &self.cloud) {
            ("cloud", Some(config)) if config.provider == "compatible" => config.redact(&format!("cloud:{}", config.model.trim())),
            ("cloud", Some(config)) => config.redact(&format!("cloud:{}:{}", config.provider, config.model.trim())),
            _ => self.engine.clone(),
        }
    }

    /// Only the public subtitle options enter the database. The cloud object is never serialized.
    pub fn persisted_metadata(&self) -> String {
        let mut metadata = serde_json::to_value(&self.subtitle).expect("subtitle request is serializable");
        metadata["engine"] = serde_json::json!(self.engine);
        metadata["model"] = serde_json::json!(self.task_model());
        metadata.to_string()
    }
}

pub fn validate_cues(cues: &[Cue]) -> anyhow::Result<()> {
    anyhow::ensure!(!cues.is_empty(), "未识别到有效语音，未生成字幕");
    let mut last_start = 0;
    for (index, cue) in cues.iter().enumerate() {
        anyhow::ensure!(!cue.text.trim().is_empty(), "第 {} 段识别内容为空", index + 1);
        anyhow::ensure!(cue.end_ms > cue.start_ms, "第 {} 段时间轴无效", index + 1);
        anyhow::ensure!(cue.start_ms >= last_start, "识别结果的时间轴顺序无效");
        last_start = cue.start_ms;
    }
    Ok(())
}

pub fn write_output(path: &Path, cues: &[Cue], format: &str, title: &str) -> anyhow::Result<()> {
    validate_cues(cues)?;
    anyhow::ensure!(["srt", "vtt", "ass", "txt", "json"].contains(&format), "不支持的字幕输出格式");
    let rendered = crate::subtitle::render(&crate::subtitle::to_srt(cues), format, title);
    anyhow::ensure!(!rendered.trim().is_empty(), "字幕导出内容为空");
    std::fs::write(path, rendered)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: u64, end: u64, text: &str) -> Cue {
        Cue { index: 1, start_ms: start, end_ms: end, text: text.into() }
    }

    fn request(engine: &str) -> AsrRequest {
        serde_json::from_value(serde_json::json!({
            "engine": engine,
            "subtitle": {"video_path":"test.wav","language":"auto","output_format":"srt","translate_to":"en"},
            "cloud": {"base_url":"https://api.example.test/v1","model":"whisper-1","api_key":"sk-private-must-not-persist"}
        })).unwrap()
    }

    #[test]
    fn persisted_metadata_excludes_cloud_config_and_credentials() {
        let req = request("cloud");
        let raw = req.persisted_metadata();
        assert!(!raw.contains("sk-private"));
        assert!(!raw.contains("api_key"));
        assert!(!raw.contains("base_url"));
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["engine"], "cloud");
        assert_eq!(value["translate_to"], "en");
        assert_eq!(req.task_model(), "cloud:whisper-1");
    }

    #[test]
    fn vendor_task_keeps_provider_and_never_persists_its_credentials() {
        let req: AsrRequest = serde_json::from_value(serde_json::json!({"engine":"cloud",
            "subtitle":{"video_path":"test.wav", "language":"zh", "output_format":"srt"},
            "cloud":{"provider":"fish", "base_url":"https://api.fish.audio", "model":"transcribe-1-pro",
                "api_key":"example-key-private", "secret_key":"example-secret-private", "secret_id":"example-id-private", "app_id":"example-app-private"}})).unwrap();
        assert_eq!(req.task_model(), "cloud:fish:transcribe-1-pro");
        let metadata = req.persisted_metadata();
        for secret in ["example-key-private", "example-secret-private", "example-id-private", "example-app-private", "api.fish.audio"] {
            assert!(!metadata.contains(secret));
        }
    }

    #[test]
    fn local_engine_requests_reject_unsupported_languages_and_translation() {
        let mut req = request("paraformer");
        assert!(req.validate().is_err());
        req.subtitle.translate_to = None;
        req.subtitle.language = "fr".into();
        assert!(req.validate().is_err());
        req.subtitle.language = "zh".into();
        assert!(req.validate().is_ok());
        req.engine = "arbitrary-command".into();
        assert!(req.validate().is_err());
    }

    #[test]
    fn additional_engines_cannot_report_empty_or_invalid_subtitles_as_success() {
        for cues in [vec![], vec![cue(1000, 1000, "hello")], vec![cue(2000, 1000, "hello")],
            vec![cue(0, 1000, "   ")], vec![cue(2000, 3000, "later"), cue(0, 1000, "earlier")]] {
            assert!(validate_cues(&cues).is_err());
        }
        assert!(validate_cues(&[cue(0, 1000, "你好"), cue(1200, 2500, "世界")]).is_ok());
    }

    #[test]
    fn local_and_cloud_results_use_existing_timestamped_export_formats() {
        let dir = std::env::temp_dir().join(format!("umi-asr-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cues = [cue(1200, 3200, "你好，世界")];
        for (format, expected) in [("srt", "00:00:01,200 --> 00:00:03,200"), ("vtt", "WEBVTT"),
            ("txt", "你好，世界"), ("ass", "[Events]"), ("json", "\"start_ms\": 1200")] {
            let path = dir.join(format!("result.{format}"));
            write_output(&path, &cues, format, "Test").unwrap();
            assert!(std::fs::read_to_string(path).unwrap().contains(expected), "{format}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
