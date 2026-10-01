//! 自动化自检系统：真实调用 yt-dlp / ffmpeg / whisper 端到端验证全部功能
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::ctx::{command_for, Ctx};
use crate::db::Db;
use crate::models::*;
use crate::{converter, downloader, http_testserver, subtitle, tools};

pub struct CaseEnv {
    pub ctx: Ctx,
    pub db: Arc<Db>,
    pub work: PathBuf,
}

impl CaseEnv {
    pub fn tiny_ctx(&self) -> Ctx {
        let mut c = self.ctx.clone();
        c.settings.whisper_model = "tiny".into();
        c
    }
    pub fn p(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }
}

type CaseFn = Box<dyn Fn(&CaseEnv) -> Result<(String, Option<String>), String> + Send + Sync>;

pub struct Case {
    pub id: &'static str,
    pub group: &'static str,
    pub name: &'static str,
    pub heavy: bool,
    pub f: CaseFn,
}

pub fn case_catalog() -> Vec<(String, String, String, bool)> {
    all_cases()
        .iter()
        .map(|c| (c.id.into(), c.group.into(), c.name.into(), c.heavy))
        .collect()
}

/* ==================== 小工具 ==================== */

fn ok(detail: impl Into<String>) -> Result<(String, Option<String>), String> {
    Ok((detail.into(), None))
}
fn skip(detail: impl Into<String>) -> Result<(String, Option<String>), String> {
    Ok((String::new(), Some(detail.into())))
}

fn run_proc(exe: &Path, args: &[String]) -> Result<(i32, String, String), String> {
    let out = command_for(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("启动进程失败：{e}"))?;
    Ok((
        out.status.code().unwrap_or(-1),
        crate::ctx::decode_output(&out.stdout),
        crate::ctx::decode_output(&out.stderr),
    ))
}

fn need_tool(ctx: &Ctx, which: &str) -> Result<PathBuf, String> {
    let p = match which {
        "ffmpeg" => ctx.tools.ffmpeg(),
        "ffprobe" => ctx.tools.ffprobe(),
        "yt-dlp" => ctx.tools.ytdlp(),
        "whisper" => ctx.tools.whisper(),
        _ => return Err("未知工具".into()),
    };
    p.map(|x| x.to_path_buf()).map_err(|e| e.to_string())
}

fn make_video(
    ctx: &Ctx,
    out: &Path,
    w: u32,
    h: u32,
    secs: f64,
    crf: u32,
    with_audio: bool,
) -> Result<(), String> {
    let ffmpeg = need_tool(ctx, "ffmpeg")?;
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("testsrc2=size={w}x{h}:rate=25"),
    ];
    if with_audio {
        args.push("-f".into());
        args.push("lavfi".into());
        args.push("-i".into());
        args.push("sine=frequency=440:sample_rate=44100".into());
    }
    args.push("-t".into());
    args.push(format!("{secs}"));
    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-preset".into());
    args.push("ultrafast".into());
    args.push("-crf".into());
    args.push(crf.to_string());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    if with_audio {
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("96k".into());
        args.push("-shortest".into());
    }
    args.push(out.to_string_lossy().to_string());
    let (code, _, errtxt) = run_proc(&ffmpeg, &args)?;
    if code != 0 || !out.is_file() {
        return Err(format!(
            "合成测试视频失败(code={code})：{}",
            errtxt.lines().last().unwrap_or("")
        ));
    }
    Ok(())
}

fn size_of(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn find_media(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_file() {
            let s = size_of(&p);
            if best.as_ref().map(|(bs, _)| s > *bs).unwrap_or(true) {
                best = Some((s, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/* ==================== 用例清单 ==================== */

fn all_cases() -> Vec<Case> {
    let mut v: Vec<Case> = Vec::new();
    macro_rules! case {
        ($id:expr, $group:expr, $name:expr, $heavy:expr, $f:expr) => {
            v.push(Case {
                id: $id,
                group: $group,
                name: $name,
                heavy: $heavy,
                f: Box::new($f),
            });
        };
    }

    /* ---------- 环境依赖 ---------- */
    case!("env.ytdlp", "环境依赖", "yt-dlp 可用性", false, |e: &CaseEnv| {
        let p = need_tool(&e.ctx, "yt-dlp")?;
        let v = tools::tool_version(&p, "yt-dlp").ok_or("无法获取版本")?;
        ok(format!("{} ({})", v, p.display()))
    });
    case!("env.ffmpeg", "环境依赖", "FFmpeg 可用性", false, |e: &CaseEnv| {
        let p = need_tool(&e.ctx, "ffmpeg")?;
        let v = tools::tool_version(&p, "ffmpeg").ok_or("无法获取版本")?;
        ok(v)
    });
    case!("env.ffprobe", "环境依赖", "FFprobe 可用性", false, |e: &CaseEnv| {
        let p = need_tool(&e.ctx, "ffprobe")?;
        let v = tools::tool_version(&p, "ffprobe").ok_or("无法获取版本")?;
        ok(v)
    });
    case!("env.whisper", "环境依赖", "Whisper 引擎可用性", false, |e: &CaseEnv| {
        let p = need_tool(&e.ctx, "whisper")?;
        let v = tools::tool_version(&p, "whisper");
        ok(format!("{} ({})", v.unwrap_or_else(|| "whisper.cpp".into()), p.display()))
    });
    case!("env.model", "环境依赖", "Whisper 模型文件", false, |e: &CaseEnv| {
        let c = e.tiny_ctx();
        let m = c.model_path();
        if m.is_file() {
            ok(format!("{} · {}", m.file_name().unwrap_or_default().to_string_lossy(), tools::human_size(size_of(&m))))
        } else {
            Err(format!("缺少模型文件 {}", m.display()))
        }
    });

    /* ---------- FFmpeg 转换 ---------- */
    case!("ffmpeg.synth", "转换模块", "合成标准测试素材", false, |e: &CaseEnv| {
        make_video(&e.ctx, &e.p("test_src.mp4"), 640, 360, 6.0, 26, true)?;
        make_video(&e.ctx, &e.p("short_src.mp4"), 320, 240, 2.0, 30, true)?;
        make_video(&e.ctx, &e.p("big_src.mp4"), 1280, 720, 4.0, 20, true)?;
        let a = size_of(&e.p("test_src.mp4"));
        let b = size_of(&e.p("short_src.mp4"));
        let c = size_of(&e.p("big_src.mp4"));
        if a < 5_000 || b < 1_000 || c < 20_000 {
            return Err(format!("合成素材体积异常：{a}/{b}/{c} 字节"));
        }
        ok(format!("标准 {} · 短片 {} · 大尺寸 {}", tools::human_size(a), tools::human_size(b), tools::human_size(c)))
    });
    case!("ffmpeg.probe", "转换模块", "媒体信息探测(ffprobe)", false, |e: &CaseEnv| {
        let info = converter::probe_media(&e.ctx, &e.p("test_src.mp4").to_string_lossy()).map_err(|x| x.to_string())?;
        let d = info.duration.ok_or("缺少时长")?;
        if !(5.0..=7.5).contains(&d) {
            return Err(format!("时长异常：{d}"));
        }
        if info.video_codec.as_deref() != Some("h264") {
            return Err(format!("视频编码异常：{:?}", info.video_codec));
        }
        if info.audio_codec.is_none() {
            return Err("缺少音频流".into());
        }
        if info.width != Some(640) || info.height != Some(360) {
            return Err(format!("分辨率异常：{}x{}", info.width.unwrap_or(0), info.height.unwrap_or(0)));
        }
        ok(format!("{:.2}s · {} · {}", d, info.video_codec.clone().unwrap_or_default(), info.audio_codec.clone().unwrap_or_default()))
    });
    case!("ffmpeg.mp4_to_mkv", "转换模块", "MP4 → MKV 转封装", false, |e: &CaseEnv| {
        convert_and_verify(e, "test_src.mp4", "mkv", false, 640, 360, Some("h264"))
    });
    case!("ffmpeg.mp4_to_webm", "转换模块", "MP4 → WebM (VP9/Opus)", false, |e: &CaseEnv| {
        convert_and_verify(e, "short_src.mp4", "webm", false, 320, 240, Some("vp9"))
    });
    case!("ffmpeg.mp4_to_avi", "转换模块", "MP4 → AVI", false, |e: &CaseEnv| {
        convert_and_verify(e, "short_src.mp4", "avi", false, 320, 240, None)
    });
    case!("ffmpeg.h265", "转换模块", "H.265/HEVC 编码", false, |e: &CaseEnv| {
        convert_and_verify(e, "short_src.mp4", "mp4", false, 320, 240, Some("hevc"))
    });
    case!("ffmpeg.scale", "转换模块", "分辨率缩放 720p → 360p", false, |e: &CaseEnv| {
        let (path, _) = convert_with(
            e,
            "big_src.mp4",
            "mp4",
            &ConvertRequest {
                input_file: e.p("big_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: "mp4".into(),
                video_codec: Some("libx264".into()),
                audio_codec: Some("aac".into()),
                bitrate: None,
                resolution: Some("640x360".into()),
                crf: Some(30),
                extract_audio: false,
                mute: false,
                ..Default::default()
            },
        )?;
        let info = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if info.width != Some(640) || info.height != Some(360) {
            return Err(format!("缩放结果异常：{}x{}", info.width.unwrap_or(0), info.height.unwrap_or(0)));
        }
        ok("640x360 缩放正确")
    });
    case!("ffmpeg.mute", "转换模块", "去除音轨", false, |e: &CaseEnv| {
        let (path, _) = convert_with(
            e,
            "short_src.mp4",
            "mp4",
            &ConvertRequest {
                input_file: e.p("short_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: "mp4".into(),
                video_codec: Some("libx264".into()),
                audio_codec: None,
                bitrate: None,
                resolution: None,
                crf: Some(30),
                extract_audio: false,
                mute: true,
                ..Default::default()
            },
        )?;
        let info = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if info.audio_codec.is_some() {
            return Err(format!("仍存在音频轨：{:?}", info.audio_codec));
        }
        ok("音轨已移除")
    });
    case!("ffmpeg.extract_mp3", "转换模块", "提取音频 MP3", false, |e: &CaseEnv| {
        let (path, _) = convert_with(
            e,
            "test_src.mp4",
            "mp3",
            &ConvertRequest {
                input_file: e.p("test_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: "mp3".into(),
                video_codec: None,
                audio_codec: Some("libmp3lame".into()),
                bitrate: None,
                resolution: None,
                crf: None,
                extract_audio: true,
                mute: false,
                ..Default::default()
            },
        )?;
        let info = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if info.audio_codec.as_deref() != Some("mp3") {
            return Err(format!("编码异常：{:?}", info.audio_codec));
        }
        if info.video_codec.is_some() {
            return Err("MP3 中不应包含视频流".into());
        }
        ok(format!("MP3 {:.1}s", info.duration.unwrap_or(0.0)))
    });
    case!("ffmpeg.extract_wav", "转换模块", "提取音频 WAV 16k 单声道", false, |e: &CaseEnv| {
        let (path, _) = convert_with(
            e,
            "test_src.mp4",
            "wav",
            &ConvertRequest {
                input_file: e.p("test_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: "wav".into(),
                video_codec: None,
                audio_codec: Some("pcm_s16le".into()),
                bitrate: None,
                resolution: None,
                crf: None,
                extract_audio: true,
                mute: false,
                ..Default::default()
            },
        )?;
        let info = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if info.audio_codec.as_deref() != Some("pcm_s16le") {
            return Err(format!("编码异常：{:?}", info.audio_codec));
        }
        ok("PCM 16bit 提取成功")
    });
    case!("ffmpeg.compress", "转换模块", "视频压缩（体积下降）", false, |e: &CaseEnv| {
        let (path, _) = convert_with(
            e,
            "big_src.mp4",
            "mp4",
            &ConvertRequest {
                input_file: e.p("big_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: "mp4".into(),
                video_codec: Some("libx264".into()),
                audio_codec: Some("aac".into()),
                bitrate: None,
                resolution: None,
                crf: Some(38),
                extract_audio: false,
                mute: false,
                ..Default::default()
            },
        )?;
        let src = size_of(&e.p("big_src.mp4"));
        let dst = size_of(&path);
        if dst == 0 {
            return Err("输出为空".into());
        }
        if dst >= src {
            return Err(format!("压缩后未变小：{} → {}", tools::human_size(src), tools::human_size(dst)));
        }
        ok(format!("{} → {}", tools::human_size(src), tools::human_size(dst)))
    });
    case!("ffmpeg.thumbnail", "转换模块", "提取视频缩略图", false, |e: &CaseEnv| {
        let out = e.p("thumb.jpg");
        converter::extract_thumbnail(&e.ctx, &e.p("test_src.mp4").to_string_lossy(), 1.0, &out).map_err(|x| x.to_string())?;
        let s = size_of(&out);
        if s < 800 {
            return Err(format!("缩略图过小：{s} 字节"));
        }
        ok(format!("{}", tools::human_size(s)))
    });
    case!("ffmpeg.progress", "转换模块", "转码进度解析", false, |_e: &CaseEnv| {
        let t = converter::parse_convert_line("out_time=00:00:03.000000", Some(6.0))
            .ok_or("未解析到进度")?;
        let pct = t.percent.ok_or("未计算百分比")?;
        if (pct - 50.0).abs() > 0.5 {
            return Err(format!("百分比错误：{pct}"));
        }
        let t2 = converter::parse_convert_line("progress=end", Some(6.0)).ok_or("未解析结束标记")?;
        if !t2.done {
            return Err("结束标记未识别".into());
        }
        let t3 = converter::parse_convert_line("speed=2.5x", Some(6.0)).ok_or("未解析速度")?;
        if t3.speed.as_deref() != Some("2.5x") {
            return Err("速度解析错误".into());
        }
        ok("out_time / progress / speed 解析正确")
    });
    case!("ffmpeg.error", "转换模块", "错误处理（文件不存在）", false, |e: &CaseEnv| {
        let r = converter::probe_media(&e.ctx, &e.p("not-exist.mp4").to_string_lossy())
            .map_err(|x| x.to_string());
        match r {
            Err(msg) if msg.to_string().contains("不存在") => ok("正确返回友好错误"),
            Err(msg) => ok(format!("返回错误：{msg}")),
            Ok(_) => Err("对不存在的文件竟然返回成功".into()),
        }
    });
    case!("ffmpeg.unique_out", "转换模块", "输出文件重名保护", false, |e: &CaseEnv| {
        let dir = e.p("out");
        let src = e.p("test_src.mp4");
        let p1 = converter::output_path_for(&src, &dir, "mp4");
        std::fs::write(&p1, b"x").map_err(|x| x.to_string())?;
        let p2 = converter::output_path_for(&src, &dir, "mp4");
        if p1 == p2 {
            return Err("重名时未生成新文件名".into());
        }
        let _ = std::fs::remove_file(&p1);
        ok(format!("自动避让：{}", p2.file_name().unwrap_or_default().to_string_lossy()))
    });

    /* ---------- 下载模块 ---------- */
    /* ---------- 新增：更多格式 / 自动检测 / 高级选项 ---------- */
    case!("ffmpeg.auto_detect", "转换模块", "自动检测输出格式（无损封装）", false, |e: &CaseEnv| {
        let info = converter::probe_media(&e.ctx, &e.p("test_src.mp4").to_string_lossy())
            .map_err(|x| x.to_string())?;
        let t = converter::auto_target(&info);
        if t.format != "mp4" || t.video != "copy" || t.audio != "copy" {
            return Err(format!("自动检测结果异常：{} / {} / {}", t.format, t.video, t.audio));
        }
        let req = ConvertRequest {
            input_file: e.p("test_src.mp4").to_string_lossy().to_string(),
            output_dir: Some(e.p("out").to_string_lossy().to_string()),
            format: t.format.clone(),
            video_codec: Some(t.video.clone()),
            audio_codec: Some(t.audio.clone()),
            resolution: None,
            crf: None,
            extract_audio: t.extract_audio,
            ..Default::default()
        };
        let (path, _o) = convert_with(e, "test_src.mp4", &t.format, &req)?;
        if !path.is_file() || size_of(&path) < 5_000 {
            return Err("输出文件缺失或体积异常".into());
        }
        let p2 = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if p2.video_codec.as_deref() != Some("h264") {
            return Err(format!("封装后编码异常：{:?}", p2.video_codec));
        }
        ok(format!("{} → {} · {}", t.reason, t.format, tools::human_size(size_of(&path))))
    });
    case!("ffmpeg.extra_audio_formats", "转换模块", "更多音频格式（M4A/FLAC/OPUS）", false, |e: &CaseEnv| {
        let mut done: Vec<String> = Vec::new();
        for (fmt, codec) in [("m4a", "aac"), ("flac", "flac"), ("opus", "libopus")] {
            let req = ConvertRequest {
                input_file: e.p("short_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: fmt.into(),
                video_codec: None,
                audio_codec: Some(codec.into()),
                resolution: None,
                crf: None,
                extract_audio: true,
                ..Default::default()
            };
            let (path, _o) = convert_with(e, "short_src.mp4", fmt, &req)?;
        if !path.is_file() || size_of(&path) < 1_000 {
            return Err("输出文件缺失或体积异常".into());
        }
            done.push(format!("{fmt} {}", tools::human_size(size_of(&path))));
        }
        ok(done.join(" · "))
    });
    case!("ffmpeg.extra_video_formats", "转换模块", "更多视频格式（TS/MOV/GIF）", false, |e: &CaseEnv| {
        let mut done: Vec<String> = Vec::new();
        for (fmt, vc, ac) in [
            ("ts", "libx264", "aac"),
            ("mov", "libx264", "aac"),
            ("gif", "gif", "none"),
        ] {
            let req = ConvertRequest {
                input_file: e.p("short_src.mp4").to_string_lossy().to_string(),
                output_dir: Some(e.p("out").to_string_lossy().to_string()),
                format: fmt.into(),
                video_codec: Some(vc.into()),
                audio_codec: Some(ac.into()),
                resolution: None,
                crf: Some(32),
                extract_audio: false,
                ..Default::default()
            };
            let (path, _o) = convert_with(e, "short_src.mp4", fmt, &req)?;
        if !path.is_file() || size_of(&path) < 500 {
            return Err("输出文件缺失或体积异常".into());
        }
            done.push(format!("{fmt} {}", tools::human_size(size_of(&path))));
        }
        ok(done.join(" · "))
    });
    case!("ffmpeg.advanced_opts", "转换模块", "高级选项（变速/帧率/清元数据）", false, |e: &CaseEnv| {
        let req = ConvertRequest {
            input_file: e.p("test_src.mp4").to_string_lossy().to_string(),
            output_dir: Some(e.p("out").to_string_lossy().to_string()),
            format: "mp4".into(),
            video_codec: Some("libx264".into()),
            audio_codec: Some("aac".into()),
            resolution: None,
            crf: Some(30),
            extract_audio: false,
            fps: Some(15),
            speed: Some(2.0),
            remove_metadata: true,
            ..Default::default()
        };
        let (path, _o) = convert_with(e, "test_src.mp4", "mp4", &req)?;
        let p = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        let d = p.duration.unwrap_or(0.0);
        if !(2.2..=4.2).contains(&d) {
            return Err(format!("2 倍速后时长异常：{d:.2}s（6s 素材预期 ≈3s）"));
        }
        let f = p.fps.unwrap_or(0.0);
        if (f - 15.0).abs() > 1.5 {
            return Err(format!("帧率未生效：{f}（预期 15）"));
        }
        ok(format!("{d:.2}s · {}fps · 变速+限帧+清元数据生效", f.round()))
    });
    case!("ffmpeg.audio_params", "转换模块", "音频参数（码率/采样率/声道）", false, |e: &CaseEnv| {
        let req = ConvertRequest {
            input_file: e.p("short_src.mp4").to_string_lossy().to_string(),
            output_dir: Some(e.p("out").to_string_lossy().to_string()),
            format: "mp3".into(),
            video_codec: None,
            audio_codec: Some("libmp3lame".into()),
            resolution: None,
            crf: None,
            extract_audio: true,
            audio_bitrate: Some("128k".into()),
            sample_rate: Some(44100),
            channels: Some(1),
            ..Default::default()
        };
        let args = converter::build_ffmpeg_args(&req, Path::new("x.mp3"));
        let j = args.join(" ");
        for need in ["-ar 44100", "-ac 1", "-b:a 128k"] {
            if !j.contains(need) {
                return Err(format!("缺少音频参数：{need}"));
            }
        }
        let (path, _o) = convert_with(e, "short_src.mp4", "mp3", &req)?;
        if !path.is_file() || size_of(&path) < 1_000 {
            return Err("输出文件缺失或体积异常".into());
        }
        let p = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
        if p.audio_codec.as_deref() != Some("mp3") {
            return Err(format!("音频编码异常：{:?}", p.audio_codec));
        }
        ok(format!("44.1kHz 单声道 128k · {}", tools::human_size(size_of(&path))))
    });
    case!("convert.hw_encoders", "转换模块", "硬件编码参数映射", false, |_e: &CaseEnv| {
        // 硬件编码器不能带 -preset medium（会直接报错），必须走各自的码控参数
        let nv = converter::quality_args("h264_nvenc", 23).join(" ");
        if !nv.contains("-cq 23") || nv.contains("-preset medium") {
            return Err(format!("nvenc 参数错误：{nv}"));
        }
        let qsv = converter::quality_args("hevc_qsv", 23).join(" ");
        if !qsv.contains("-global_quality 23") {
            return Err(format!("qsv 参数错误：{qsv}"));
        }
        let amf = converter::quality_args("h264_amf", 23).join(" ");
        if !amf.contains("-rc cqp") {
            return Err(format!("amf 参数错误：{amf}"));
        }
        if !converter::quality_args("gif", 23).is_empty() {
            return Err("gif 不应携带质量参数".into());
        }
        if converter::normalize_audio_codec("mp3") != "libmp3lame" {
            return Err("音频编码别名归一化失败".into());
        }
        ok("nvenc / qsv / amf / gif 参数映射正确".to_string())
    });
    case!("convert.atempo", "转换模块", "变速滤镜链（超出 0.5~2.0）", false, |_e: &CaseEnv| {
        let c = converter::atempo_chain(3.0);
        if !c.contains("atempo=2.0") || c.split('.').count() < 2 {
            return Err(format!("3 倍速滤镜链错误：{c}"));
        }
        let c2 = converter::atempo_chain(0.25);
        if !c2.starts_with("atempo=0.5") {
            return Err(format!("0.25 倍速滤镜链错误：{c2}"));
        }
        ok(format!("3x → {c}"))
    });
    case!("download.args", "下载模块", "yt-dlp 参数构建", false, |e: &CaseEnv| {
        let req = DownloadRequest {
            url: "https://example.com/v".into(),
            format_id: Some("137".into()),
            mode: "video".into(),
            output_dir: None,
            audio_format: None,
            merge_container: Some("mp4".into()),
            embed_thumbnail: true,
            embed_subs: false,
            filename_template: None,
            playlist: false,
            rate_limit: None,
            ..Default::default()
        };
        let args = downloader::build_args(&req, &e.ctx, Path::new("out"));
        let joined = args.join(" ");
        for need in ["--newline", "--progress", "-f", "137+bestaudio/137", "--merge-output-format", "--embed-thumbnail", "-o"] {
            if !joined.contains(need) {
                return Err(format!("缺少参数 {need}"));
            }
        }
        let req2 = DownloadRequest { mode: "audio".into(), audio_format: Some("flac".into()), ..req.clone() };
        let j2 = downloader::build_args(&req2, &e.ctx, Path::new("out")).join(" ");
        if !j2.contains("-x") || !j2.contains("flac") {
            return Err("音频模式参数缺失".into());
        }
        // 本机地址必须绕过系统代理直连（否则 Clash 等会把 127.0.0.1 也代理走 → 502）
        let local = DownloadRequest { url: "http://127.0.0.1:8080/a.mp4".into(), ..req.clone() };
        let jl = downloader::build_args(&local, &e.ctx, Path::new("out")).join(" ");
        if !jl.contains("--proxy ") {
            return Err("本机地址未添加 --proxy 直连参数".into());
        }
        let lh = DownloadRequest { url: "http://localhost:8080/a.mp4".into(), ..req.clone() };
        if !downloader::build_args(&lh, &e.ctx, Path::new("out")).join(" ").contains("--proxy ") {
            return Err("localhost 未添加 --proxy 直连参数".into());
        }
        if downloader::url_is_loopback("https://example.com/v") {
            return Err("公网地址被误判为本机".into());
        }
        // 仅封面模式：跳过视频下载，只写封面
        let th = DownloadRequest { mode: "thumb".into(), ..req.clone() };
        let jt = downloader::build_args(&th, &e.ctx, Path::new("out")).join(" ");
        if !jt.contains("--write-thumbnail") || !jt.contains("--skip-download") {
            return Err("仅封面模式参数缺失".into());
        }
        // 仅字幕模式：多语言 + 转 srt
        let sb = DownloadRequest {
            mode: "subs".into(),
            subtitle_langs: vec!["zh".into(), "en".into()],
            ..req.clone()
        };
        let js = downloader::build_args(&sb, &e.ctx, Path::new("out")).join(" ");
        if !js.contains("--write-subs") || !js.contains("--convert-subs") || !js.contains("zh,en") {
            return Err("仅字幕模式参数缺失".into());
        }
        ok(format!("视频 {} 项 / 音频 {} 项 / 封面+字幕参数校验通过", args.len(), j2.split(' ').count()))
    });
    case!("download.progress", "下载模块", "下载进度解析", false, |_e: &CaseEnv| {
        let t = downloader::parse_progress("[download]  42.3% of  165.53MiB at  3.21MiB/s ETA 00:52")
            .ok_or("未解析进度")?;
        if (t.percent.unwrap_or(0.0) - 42.3).abs() > 0.01 {
            return Err("百分比错误".into());
        }
        let total = t.total.ok_or("缺少总大小")?;
        if total != (165.53 * 1024.0 * 1024.0) as i64 {
            return Err(format!("总大小换算错误：{total}"));
        }
        if t.speed.as_deref().unwrap_or("").is_empty() || t.eta.as_deref() != Some("00:52") {
            return Err("速度/剩余时间解析错误".into());
        }
        if downloader::parse_progress("[download] 100% of 1.00MiB in 00:01").is_none() {
            return Err("完成行解析失败".into());
        }
        ok(format!("{} · {} · ETA {}", t.percent.unwrap(), tools::human_size(total as u64), t.eta.clone().unwrap()))
    });
    case!("download.media_info", "下载模块", "媒体信息 JSON 解析", false, |_e: &CaseEnv| {
        let raw = r#"{
          "id":"abc123","title":"测试视频 · Demo","thumbnail":"https://x/y.jpg",
          "uploader":"umi","duration":212.5,"webpage_url":"https://x/watch",
          "extractor_key":"Youtube","filesize_approx":12345678,
          "subtitles":{"zh-Hans":[{"ext":"vtt"}],"en":[{"ext":"vtt"}]},
          "formats":[
            {"format_id":"137","ext":"mp4","width":1920,"height":1080,"fps":30,"vcodec":"avc1","acodec":"none","filesize":10000000},
            {"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","filesize":2000000},
            {"format_id":"18","ext":"mp4","resolution":"640x360","vcodec":"avc1","acodec":"mp4a.40.2"}
          ]}"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let info = downloader::parse_media_info(&v);
        if info.title != "测试视频 · Demo" {
            return Err("标题解析错误".into());
        }
        if info.formats.len() != 3 {
            return Err(format!("格式数量错误：{}", info.formats.len()));
        }
        if (info.duration.unwrap_or(0.0) - 212.5).abs() >= 0.001 {
            return Err("时长解析错误".into());
        }
        let f0 = &info.formats[0];
        if !f0.has_video || f0.has_audio || f0.resolution != "1920x1080" {
            return Err(format!("格式0 解析错误：{:?}", f0));
        }
        if info.formats[1].has_video || !info.formats[1].has_audio {
            return Err("音频格式识别错误".into());
        }
        if info.subtitles.len() != 2 {
            return Err("字幕语言解析错误".into());
        }
        if !info.filesize_approx.unwrap_or(0) > 0 {
            return Err("总大小解析错误".into());
        }
        ok(format!("3 格式 · 2 字幕语言 · {:.1}s", info.duration.unwrap()))
    });
    case!("download.error_line", "下载模块", "错误行识别", false, |_e: &CaseEnv| {
        match downloader::classify_line("ERROR: Unsupported URL: https://x/y") {
            downloader::LineKind::Error(m) if m.contains("Unsupported URL") => ok(m),
            _ => Err("未识别出错误信息".into()),
        }
    });
    case!("download.local_http", "下载模块", "本地 HTTP 真实下载（含进度）", true, |e: &CaseEnv| {
        let server = http_testserver::serve_dir(&e.work).map_err(|x| x.to_string())?;
        let out = e.p("dl_basic");
        std::fs::create_dir_all(&out).map_err(|x| x.to_string())?;
        let req = DownloadRequest {
            url: server.url("test_src.mp4"),
            format_id: None,
            mode: "best".into(),
            output_dir: Some(out.to_string_lossy().to_string()),
            audio_format: None,
            merge_container: Some("mp4".into()),
            embed_thumbnail: false,
            embed_subs: false,
            filename_template: None,
            playlist: false,
            rate_limit: None,
            ..Default::default()
        };
        let ticks = Arc::new(AtomicUsize::new(0));
        let t2 = ticks.clone();
        let outcome = downloader::run_download(
            &e.ctx,
            &req,
            &out,
            move |_| {
                t2.fetch_add(1, Ordering::SeqCst);
            },
            |_| {},
            || false,
        )
        .map_err(|x| x.to_string())?;
        server.stop();
        if outcome.exit_code != 0 {
            return Err(format!("yt-dlp 退出码 {}：{}", outcome.exit_code, outcome.stderr_tail.lines().last().unwrap_or("")));
        }
        let f = find_media(&out).ok_or("未生成下载文件")?;
        let src = size_of(&e.p("test_src.mp4"));
        if size_of(&f) != src {
            return Err(format!("文件大小不一致：{} vs {}", size_of(&f), src));
        }
        ok(format!(
            "{} · 进度事件 {} 次 · HTTP 请求 {}",
            tools::human_size(src),
            ticks.load(Ordering::SeqCst),
            server.requests.load(Ordering::SeqCst)
        ))
    });
    case!("download.pause_resume", "下载模块", "暂停 / 断点续传", true, |e: &CaseEnv| {
        let server = http_testserver::serve_dir(&e.work).map_err(|x| x.to_string())?;
        // 构造一个较大的下载源（约 4MB）
        let big = e.p("big_src.mp4");
        let base = std::fs::read(&big).map_err(|x| x.to_string())?;
        let mut padded = Vec::with_capacity(base.len() * 12);
        for _ in 0..12 {
            padded.extend_from_slice(&base);
        }
        let target = e.p("large_src.mp4");
        std::fs::write(&target, &padded).map_err(|x| x.to_string())?;
        let total = padded.len() as u64;
        let rate = (total / 30).max(60_000); // 约 30 秒下载完
        let out = e.p("dl_resume");
        std::fs::create_dir_all(&out).map_err(|x| x.to_string())?;

        let req = DownloadRequest {
            url: server.url("large_src.mp4"),
            format_id: None,
            mode: "best".into(),
            output_dir: Some(out.to_string_lossy().to_string()),
            audio_format: None,
            merge_container: Some("mp4".into()),
            embed_thumbnail: false,
            embed_subs: false,
            filename_template: None,
            playlist: false,
            rate_limit: Some(rate.to_string()),
            ..Default::default()
        };

        // 第一次：跑到 8% 主动暂停
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let first = downloader::run_download(
            &e.ctx,
            &req,
            &out,
            move |t| {
                if t.percent.unwrap_or(0.0) >= 8.0 {
                    c2.store(true, Ordering::SeqCst);
                }
            },
            |_| {},
            || cancel.load(Ordering::SeqCst),
        )
        .map_err(|x| x.to_string())?;
        if !first.killed {
            return Err("暂停未生效（进程未被终止）".into());
        }
        let part = std::fs::read_dir(&out)
            .map_err(|x| x.to_string())?
            .flatten()
            .map(|x| x.path())
            .find(|p| p.to_string_lossy().ends_with(".part"));
        let part_path = part.ok_or("暂停后未保留 .part 断点文件")?;
        let part_size = size_of(&part_path);
        if part_size == 0 || part_size >= total {
            return Err(format!("断点文件大小异常：{part_size}/{total}"));
        }

        // 第二次：续传至完成
        let resumed = downloader::run_download(&e.ctx, &req, &out, |_| {}, |_| {}, || false).map_err(|x| x.to_string())?;
        server.stop();
        if resumed.exit_code != 0 {
            return Err(format!("续传失败：{}", resumed.stderr_tail.lines().last().unwrap_or("")));
        }
        let f = find_media(&out).ok_or("续传后未找到成品文件")?;
        if size_of(&f) != total {
            return Err(format!("续传后大小不完整：{}/{}", size_of(&f), total));
        }
        ok(format!(
            "断点 {}/{} → 续传完成，Range 请求 {} 次",
            tools::human_size(part_size),
            tools::human_size(total),
            server.range_requests.load(Ordering::SeqCst)
        ))
    });
    case!("download.remote", "下载模块", "真实站点解析（联网）", true, |e: &CaseEnv| {
        // 真实站点元数据抓取：验证 extractor 与 JSON 解析链路
        const CANDIDATES: [&str; 2] = [
            "https://www.youtube.com/watch?v=aqz-KE-bpKQ",
            "https://archive.org/details/BigBuckBunny_124",
        ];
        let mut last_err = String::new();
        let mut incomplete: Vec<String> = Vec::new();
        for url in CANDIDATES {
            match downloader::probe(&e.ctx, url, false) {
                Ok(info) => {
                    if info.title.trim().is_empty() || info.formats.is_empty() {
                        // 不中断：继续尝试下一个候选站点
                        incomplete.push(format!(
                            "{} → title={:?} formats={}（可能需要登录 / Cookies）",
                            info.extractor.clone().unwrap_or_else(|| url.to_string()),
                            info.title,
                            info.formats.len()
                        ));
                        continue;
                    }
                    let with_video = info.formats.iter().filter(|f| f.has_video).count();
                    if with_video == 0 {
                        incomplete.push(format!("{} → 未解析到视频格式", url));
                        continue;
                    }
                    return ok(format!(
                        "{} · {} 种格式（{} 含视频）· {:.0}s · 封面{} · 字幕 {} 种",
                        info.extractor.clone().unwrap_or_else(|| "?".into()),
                        info.formats.len(),
                        with_video,
                        info.duration.unwrap_or(0.0),
                        if info.thumbnail.is_some() { "有" } else { "无" },
                        info.subtitle_tracks.len()
                    ));
                }
                Err(msg) => last_err = msg.to_string(),
            }
        }
        if !incomplete.is_empty() {
            return Err(format!("解析结果不完整：{}", incomplete.join(" ｜ ")));
        }
        // 网络受限（如被墙/离线）时判为跳过而非失败
        if last_err.to_lowercase().contains("unable to download")
            || last_err.to_lowercase().contains("timed out")
            || last_err.to_lowercase().contains("timeout")
            || last_err.to_lowercase().contains("failed to resolve")
            || last_err.to_lowercase().contains("connection")
            || last_err.to_lowercase().contains("network")
        {
            return skip(format!("网络不可达，已跳过：{}", last_err.chars().take(80).collect::<String>()));
        }
        Err(format!("真实站点解析失败：{}", last_err.chars().take(120).collect::<String>()))
    });
    case!("download.remote_file", "下载模块", "真实站点端到端下载（联网）", true, |e: &CaseEnv| {
        // 真实站点「解析 → 选流 → 下载 → 提取」整链路验证（不只是元数据）
        const CANDIDATES: [(&str, &str); 2] = [
            ("https://www.youtube.com/watch?v=aqz-KE-bpKQ", "YouTube Big Buck Bunny（60fps）"),
            ("https://archive.org/details/BigBuckBunny_124", "archive.org Big Buck Bunny"),
        ];
        let mut last_err = String::new();
        for (i, (url, label)) in CANDIDATES.iter().enumerate() {
            let out = e.p(&format!("dl_remote_{i}"));
            std::fs::create_dir_all(&out).map_err(|x| x.to_string())?;
            let req = DownloadRequest {
                url: url.to_string(),
                format_id: None,
                mode: "audio".into(),
                output_dir: Some(out.to_string_lossy().to_string()),
                audio_format: Some("mp3".into()),
                merge_container: Some("mp4".into()),
                embed_thumbnail: false,
                embed_subs: false,
                filename_template: None,
                playlist: false,
                rate_limit: None,
                ..Default::default()
            };
            let outcome = match downloader::run_download(&e.ctx, &req, &out, |_| {}, |_| {}, || false) {
                Ok(o) => o,
                Err(msg) => {
                    last_err = msg.to_string();
                    continue;
                }
            };
            if outcome.exit_code != 0 {
                last_err = outcome
                    .stderr_tail
                    .lines()
                    .rev()
                    .find(|l| l.to_uppercase().contains("ERROR"))
                    .or_else(|| outcome.stderr_tail.lines().last())
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(160)
                    .collect();
                continue;
            }
            let f = find_media(&out).ok_or_else(|| format!("{label} 未生成下载文件"))?;
            let sz = size_of(&f);
            if sz == 0 {
                return Err(format!("{label} 下载文件为空"));
            }
            return ok(format!(
                "{label} · {} · 文件 {}",
                tools::human_size(sz),
                f.file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default()
            ));
        }
        let low = last_err.to_lowercase();
        if low.contains("unable to download")
            || low.contains("timed out")
            || low.contains("timeout")
            || low.contains("failed to resolve")
            || low.contains("connection")
            || low.contains("network")
        {
            return skip(format!("网络不可达，已跳过：{}", last_err.chars().take(80).collect::<String>()));
        }
        Err(format!("真实站点下载失败：{}", last_err.chars().take(160).collect::<String>()))
    });
    case!("download.cover_from_file", "下载模块", "已下载视频的封面生成与缓存", false, |e: &CaseEnv| {
        let src = e.p("test_src.mp4");
        if !src.is_file() {
            make_video(&e.ctx, &src, 320, 180, 4.0, 26, true).map_err(|x| x.to_string())?;
        }
        let key = format!("covtest{}", std::process::id());
        let _ = std::fs::remove_file(e.ctx.dirs.cache.join("covers").join(format!("{key}.jpg")));
        let t0 = std::time::Instant::now();
        let p1 = downloader::cover_from_file(&e.ctx, &src.to_string_lossy(), &key, Some(6.0))
            .ok_or("未生成封面")?;
        let cold_ms = t0.elapsed().as_millis();
        let s1 = size_of(Path::new(&p1));
        if s1 < 512 {
            return Err(format!("封面文件过小：{s1} 字节"));
        }
        // 第二次应命中缓存：路径一致且几乎不耗时
        let t1 = std::time::Instant::now();
        let p2 = downloader::cover_from_file(&e.ctx, &src.to_string_lossy(), &key, Some(6.0));
        let warm_ms = t1.elapsed().as_millis();
        if p2.as_deref() != Some(p1.as_str()) {
            return Err("二次调用未命中缓存".into());
        }
        if warm_ms > 300 {
            return Err(format!("缓存命中仍耗时 {warm_ms}ms"));
        }
        // 纯音频不应抽帧
        let audio = e.p("fake_audio.mp3");
        std::fs::write(&audio, b"ID3\x03fake").map_err(|x| x.to_string())?;
        if downloader::cover_from_file(&e.ctx, &audio.to_string_lossy(), "covtest_audio", None).is_some()
        {
            return Err("音频文件不应生成封面".into());
        }
        ok(format!(
            "{} · 生成 {}ms · 缓存命中 {}ms · 音频跳过 ✓",
            tools::human_size(s1),
            cold_ms,
            warm_ms
        ))
    });
    case!("download.parse_no_codec", "下载模块", "无编解码字段站点的格式判定", false, |_e: &CaseEnv| {
        // archive.org 等站点只给宽高/分辨率，不给 vcodec / acodec：
        // 旧逻辑把它们全部判成纯音频，导致「解析结果不完整 → 无法下载」
        let j = serde_json::json!({
            "_type": "video", "id": "x", "title": "Big Buck Bunny", "extractor": "archive.org",
            "formats": [
                {"format_id":"0","format":"Ogg Video","ext":"ogv","width":533,"height":300,"resolution":"533x300","filesize":46935223,"url":"https://a/b.ogv"},
                {"format_id":"1","format":"h.264","ext":"mp4","width":640,"height":360,"resolution":"640x360","filesize":61878609,"url":"https://a/b.mp4"}
            ],
            "thumbnail": "https://a/t.jpg"
        });
        let info = downloader::parse_media_info(&j);
        if info.formats.len() != 2 {
            return Err(format!("格式数不符：{}", info.formats.len()));
        }
        let v = info.formats.iter().filter(|f| f.has_video).count();
        if v != 2 {
            return Err(format!("无 vcodec 的格式被误判为纯音频（含视频 {v}/2）"));
        }
        if info.formats.iter().any(|f| f.resolution == "audio only") {
            return Err("分辨率判定错误".into());
        }
        ok(format!("2 种格式 · {v} 含视频 · 分辨率 533x300/640x360 保留 ✓"))
    });
    case!("logo.custom", "外观", "自定义 LOGO：格式校验 / 4MB 上限 / 落盘 / 换格式清理", false, |e: &CaseEnv| {
        use crate::apply_logo_file;
        let data = e.p("branding_data");
        std::fs::create_dir_all(&data).map_err(|x| x.to_string())?;

        // 1) PNG 落盘到 branding/logo.png
        let png = e.p("my_logo.png");
        std::fs::write(&png, PNG_1PX).map_err(|x| x.to_string())?;
        let saved = apply_logo_file(&data, &png)?;
        if !saved.is_file() {
            return Err("LOGO 未落盘".into());
        }
        if saved.file_name().and_then(|n| n.to_str()) != Some("logo.png") {
            return Err(format!("文件名应为 logo.png，实际 {saved:?}"));
        }
        if saved.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) != Some("branding") {
            return Err("应保存到 branding 子目录".into());
        }

        // 2) 换成 SVG：旧文件必须被清理，目录里只留一个 logo.*
        let svg = e.p("my_logo.svg");
        std::fs::write(&svg, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").map_err(|x| x.to_string())?;
        let saved2 = apply_logo_file(&data, &svg)?;
        if !saved2.is_file() {
            return Err("SVG LOGO 未落盘".into());
        }
        let left = std::fs::read_dir(saved2.parent().unwrap())
            .map_err(|x| x.to_string())?
            .filter_map(|x| x.ok())
            .count();
        if left != 1 {
            return Err(format!("换格式后旧 LOGO 未清理，branding 目录里有 {left} 个文件"));
        }

        // 3) 非图片 / 不存在的文件 / 超大文件都必须被拒绝
        let txt = e.p("nope.txt");
        std::fs::write(&txt, b"x").map_err(|x| x.to_string())?;
        if apply_logo_file(&data, &txt).is_ok() {
            return Err("txt 应被拒绝".into());
        }
        if apply_logo_file(&data, &e.p("missing.png")).is_ok() {
            return Err("不存在的文件应被拒绝".into());
        }
        let big = e.p("big.png");
        std::fs::write(&big, vec![0u8; 4 * 1024 * 1024 + 16]).map_err(|x| x.to_string())?;
        let msg = apply_logo_file(&data, &big).err().unwrap_or_default();
        if !msg.contains("过大") {
            return Err(format!("超过 4 MB 应被拒绝，实际：{msg}"));
        }

        let _ = std::fs::remove_dir_all(&data);
        ok(format!("{saved2:?} 落盘 · 换格式旧文件已清理 · txt/缺失/超大均被拒 ✓"))
    });
    case!("download.reveal_target", "下载模块", "「在文件夹中显示」的路径选择与 file_exists 标注", false, |e: &CaseEnv| {
        use crate::{annotate_file_exists, reveal_select_arg, reveal_target};
        // 1) 文件存在 → 定位该文件（Explorer 选中它）
        let f = e.p("reveal_me.txt");
        std::fs::write(&f, b"x").map_err(|x| x.to_string())?;
        if reveal_target(&f).as_deref() != Some(f.as_path()) {
            return Err("文件存在时应直接定位文件".into());
        }
        // 2) 目录 → 打开该目录
        let d = e.p("reveal_dir");
        std::fs::create_dir_all(&d).map_err(|x| x.to_string())?;
        if reveal_target(&d).as_deref() != Some(d.as_path()) {
            return Err("目录应被直接打开".into());
        }
        // 3) 文件已被删除 → None：调用方必须报错。
        //    旧行为是退到父目录：按钮“看着成功”，用户拿到的窗口里却没选中任何产物
        //    —— 「生成的字幕文件不在文件夹里显示」就是这么来的（BUG-20）。
        let gone = d.join("gone.mp4");
        if reveal_target(&gone).is_some() {
            return Err("文件缺失时必须返回 None（不许退到父目录假装成功）".into());
        }
        // 4) 父目录也不存在 → None：调用方必须报错，不能打开无关窗口
        let nowhere = e.p("no_such_dir_umi").join("f.mp4");
        if reveal_target(&nowhere).is_some() {
            return Err("路径整体不存在时应返回 None".into());
        }
        // 5) explorer 的 /select 参数：整串路径加引号、空格 / 中文原样（用 raw_arg 传，别过 cmd）
        let spaced = d.join("示例 视频.detected-to-en.srt");
        let arg = reveal_select_arg(&spaced);
        if arg != format!("/select,\"{}\"", spaced.display()) || !arg.ends_with('"') {
            return Err(format!("/select 参数应整串加引号，实际：{arg}"));
        }
        // 6) file_exists 标注（界面据此隐藏“打开/在文件夹中显示”按钮）
        let mut list = vec![
            DownloadTask {
                id: "a".into(),
                title: "t".into(),
                url: "u".into(),
                file_path: Some(f.to_string_lossy().into_owned()),
                ..Default::default()
            },
            DownloadTask {
                id: "b".into(),
                title: "t".into(),
                url: "u".into(),
                file_path: Some(gone.to_string_lossy().into_owned()),
                ..Default::default()
            },
            DownloadTask {
                id: "c".into(),
                title: "t".into(),
                url: "u".into(),
                file_path: Some("   ".into()),
                ..Default::default()
            },
        ];
        annotate_file_exists(&mut list);
        if !list[0].file_exists || list[1].file_exists || list[2].file_exists {
            return Err(format!(
                "file_exists 标注错误：{:?}",
                list.iter().map(|x| x.file_exists).collect::<Vec<_>>()
            ));
        }
        ok("存在→定位文件 · 目录→打开 · 缺失→报错（不再退父目录假装成功） · /select 参数原样 · file_exists 标注 ✓")
    });
    case!("download.system_proxy", "下载模块", "代理选择策略（可用/死代理/本机）", false, |e: &CaseEnv| {
        // 起一个真实监听的端口当作「可用代理」
        let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|x| x.to_string())?;
        let port = l.local_addr().map_err(|x| x.to_string())?.port();
        let good = format!("http://127.0.0.1:{port}");
        let mut c = e.ctx.clone();
        c.settings.proxy = Some(good.clone());
        let p = downloader::effective_proxy(&c, "https://www.youtube.com/watch?v=x");
        if p.as_deref() != Some(good.as_str()) {
            return Err(format!("可用代理未生效：{p:?}"));
        }
        // 死代理必须被剔除并回退直连
        c.settings.proxy = Some("http://127.0.0.1:59120".into());
        if downloader::effective_proxy(&c, "https://example.com/v").is_some() {
            return Err("死代理未被剔除（应回退直连）".into());
        }
        // 本机地址绝不走代理
        if downloader::effective_proxy(&c, "http://127.0.0.1:8080/a.mp4").is_some() {
            return Err("本机地址不应使用代理".into());
        }
        drop(l);
        let mut c2 = e.ctx.clone();
        c2.settings.proxy = None;
        match downloader::effective_proxy(&c2, "https://example.com/v") {
            Some(s) => ok(format!("可用代理 ✓ 死代理回退 ✓ 本机直连 ✓ 系统代理：{s}")),
            None => ok("可用代理 ✓ 死代理回退 ✓ 本机直连 ✓ 未启用系统代理".to_string()),
        }
    });
    case!("download.proxy_probe", "下载模块", "代理端口探活（死代理自动回退）", false, |_e: &CaseEnv| {
        // 真实监听的端口 → 判为可用
        let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        if !downloader::proxy_alive(&format!("http://127.0.0.1:{port}")) {
            return Err("存活端口被误判为不可用".into());
        }
        drop(l);
        // 未监听端口 → 必须判为不可用，否则会拿死代理去联网（正是「真实站点解析报错」的根因）
        if downloader::proxy_alive("http://127.0.0.1:59118") {
            return Err("死代理端口未被识别".into());
        }
        if downloader::proxy_alive("http://user:pass@127.0.0.1:59119") {
            return Err("带认证信息的代理地址解析异常".into());
        }
        if downloader::proxy_alive("") {
            return Err("空代理应为不可用".into());
        }
        ok(format!("存活端口 {port} ✓ · 死代理识别 ✓ · 认证地址解析 ✓"))
    });
    case!("download.bad_url", "下载模块", "无效链接错误处理", false, |e: &CaseEnv| {
        // 使用未监听的端口，确保请求必然失败
        let bad = "http://127.0.0.1:59117/nope.mp4";
        let out = e.p("dl_bad");
        std::fs::create_dir_all(&out).map_err(|x| x.to_string())?;
        let req = DownloadRequest {
            url: bad.into(),
            format_id: None,
            mode: "best".into(),
            output_dir: Some(out.to_string_lossy().to_string()),
            audio_format: None,
            merge_container: Some("mp4".into()),
            embed_thumbnail: false,
            embed_subs: false,
            filename_template: None,
            playlist: false,
            rate_limit: None,
            ..Default::default()
        };
        // 探测层允许降级为直链信息（与浏览器行为一致），但真正下载必须给出友好错误
        let probed = downloader::probe(&e.ctx, bad, false).is_ok();
        let brief = |s: &str| s.chars().take(60).collect::<String>();
        match downloader::run_download(&e.ctx, &req, &out, |_| {}, |_| {}, || false) {
            Err(msg) => ok(format!("友好报错（探测降级 {probed}）：{}", brief(&msg.to_string()))),
            Ok(o) if o.exit_code != 0 => {
                let tail = o.stderr_tail.lines().last().unwrap_or("").trim().to_string();
                if tail.is_empty() {
                    return Err("下载失败但没有给出错误信息".into());
                }
                ok(format!("友好报错（探测降级 {probed}）：{}", brief(&tail)))
            }
            Ok(o) => Err(format!("不存在的地址竟然下载成功（退出码 {}）", o.exit_code)),
        }
    });
    case!("download.db_roundtrip", "下载模块", "任务持久化（SQLite）", false, |e: &CaseEnv| {
        let mut t = downloader::new_task(
            &DownloadRequest {
                url: "https://example.com/selftest".into(),
                format_id: None,
                mode: "best".into(),
                output_dir: None,
                audio_format: None,
                merge_container: None,
                embed_thumbnail: false,
                embed_subs: false,
                filename_template: None,
                playlist: false,
                rate_limit: None,
                ..Default::default()
            },
            crate::ctx::now_ms(),
        );
        t.title = "自检任务".into();
        t.status = TaskStatus::Downloading;
        t.progress = 33.3;
        e.db.upsert_download(&t, Some("{\"url\":\"x\"}")).map_err(|x| x.to_string())?;
        let back = e.db.get_download(&t.id).map_err(|x| x.to_string())?.ok_or("读取失败")?;
        if back.title != "自检任务" || (back.progress - 33.3).abs() > 0.01 {
            return Err("字段不一致".into());
        }
        if e.db.get_download_request(&t.id).map_err(|x| x.to_string())?.is_none() {
            return Err("请求参数未持久化".into());
        }
        e.db.delete_download(&t.id).map_err(|x| x.to_string())?;
        if e.db.get_download(&t.id).map_err(|x| x.to_string())?.is_some() {
            return Err("删除失败".into());
        }
        ok("写入 / 读取 / 删除 全部正确")
    });

    /* ---------- 字幕模块 ---------- */
    let srt_sample = "1\n00:00:00,000 --> 00:00:02,500\n你好，世界\n\n2\n00:00:02,500 --> 00:00:05,120\nHello world\n第二行\n\n3\n00:00:05,200 --> 00:00:08,000\numi Downloader\n";
    case!("subtitle.product_naming", "字幕模块", "字幕产物路径（基名 + 语言后缀）", false, move |e: &CaseEnv| {
        // BUG-01：whisper 的 -of <base> 是「追加」后缀 → <视频名>.<语言>.srt；
        // 这里用生产链路的 subtitle_out_base 生成基名，确保自检覆盖真实命名
        let video = e.p("speech.mp4");
        let req = SubtitleRequest {
            video_path: video.to_string_lossy().to_string(),
            language: "en".into(),
            model: Some("tiny".into()),
            translate_to: None,
            output_format: "srt".into(),
            output_dir: Some(e.work.to_string_lossy().to_string()),
        };
        let base = subtitle::subtitle_out_base(&e.ctx, &req);
        let want = e.p("speech.en");
        if base != want {
            return Err(format!("产物基名错误：{}（应为 {}）", base.display(), want.display()));
        }
        let srt = subtitle::srt_path_for_base(&base);
        if srt != e.p("speech.en.srt") {
            return Err(format!("字幕路径错误：{}", srt.display()));
        }
        // 曾经的写法（with_extension）会把 .en 替掉 —— 必须永远不相等
        if srt == base.with_extension("srt") {
            return Err("又退回到 with_extension 的错误命名了".into());
        }
        // 落盘后必须能被回退扫描定位到（含 `<基名>.<其它>.srt` 变体）
        let _ = std::fs::remove_file(&srt);
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nhi\n").map_err(|x| x.to_string())?;
        let found = subtitle::resolve_whisper_srt(&base).ok_or("写出的 srt 定位不到")?;
        if found != srt {
            return Err(format!("定位到错误产物：{}", found.display()));
        }
        let _ = std::fs::remove_file(&srt);
        ok(format!("{} → {}", base.file_name().unwrap_or_default().to_string_lossy(), srt.file_name().unwrap_or_default().to_string_lossy()))
    });
    case!("subtitle.srt_parse", "字幕模块", "SRT 解析", false, move |_e: &CaseEnv| {
        let cues = subtitle::parse_srt(srt_sample);
        if cues.len() != 3 {
            return Err(format!("条目数量错误：{}", cues.len()));
        }
        if cues[0].start_ms != 0 || cues[0].end_ms != 2500 {
            return Err(format!("时间轴解析错误：{:?}", cues[0]));
        }
        if cues[1].text != "Hello world\n第二行" {
            return Err(format!("多行文本解析错误：{:?}", cues[1].text));
        }
        if subtitle::parse_srt("").len() != 0 {
            return Err("空文本应返回 0 条".into());
        }
        ok(format!("3 条 · 首条 {}-{}ms", cues[0].start_ms, cues[0].end_ms))
    });
    case!("subtitle.srt_validate", "字幕模块", "SRT 结构校验", false, move |_e: &CaseEnv| {
        let n = subtitle::validate_srt(srt_sample).map_err(|x| format!("合法字幕被误判：{x}"))?;
        if n != 3 {
            return Err("条目数错误".into());
        }
        let bad = "1\n00:00:05,000 --> 00:00:02,000\n倒挂时间轴\n";
        if subtitle::validate_srt(bad).is_ok() {
            return Err("非法字幕未被识别".into());
        }
        if subtitle::validate_srt("no timestamps here").is_ok() {
            return Err("无时间轴内容未被识别".into());
        }
        ok("合法/非法样本判定正确")
    });
    case!("subtitle.export_vtt", "字幕模块", "导出 VTT", false, move |_e: &CaseEnv| {
        let v = subtitle::to_vtt(&subtitle::parse_srt(srt_sample));
        if !v.starts_with("WEBVTT") || !v.contains("00:00:02.500 -->") {
            return Err(format!("VTT 格式异常：{}", &v[..v.len().min(60)]));
        }
        ok(format!("{} 字节，头部与时间轴正确", v.len()))
    });
    case!("subtitle.export_ass", "字幕模块", "导出 ASS", false, move |_e: &CaseEnv| {
        let a = subtitle::to_ass(&subtitle::parse_srt(srt_sample), "umi");
        for need in ["[Script Info]", "[V4+ Styles]", "[Events]", "Dialogue: 0,0:00:00.00,0:00:02.50"] {
            if !a.contains(need) {
                return Err(format!("ASS 缺少段落：{need}"));
            }
        }
        ok(format!("{} 字节，段落齐全", a.len()))
    });
    case!("subtitle.export_txt", "字幕模块", "导出 TXT / JSON", false, move |_e: &CaseEnv| {
        let cues = subtitle::parse_srt(srt_sample);
        let t = subtitle::to_txt(&cues);
        if t.lines().count() != 3 || t.contains("-->") {
            return Err("TXT 输出异常".into());
        }
        let j = subtitle::to_json(&cues);
        let v: serde_json::Value = serde_json::from_str(&j).map_err(|x| x.to_string())?;
        if v.as_array().map(|a| a.len()) != Some(3) {
            return Err("JSON 输出异常".into());
        }
        ok("TXT 3 行 · JSON 3 条")
    });
    case!("subtitle.roundtrip", "字幕模块", "格式往返一致性", false, move |_e: &CaseEnv| {
        let cues = subtitle::parse_srt(srt_sample);
        for fmt in ["srt", "vtt", "ass", "txt", "json"] {
            let text = subtitle::render(srt_sample, fmt, "umi");
            if text.trim().is_empty() {
                return Err(format!("{fmt} 输出为空"));
            }
        }
        let again = subtitle::to_srt(&subtitle::parse_srt(&subtitle::to_srt(&cues)));
        if subtitle::parse_srt(&again).len() != cues.len() {
            return Err("SRT 往返后条目数变化".into());
        }
        ok("srt/vtt/ass/txt/json 全部可渲染")
    });
    case!("subtitle.tts", "字幕模块", "生成测试语音（系统 TTS）", true, |e: &CaseEnv| {
        let wav = e.p("speech.wav");
        let _ = std::fs::remove_file(&wav);
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $s.Rate = -1; $s.SetOutputToWaveFile('{}'); \
             $s.Speak('Hello, this is umi downloader self test. Download, convert and generate subtitles.'); \
             $s.Dispose();",
            wav.to_string_lossy()
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|x| format!("调用系统 TTS 失败：{x}"))?;
        if !out.status.success() || !wav.is_file() || size_of(&wav) < 10_000 {
            return Err(format!(
                "生成语音失败：{}",
                crate::ctx::decode_output(&out.stderr).lines().last().unwrap_or("").to_string()
            ));
        }
        let info = converter::probe_media(&e.ctx, &wav.to_string_lossy()).map_err(|x| x.to_string())?;
        let d = info.duration.unwrap_or(0.0);
        if d < 2.0 {
            return Err(format!("语音时长过短：{d:.2}s"));
        }
        // 顺带合成一个带语音的视频，用于「从视频提取字幕」链路
        let mp4 = e.p("speech.mp4");
        make_video(&e.ctx, &e.p("silent.mp4"), 320, 240, d, 30, false)?;
        let ffmpeg = need_tool(&e.ctx, "ffmpeg")?;
        let (code, _, err) = run_proc(
            &ffmpeg,
            &[
                "-hide_banner".into(),
                "-loglevel".into(),
                "error".into(),
                "-y".into(),
                "-i".into(),
                e.p("silent.mp4").to_string_lossy().to_string(),
                "-i".into(),
                wav.to_string_lossy().to_string(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "ultrafast".into(),
                "-crf".into(),
                "30".into(),
                "-c:a".into(),
                "aac".into(),
                "-shortest".into(),
                mp4.to_string_lossy().to_string(),
            ],
        )?;
        if code != 0 || !mp4.is_file() {
            return Err(format!("合成语音视频失败：{}", err.lines().last().unwrap_or("")));
        }
        ok(format!("语音 {:.1}s · {}", d, tools::human_size(size_of(&wav))))
    });
    case!("subtitle.extract_audio", "字幕模块", "从视频提取识别用音频", true, |e: &CaseEnv| {
        let video = e.p("speech.mp4");
        if !video.is_file() {
            return Ok(("".into(), Some("缺少测试视频（TTS 用例未通过）".into())));
        }
        let wav = e.p("extracted.wav");
        let _ = std::fs::remove_file(&wav);
        let out = subtitle::extract_audio(&e.ctx, &video.to_string_lossy(), &wav, |_| {}, || false).map_err(|x| x.to_string())?;
        if out.exit_code != 0 || !wav.is_file() {
            return Err(format!("提取失败：{}", out.stderr_tail.lines().last().unwrap_or("")));
        }
        let info = converter::probe_media(&e.ctx, &wav.to_string_lossy()).map_err(|x| x.to_string())?;
        if info.audio_codec.as_deref() != Some("pcm_s16le") {
            return Err(format!("编码错误：{:?}", info.audio_codec));
        }
        if info.video_codec.is_some() {
            return Err("音频文件不应含视频流".into());
        }
        ok(format!("{:.1}s PCM {}", info.duration.unwrap_or(0.0), tools::human_size(size_of(&wav))))
    });
    case!("subtitle.whisper", "字幕模块", "Whisper AI 转写（真实识别）", true, |e: &CaseEnv| {
        let c = e.tiny_ctx();
        let wav = e.p("extracted.wav");
        if !wav.is_file() {
            let w = e.p("speech.wav");
            if w.is_file() {
                let base = subtitle::subtitle_out_base(
                    &c,
                    &SubtitleRequest {
                        video_path: w.to_string_lossy().to_string(),
                        language: "en".into(),
                        model: Some("tiny".into()),
                        translate_to: None,
                        output_format: "srt".into(),
                        output_dir: Some(e.work.to_string_lossy().to_string()),
                    },
                );
                let outcome = subtitle::transcribe_to_format(
                    &c,
                    &SubtitleRequest {
                        video_path: w.to_string_lossy().to_string(),
                        language: "en".into(),
                        model: Some("tiny".into()),
                        translate_to: None,
                        output_format: "srt".into(),
                        output_dir: Some(e.work.to_string_lossy().to_string()),
                    },
                    &base,
                    &w,
                    |_| {},
                    |_| {},
                    || false,
                );
                return match outcome {
                    Ok(p) => {
                        let text = std::fs::read_to_string(&p).unwrap_or_default();
                        let n = subtitle::validate_srt(&text)?;
                        ok(format!("{n} 条字幕 · {}", first_words(&text)))
                    }
                    Err(x) => Err(x.to_string()),
                };
            }
            return Err("缺少可识别音频".into());
        }
        // 基名走生产链路（<视频名>.<语言>）：以前用无语言后缀的 whisper_out，
        // 正好绕开了 BUG-01 的路径命名问题，自检才会「假通过」
        let req = SubtitleRequest {
            video_path: e.p("speech.mp4").to_string_lossy().to_string(),
            language: "en".into(),
            model: Some("tiny".into()),
            translate_to: None,
            output_format: "srt".into(),
            output_dir: Some(e.work.to_string_lossy().to_string()),
        };
        let base = subtitle::subtitle_out_base(&c, &req);
        let expect = subtitle::srt_path_for_base(&base);
        let progress = Arc::new(AtomicUsize::new(0));
        let p2 = progress.clone();
        let path = subtitle::transcribe_to_format(
            &c,
            &req,
            &base,
            &wav,
            move |_| {
                p2.fetch_add(1, Ordering::SeqCst);
            },
            |_| {},
            || false,
        )
        .map_err(|x| x.to_string())?;
        if path != expect {
            return Err(format!(
                "产物路径不是 <基名>.srt：{}（应为 {}）",
                path.display(),
                expect.display()
            ));
        }
        if !path.is_file() {
            return Err(format!("产物不存在：{}", path.display()));
        }
        let text = std::fs::read_to_string(&path).map_err(|x| x.to_string())?;
        let n = subtitle::validate_srt(&text)?;
        let lower = text.to_lowercase();
        let hits = ["umi", "download", "self", "test", "hello", "convert", "subtitle"]
            .iter()
            .filter(|k| lower.contains(**k))
            .count();
        if hits < 2 {
            return Err(format!("识别内容与原文不符（命中 {hits}/7）：{}", first_words(&text)));
        }
        ok(format!("{n} 条 · 关键词命中 {hits}/7 · {}", first_words(&text)))
    });
    case!("subtitle.export_real", "字幕模块", "转写结果导出多种格式", true, |e: &CaseEnv| {
        // 与 whisper 用例同一套命名：<work>/speech.en.srt
        let req = SubtitleRequest {
            video_path: e.p("speech.mp4").to_string_lossy().to_string(),
            language: "en".into(),
            model: Some("tiny".into()),
            translate_to: None,
            output_format: "srt".into(),
            output_dir: Some(e.work.to_string_lossy().to_string()),
        };
        let srt_file = subtitle::srt_path_for_base(&subtitle::subtitle_out_base(&e.ctx, &req));
        if !srt_file.is_file() {
            return Ok(("".into(), Some("缺少转写结果（Whisper 用例未通过）".into())));
        }
        let raw = std::fs::read_to_string(&srt_file).map_err(|x| x.to_string())?;
        let mut made = Vec::new();
        for fmt in ["vtt", "ass", "txt", "json"] {
            let dest = e.p(&format!("export_test.{fmt}"));
            std::fs::write(&dest, subtitle::render(&raw, fmt, "umi")).map_err(|x| x.to_string())?;
            if size_of(&dest) < 10 {
                return Err(format!("{fmt} 导出为空"));
            }
            made.push(fmt);
        }
        // 用 ffmpeg 反向校验 VTT / ASS 语法合法：转回 SRT 再检查内容
        let ffmpeg = need_tool(&e.ctx, "ffmpeg")?;
        for fmt in ["vtt", "ass"] {
            let f = e.p(&format!("export_test.{fmt}"));
            let back = e.p(&format!("back_check_{fmt}.srt"));
            let _ = std::fs::remove_file(&back);
            let (code, _, err) = run_proc(
                &ffmpeg,
                &[
                    "-hide_banner".into(),
                    "-v".into(),
                    "error".into(),
                    "-y".into(),
                    "-i".into(),
                    f.to_string_lossy().to_string(),
                    back.to_string_lossy().to_string(),
                ],
            )?;
            if code != 0 {
                return Err(format!("ffmpeg 校验 {fmt} 失败：{}", err.lines().last().unwrap_or("")));
            }
            let back_txt = std::fs::read_to_string(&back).map_err(|x| x.to_string())?;
            if !back_txt.contains("-->") {
                return Err(format!("{fmt} 经 ffmpeg 转回 SRT 后时间轴丢失"));
            }
        }
        ok(format!("{} 导出且通过 ffmpeg 语法校验", made.join(" / ")))
    });
    case!("subtitle.cleanup", "字幕模块", "临时文件清理", false, |e: &CaseEnv| {
        let tmp = e.ctx.dirs.cache.join("subtitle-work");
        let leftovers = std::fs::read_dir(&tmp)
            .map(|rd| rd.flatten().filter(|f| f.path().extension().map(|x| x == "wav").unwrap_or(false)).count())
            .unwrap_or(0);
        if leftovers > 0 {
            return Err(format!("残留 {leftovers} 个临时音频文件"));
        }
        ok("无临时文件残留")
    });

    /* ---------- 数据层 ---------- */
    case!("db.download_crud", "数据层", "下载表 CRUD", false, |e: &CaseEnv| {
        let mut t = DownloadTask { id: "selftest-dl".into(), title: "T".into(), url: "u".into(), ..Default::default() };
        t.created_time = crate::ctx::now_ms();
        e.db.upsert_download(&t, None).map_err(|x| x.to_string())?;
        t.progress = 50.0;
        e.db.upsert_download(&t, None).map_err(|x| x.to_string())?;
        let list = e.db.list_downloads().map_err(|x| x.to_string())?;
        let found = list.iter().find(|x| x.id == "selftest-dl").ok_or("未找到记录")?;
        if (found.progress - 50.0).abs() > 0.01 {
            return Err("更新未生效".into());
        }
        e.db.delete_download("selftest-dl").map_err(|x| x.to_string())?;
        ok(format!("表内共 {} 条记录", list.len()))
    });
    case!("db.convert_crud", "数据层", "转换表 CRUD", false, |e: &CaseEnv| {
        let t = ConvertTask {
            id: "selftest-cv".into(),
            input_file: "a.mp4".into(),
            output_file: "a.mkv".into(),
            format: "mkv".into(),
            codec: Some("libx264".into()),
            status: TaskStatus::Done,
            progress: 100.0,
            error: None,
            created_time: crate::ctx::now_ms(),
        };
        e.db.upsert_convert(&t, Some("{}")).map_err(|x| x.to_string())?;
        let got = e.db.get_convert("selftest-cv").map_err(|x| x.to_string())?.ok_or("未找到")?;
        if got.output_file != "a.mkv" || got.status != TaskStatus::Done {
            return Err("字段不一致".into());
        }
        e.db.delete_convert("selftest-cv").map_err(|x| x.to_string())?;
        ok("写入 / 读取 / 删除 正确")
    });
    case!("db.subtitle_crud", "数据层", "字幕表 CRUD", false, |e: &CaseEnv| {
        let t = SubtitleTask {
            id: "selftest-sub".into(),
            video_path: "v.mp4".into(),
            language: "zh".into(),
            model: "tiny".into(),
            subtitle_path: Some("v.zh.srt".into()),
            translate_to: None,
            status: TaskStatus::Done,
            progress: 100.0,
            error: None,
            created_time: crate::ctx::now_ms(),
        };
        e.db.upsert_subtitle(&t, None).map_err(|x| x.to_string())?;
        let got = e.db.get_subtitle("selftest-sub").map_err(|x| x.to_string())?.ok_or("未找到")?;
        if got.language != "zh" || got.subtitle_path.as_deref() != Some("v.zh.srt") {
            return Err("字段不一致".into());
        }
        e.db.delete_subtitle("selftest-sub").map_err(|x| x.to_string())?;
        ok("写入 / 读取 / 删除 正确")
    });
    case!("db.kv", "数据层", "键值存储", false, |e: &CaseEnv| {
        e.db.kv_set("selftest.key", "值-测试").map_err(|x| x.to_string())?;
        let v = e.db.kv_get("selftest.key").map_err(|x| x.to_string())?;
        if v.as_deref() != Some("值-测试") {
            return Err(format!("读取不一致：{v:?}"));
        }
        e.db.kv_set("selftest.key", "覆盖").map_err(|x| x.to_string())?;
        if e.db.kv_get("selftest.key").map_err(|x| x.to_string())?.as_deref() != Some("覆盖") {
            return Err("覆盖更新失败".into());
        }
        ok("写入 / 覆盖 / 读取 正确（含中文）")
    });
    case!("db.stale_reset", "数据层", "中断任务恢复（含超限排队）", false, |e: &CaseEnv| {
        let base = crate::ctx::now_ms();
        let mut mk = |id: &str, st: TaskStatus, created: i64| -> DownloadTask {
            DownloadTask {
                id: id.into(),
                title: "S".into(),
                url: format!("u/{id}"),
                status: st,
                created_time: created,
                ..Default::default()
            }
        };
        // 3 个「下载中」+ 1 个「排队中」：并发上限设 2 → 最早 2 个转暂停，其余留在队列
        for (i, id) in ["selftest-stale-a", "selftest-stale-b", "selftest-stale-c"].iter().enumerate() {
            let t = mk(id, TaskStatus::Downloading, base + i as i64);
            e.db.upsert_download(&t, None).map_err(|x| x.to_string())?;
        }
        let q = mk("selftest-stale-q", TaskStatus::Pending, base + 10);
        e.db.upsert_download(&q, None).map_err(|x| x.to_string())?;
        e.db.reset_stale_downloads(2).map_err(|x| x.to_string())?;
        let a = e.db.get_download("selftest-stale-a").map_err(|x| x.to_string())?.ok_or("未找到")?;
        let c = e.db.get_download("selftest-stale-c").map_err(|x| x.to_string())?.ok_or("未找到")?;
        let qq = e.db.get_download("selftest-stale-q").map_err(|x| x.to_string())?.ok_or("未找到")?;
        for id in ["selftest-stale-a", "selftest-stale-b", "selftest-stale-c", "selftest-stale-q"] {
            e.db.delete_download(id).map_err(|x| x.to_string())?;
        }
        if a.status != TaskStatus::Paused {
            return Err(format!("并发内的进行中任务未恢复为暂停态：{:?}", a.status));
        }
        if c.status != TaskStatus::Pending {
            return Err(format!("超并发上限的任务未恢复为排队态：{:?}", c.status));
        }
        if qq.status != TaskStatus::Pending {
            return Err(format!("排队中的任务被改动：{:?}", qq.status));
        }
        ok("并发内 → 暂停可续传；超限 → 排队待唤醒")
    });

    /* ---------- 设置 ---------- */
    case!("settings.legacy_upgrade", "设置", "旧版配置升级（缺字段不丢失）", false, |_e: &CaseEnv| {
        // 旧版本配置文件只有部分字段；升级后必须保留旧值、新增字段取默认值
        let legacy = r#"{"download_dir":"D:\\Videos","whisper_model":"small","theme":"light","concurrency":5}"#;
        let s: crate::models::AppSettings =
            serde_json::from_str(legacy).map_err(|e| format!("旧配置解析失败：{e}"))?;
        if s.download_dir != "D:\\Videos"
            || s.whisper_model != "small"
            || s.theme != "light"
            || s.concurrency != 5
        {
            return Err("旧字段未保留".into());
        }
        if s.accent.trim().is_empty() {
            return Err("新增字段未取默认值".into());
        }
        ok(format!(
            "旧字段保留 ✓ 新增字段默认值 ✓（accent={} / 字幕语言={}）",
            s.accent, s.subtitle_language
        ))
    });
    case!("settings.roundtrip", "设置", "配置读写", false, |e: &CaseEnv| {
        let original = crate::settings::load(&e.ctx.dirs);
        let mut s = original.clone();
        s.whisper_model = "tiny".into();
        s.concurrency = 5;
        crate::settings::save(&e.ctx.dirs, &s).map_err(|x| x.to_string())?;
        let back = crate::settings::load(&e.ctx.dirs);
        if back.whisper_model != "tiny" || back.concurrency != 5 {
            return Err("配置未正确落盘".into());
        }
        // 还原用户原配置
        crate::settings::save(&e.ctx.dirs, &original).map_err(|x| x.to_string())?;
        let restored = crate::settings::load(&e.ctx.dirs);
        if restored.concurrency != original.concurrency || restored.whisper_model != original.whisper_model {
            return Err("配置还原失败".into());
        }
        ok(format!(
            "写入 / 读取 / 还原一致（模型 {}, 并发 {}）",
            back.whisper_model, back.concurrency
        ))
    });

    v
}

fn first_words(srt: &str) -> String {
    let cues = subtitle::parse_srt(srt);
    let text: String = cues.iter().take(3).map(|c| c.text.clone()).collect::<Vec<_>>().join(" ");
    let t = text.replace('\n', " ");
    t.chars().take(70).collect()
}

fn convert_with(
    e: &CaseEnv,
    src: &str,
    format: &str,
    req: &ConvertRequest,
) -> Result<(PathBuf, ConvertOutcomeInfo), String> {
    let out_dir = e.p("out");
    std::fs::create_dir_all(&out_dir).map_err(|x| x.to_string())?;
    let output = converter::output_path_for(&e.p(src), &out_dir, format);
    let progress_hits = Arc::new(AtomicUsize::new(0));
    let ph = progress_hits.clone();
    let outcome = converter::run_convert(
        &e.ctx,
        req,
        &output,
        move |_t| {
            ph.fetch_add(1, Ordering::SeqCst);
        },
        |_| {},
        || false,
    )
    .map_err(|x| x.to_string())?;
    if outcome.exit_code != 0 || !output.is_file() || size_of(&output) == 0 {
        return Err(format!(
            "转码失败(code={})：{}",
            outcome.exit_code,
            outcome.stderr_tail.lines().last().unwrap_or("")
        ));
    }
    Ok((
        output,
        ConvertOutcomeInfo { progress_events: progress_hits.load(Ordering::SeqCst) },
    ))
}

struct ConvertOutcomeInfo {
    progress_events: usize,
}

fn convert_and_verify(
    e: &CaseEnv,
    src: &str,
    format: &str,
    extract_audio: bool,
    w: i64,
    h: i64,
    expect_codec: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let req = ConvertRequest {
        input_file: e.p(src).to_string_lossy().to_string(),
        output_dir: Some(e.p("out").to_string_lossy().to_string()),
        format: format.into(),
        video_codec: if extract_audio { None } else { Some(expect_codec.unwrap_or("libx264").to_string()) },
        audio_codec: Some(if format == "webm" { "libopus".into() } else { "aac".into() }),
        bitrate: None,
        resolution: None,
        crf: Some(30),
        extract_audio,
        mute: false,
        ..Default::default()
    };
    let (path, info) = convert_with(e, src, format, &req)?;
    let probe = converter::probe_media(&e.ctx, &path.to_string_lossy()).map_err(|x| x.to_string())?;
    if let Some(c) = expect_codec {
        if probe.video_codec.as_deref() != Some(c) {
            return Err(format!("编码器不符：期望 {c}，实际 {:?}", probe.video_codec));
        }
    }
    if !extract_audio {
        if probe.width != Some(w) || probe.height != Some(h) {
            return Err(format!("分辨率不符：{}x{}", probe.width.unwrap_or(0), probe.height.unwrap_or(0)));
        }
    }
    ok(format!(
        "{} · {} · 进度事件 {}",
        format.to_uppercase(),
        tools::human_size(size_of(&path)),
        info.progress_events
    ))
}

/* ==================== 执行器 ==================== */

    /// 最小合法 PNG（1x1 透明）：自检用来验证自定义 LOGO 的落盘流程
    const PNG_1PX: [u8; 70] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00,
        0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x60, 0x60, 0x60,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0xA5, 0xF6, 0x45, 0x40, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

pub fn run_all(
    ctx: &Ctx,
    db: &Arc<Db>,
    deep: bool,
    cb: Option<Arc<dyn Fn(&TestCase) + Send + Sync>>,
) -> TestReport {
    let work = ctx.dirs.cache.join("selftest");
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::create_dir_all(&work);

    let mut env = CaseEnv { ctx: ctx.clone(), db: db.clone(), work: work.clone() };
    // 自检使用 whisper_model=tiny（体积小、速度快）
    env.ctx.settings.whisper_model = "tiny".into();

    let mut report = TestReport { started_at: crate::ctx::now_ms(), ..Default::default() };

    // 深度模式：确保 tiny 模型就绪
    if deep {
        let model = ctx.dirs.models.join("ggml-tiny.bin");
        if !model.is_file() {
            let print = |msg: &str| println!("[自检准备] {msg}");
            print("下载 Whisper tiny 模型（约 75MB）…");
            match tools::install_whisper_model(&env.ctx, "tiny") {
                Ok(_) => print("模型就绪"),
                Err(e) => print(&format!("模型下载失败：{e}")),
            }
        }
    }

    for c in all_cases() {
        let mut tc = TestCase {
            id: c.id.into(),
            group: c.group.into(),
            name: c.name.into(),
            status: "running".into(),
            detail: None,
            duration_ms: None,
        };
        if c.heavy && !deep {
            tc.status = "skip".into();
            tc.detail = Some("快速模式跳过（使用 --deep 运行完整测试）".into());
            report.cases.push(tc.clone());
            report.total += 1;
            report.skipped += 1;
            if let Some(cb) = &cb {
                cb(&tc);
            }
            println!("[SKIP] {} {}", c.id, c.name);
            continue;
        }
        let t0 = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (c.f)(&env)));
        let ms = t0.elapsed().as_millis() as i64;
        tc.duration_ms = Some(ms);
        match result {
            Ok(Ok((detail, skip_reason))) => {
                if let Some(r) = skip_reason {
                    tc.status = "skip".into();
                    tc.detail = Some(r);
                    report.skipped += 1;
                    println!("[SKIP] {} {} — {}", c.id, c.name, tc.detail.clone().unwrap_or_default());
                } else {
                    tc.status = "pass".into();
                    tc.detail = Some(detail);
                    report.passed += 1;
                    println!("[PASS] {} {} ({ms}ms) {}", c.id, c.name, tc.detail.clone().unwrap_or_default());
                }
            }
            Ok(Err(e)) => {
                tc.status = "fail".into();
                tc.detail = Some(e);
                report.failed += 1;
                println!("[FAIL] {} {} ({ms}ms) — {}", c.id, c.name, tc.detail.clone().unwrap_or_default());
            }
            Err(_) => {
                tc.status = "fail".into();
                tc.detail = Some("用例执行时发生 panic".into());
                report.failed += 1;
                println!("[FAIL] {} {} — 发生 panic", c.id, c.name);
            }
        }
        report.total += 1;
        report.cases.push(tc.clone());
        if let Some(cb) = &cb {
            cb(&tc);
        }
    }

    report.finished_at = crate::ctx::now_ms();
    report
}
