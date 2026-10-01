use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::ctx::{command_for, ensure_dir, kill_tree, LineBuffer, Ctx};
use crate::models::{ConvertRequest, MediaProbe, TaskStatus};

fn res() -> &'static (Regex, Regex, Regex) {
    static R: OnceLock<(Regex, Regex, Regex)> = OnceLock::new();
    R.get_or_init(|| {
        (
            Regex::new(r"out_time=(\d+):(\d+):(\d+)\.(\d+)").unwrap(),
            Regex::new(r"progress=(\w+)").unwrap(),
            Regex::new(r"speed=\s*([\d.]+x|N/A)").unwrap(),
        )
    })
}

/// 输出容器扩展名
///
/// 少数格式的「格式 id」并不是 ffmpeg 认识的容器扩展名：
/// ALAC 没有独立封装器（`.alac` 会让 ffmpeg 报 “Invalid argument”），按苹果惯例写进 M4A。
pub fn output_ext(format: &str) -> String {
    match format.to_lowercase().as_str() {
        "alac" => "m4a".into(),
        other => other.to_string(),
    }
}

/// 依据输入文件名与目标格式推导输出路径
pub fn output_path_for(input: &Path, out_dir: &Path, format: &str) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let ext = output_ext(format);
    let mut candidate = out_dir.join(format!("{stem}.{ext}"));
    if candidate == input {
        candidate = out_dir.join(format!("{stem}_converted.{ext}"));
    }
    // 避免同名覆盖
    if candidate.exists() {
        let mut i = 1;
        loop {
            let c = out_dir.join(format!("{stem} ({i}).{ext}"));
            if !c.exists() {
                return c;
            }
            i += 1;
            if i > 999 {
                return c;
            }
        }
    }
    candidate
}

/// 编解码器别名归一化：hevc/H.265/x265 → 真实编码器名
pub fn normalize_video_codec(vc: &str) -> String {
    match vc.trim().to_lowercase().as_str() {
        "hevc" | "h265" | "h.265" | "x265" => "libx265".into(),
        "h264" | "h.264" | "avc" | "x264" => "libx264".into(),
        "av1" | "svtav1" | "svt-av1" => "libsvtav1".into(),
        "vp9" => "libvpx-vp9".into(),
        "vp8" => "libvpx".into(),
        other => other.to_string(),
    }
}

/// 音频编码器别名归一化
pub fn normalize_audio_codec(ac: &str) -> String {
    match ac.trim().to_lowercase().as_str() {
        "mp3" => "libmp3lame".into(),
        "opus" => "libopus".into(),
        "ogg" | "vorbis" => "libvorbis".into(),
        "wma" => "wmav2".into(),
        "m4a" => "aac".into(),
        other => other.to_string(),
    }
}

/// 目标格式只认一种视频编码器（UI 的编码选择不参与）。
///
/// 实测（本机 ffmpeg 8.0.1 full）：`-c:v libx264` 写 .m2v / .y4m / .dv 全部空文件
/// （mpeg2video / yuv4mpegpipe / dv 封装器直接拒绝 h264），必须锁定容器唯一认的编码器。
const FORCED_VIDEO_CODEC: &[(&str, &str)] = &[
    ("m2v", "mpeg2video"),
    ("y4m", "rawvideo"),
    ("dv", "dvvideo"),
];

/// 目标格式只有唯一可用编码器（UI 的编码选择不参与，含用户选了 copy 的情况）。
const FORCED_AUDIO_CODEC: &[(&str, &str)] = &[
    ("dts", "dca"),
    ("alac", "alac"),
    ("tta", "tta"),
];

/// ffmpeg `-encoders` 里标志位带 X 的编码器（本机 8.0.1 实测全量）。
///
/// 带 X 不等于「写不了」：加上 `-strict experimental` 就能写出真实文件（DTS 的 dca 就是如此），
/// 所以格式可用性不能再把它们排除掉，只在拼参数时补 `-strict experimental`。
pub const EXPERIMENTAL_ENCODERS: &[&str] =
    &["dca", "opus", "vorbis", "mlp", "truehd", "s302m", "avui", "pdv"];

/// 该编码器是否需要 `-strict experimental`
pub fn is_experimental_encoder(name: &str) -> bool {
    EXPERIMENTAL_ENCODERS.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// 变速滤镜链：atempo 单次仅支持 0.5~2.0，超出范围需要串联
pub fn atempo_chain(speed: f64) -> String {
    let s = speed.clamp(0.1, 100.0);
    let mut parts: Vec<String> = Vec::new();
    let mut rest = s;
    while rest > 2.0 {
        parts.push("atempo=2.0".into());
        rest /= 2.0;
    }
    while rest < 0.5 {
        parts.push("atempo=0.5".into());
        rest *= 2.0;
    }
    parts.push(format!("atempo={rest:.4}"));
    parts.join(",")
}

/// 依编码器选择合适的质量参数：-preset 只对 x264/x265/svt-av1 有效，
/// libvpx 系列必须用 -deadline/-cpu-used，
/// 硬件编码器（nvenc/qsv/amf）必须用各自的码控参数，否则 ffmpeg 直接报错退出。
pub fn quality_args(vc: &str, crf: i64) -> Vec<String> {
    match vc {
        "libx264" | "libx265" => vec![
            "-crf".into(),
            crf.to_string(),
            "-preset".into(),
            "medium".into(),
        ],
        "libvpx-vp9" | "libvpx" => vec![
            "-crf".into(),
            crf.to_string(),
            "-b:v".into(),
            "0".into(),
            "-row-mt".into(),
            "1".into(),
            "-deadline".into(),
            "good".into(),
            "-cpu-used".into(),
            "2".into(),
        ],
        "libsvtav1" => vec!["-crf".into(), crf.to_string(), "-preset".into(), "8".into()],
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => vec![
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            crf.to_string(),
            "-preset".into(),
            "p5".into(),
            "-b:v".into(),
            "0".into(),
        ],
        "h264_qsv" | "hevc_qsv" => vec![
            "-global_quality".into(),
            crf.to_string(),
            "-look_ahead".into(),
            "1".into(),
        ],
        "h264_amf" | "hevc_amf" => vec![
            "-rc".into(),
            "cqp".into(),
            "-qp_i".into(),
            crf.to_string(),
            "-qp_p".into(),
            crf.to_string(),
        ],
        "gif" => vec![],
        _ => vec!["-crf".into(), crf.to_string()],
    }
}

/// 自动检测结果
pub struct AutoTarget {
    pub format: String,
    pub video: String,
    pub audio: String,
    pub extract_audio: bool,
    pub reason: String,
}

/// 依据源文件探测结果推荐最佳输出格式（自动检测，与前端 autoDetectTarget 保持一致）：
/// 1) h264/hevc + aac/mp3 → mp4 无损封装（不重编码，最快）
/// 2) vp8/vp9/av1 + opus/vorbis → webm 无损封装
/// 3) 纯音频：无损源保留 flac，opus 直封，其余转 mp3
/// 4) 其它 → mp4 + libx264/aac 重编码
pub fn auto_target(p: &MediaProbe) -> AutoTarget {
    // 文档类输入：输出目标由 docs 决定（xlsx → csv、pptx → md、pdf → txt…），
    // 否则会掉进下面的媒体分支给出 mp4/mp3 这种荒谬结果。
    if let Some(kind) = p.format_name.as_deref().and_then(|f| f.strip_prefix("document:")) {
        let format = crate::docs::auto_document_target(kind);
        return AutoTarget {
            video: "none".into(),
            audio: "none".into(),
            extract_audio: false,
            reason: format!("输入为 {kind} 文档，推荐输出为 {format}（不经 ffmpeg，由文档引擎处理）"),
            format,
        };
    }

    let v = p.video_codec.clone().unwrap_or_default().to_lowercase();
    let a = p.audio_codec.clone().unwrap_or_default().to_lowercase();
    let has_video = !v.is_empty() && v != "none";
    let has_audio = !a.is_empty() && a != "none";

    if !has_video {
        let lossless = ["flac", "alac", "pcm_s16le", "pcm_s24le", "wav"]
            .iter()
            .any(|c| a.contains(c));
        if lossless {
            return AutoTarget {
                format: "flac".into(),
                video: "none".into(),
                audio: "flac".into(),
                extract_audio: true,
                reason: format!("源为无损音频（{}），推荐 FLAC 无损导出", if a.is_empty() { "未知" } else { &a }),
            };
        }
        if a == "opus" {
            return AutoTarget {
                format: "opus".into(),
                video: "none".into(),
                audio: "libopus".into(),
                extract_audio: true,
                reason: "源为 Opus，直接封装为 .opus 无需重编码".into(),
            };
        }
        return AutoTarget {
            format: "mp3".into(),
            video: "none".into(),
            audio: "libmp3lame".into(),
            extract_audio: true,
            reason: format!(
                "源音频编码 {} 兼容性一般，推荐转 MP3（通用性最好）",
                if a.is_empty() { "未知" } else { &a }
            ),
        };
    }

    let mp4_video = ["h264", "avc1", "hevc", "h265"].contains(&v.as_str());
    let mp4_audio = !has_audio || ["aac", "mp3", "ac3", "eac3"].contains(&a.as_str());
    if mp4_video && mp4_audio {
        return AutoTarget {
            format: "mp4".into(),
            video: "copy".into(),
            audio: if has_audio { "copy".into() } else { "none".into() },
            extract_audio: false,
            reason: format!("源为 {v}，可直接无损封装为 MP4（不重编码，速度最快）"),
        };
    }

    let webm_video = ["vp8", "vp9", "av1"].contains(&v.as_str());
    let webm_audio = !has_audio || ["opus", "vorbis"].contains(&a.as_str());
    if webm_video && webm_audio {
        return AutoTarget {
            format: "webm".into(),
            video: "copy".into(),
            audio: if has_audio { "copy".into() } else { "none".into() },
            extract_audio: false,
            reason: format!("源为 {v}，可直接无损封装为 WebM"),
        };
    }

    AutoTarget {
        format: "mp4".into(),
        video: "libx264".into(),
        audio: "aac".into(),
        extract_audio: false,
        reason: format!("源编码 {v} 通用性不足，推荐重编码为 MP4（H.264 + AAC，兼容性最好）"),
    }
}

/// 判断是否图片文件（按扩展名）
///
/// 含 psd / heif：ffmpeg 认不出 `.heic` 容器、对真实 PSD 解码也常失败，
/// 这两类由受管 ImageMagick 引擎接手（见 `needs_magick_engine`）。
pub fn is_image_path(p: &str) -> bool {
    let ext = std::path::Path::new(p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "ico", "avif", "heic", "heif", "gif",
        "psd", "tga", "pnm", "ppm", "pgm", "pbm", "pam", "pcx", "exr", "xbm", "xwd", "dds", "svg",
    ]
    .contains(&ext.as_str())
}

/// 构建 FFmpeg 参数（纯函数，可单测）
pub fn build_ffmpeg_args(req: &ConvertRequest, output: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-y".into()];

    // 硬件解码（可选）
    if req.hwaccel {
        a.push("-hwaccel".into());
        a.push("auto".into());
    }

    a.push("-i".into());
    a.push(req.input_file.clone());
    a.push("-progress".into());
    a.push("pipe:1".into());
    a.push("-nostats".into());

    // ── 图片转换分支（png/jpg/webp/bmp/tiff/ico/pnm…）：走 image2，不套视频编码器 ──
    // 已实测可写：png jpg jpeg webp bmp tif tiff avif gif tga ppm pgm pbm pnm xbm xwd pcx exr
    const IMAGE_OUT: &[&str] = &[
        "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "ico", "avif", "tga", "pnm", "ppm",
        "pgm", "pbm", "pam", "xbm", "xwd", "pcx", "exr",
    ];
    // 扩展名不是 image2 认得的标签 / 封装器不会自选编码器：必须显式指定，否则写出 0 / 4 字节空文件
    const IMAGE_OUT_SPECIAL: &[(&str, &str, Option<&str>)] = &[
        // .pnm 无封装器也非 image2 标签 → `-f image2 -c:v ppm`（裸写 .pnm 直接 Invalid argument）
        ("pnm", "ppm", None),
        // ico 封装器只收 ≤256 的位图，默认编码器是空的 → 必须 -c:v bmp + 缩到 ≤256
        ("ico", "bmp", Some("scale='min(256,iw)':'min(256,ih)'")),
    ];
    let fmt_l = req.format.to_ascii_lowercase();
    if !req.extract_audio && (IMAGE_OUT.contains(&fmt_l.as_str()) || is_image_path(&req.input_file))
    {
        a.push("-frames:v".into());
        a.push("1".into());
        match fmt_l.as_str() {
            "jpg" | "jpeg" => {
                a.push("-q:v".into());
                a.push("2".into());
            }
            "webp" | "avif" => {
                a.push("-q:v".into());
                a.push("80".into());
            }
            _ => {}
        }
        if let Some((_, codec, scale)) = IMAGE_OUT_SPECIAL.iter().find(|(id, _, _)| *id == fmt_l).copied() {
            a.push("-f".into());
            a.push("image2".into());
            a.push("-c:v".into());
            a.push(codec.into());
            if let Some(f) = scale {
                a.push("-vf".into());
                a.push(f.into());
            }
        }
        a.push("-map_metadata".into());
        a.push("-1".into());
        a.push(output.to_string_lossy().to_string());
        return a;
    }

    let speed = req.speed.unwrap_or(1.0);
    let speed_on = (speed - 1.0).abs() > 1e-4 && speed > 0.0;
    let is_gif = req.format.eq_ignore_ascii_case("gif");
    let mut vf: Vec<String> = Vec::new();
    let mut af: Vec<String> = Vec::new();

    if req.extract_audio {
        a.push("-vn".into());
    } else {
        let ui_v = req.video_codec.as_deref().unwrap_or("").trim().to_string();
        // 容器只认一种视频编码器时锁定它（libx264 写 .m2v/.y4m/.dv 会得到空文件）
        let forced_v = FORCED_VIDEO_CODEC
            .iter()
            .find(|(f, _)| f.eq_ignore_ascii_case(&fmt_l))
            .map(|(_, c)| c.to_string());
        if ui_v.eq_ignore_ascii_case("copy") {
            a.push("-c:v".into());
            a.push("copy".into());
        } else if !ui_v.eq_ignore_ascii_case("none") && (!ui_v.is_empty() || forced_v.is_some()) {
            let vc = forced_v.unwrap_or_else(|| normalize_video_codec(&ui_v));
            a.push("-c:v".into());
            a.push(vc.clone());
            if is_experimental_encoder(&vc) {
                a.push("-strict".into());
                a.push("experimental".into());
            }
            if let Some(crf) = req.crf {
                a.extend(quality_args(&vc, crf));
            }
            if let Some(br) = req
                .bitrate
                .as_ref()
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"))
            {
                a.push("-b:v".into());
                a.push(br.clone());
            }
        }
        if let Some(r) = req.resolution.as_ref().filter(|s| !s.is_empty() && *s != "原分辨率") {
            let scale = r.replace(['x', 'X'], ":");
            vf.push(format!(
                "scale={scale}:force_original_aspect_ratio=decrease,pad=ceil(iw/2)*2:ceil(ih/2)*2"
            ));
        }
        if let Some(fps) = req.fps.filter(|f| *f > 0) {
            vf.push(format!("fps={fps}"));
        }
        if speed_on {
            vf.push(format!("setpts={:.6}*PTS", 1.0 / speed));
        }
    }

    if req.mute || is_gif {
        a.push("-an".into());
    } else {
        // 只有唯一编码器的格式（dts→dca / alac→alac / tta→tta）锁定编码器：
        // 用户选了 copy 或别的编码都会写出错容器；dca 还需 -strict experimental
        let forced_a = FORCED_AUDIO_CODEC
            .iter()
            .find(|(f, _)| f.eq_ignore_ascii_case(&fmt_l))
            .map(|(_, c)| c.to_string());
        let mut audio_ready = false;
        if let Some(acn) = forced_a {
            a.push("-c:a".into());
            a.push(acn.clone());
            if is_experimental_encoder(&acn) {
                a.push("-strict".into());
                a.push("experimental".into());
            }
            if let Some(br) = req
                .audio_bitrate
                .as_ref()
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"))
            {
                if !acn.starts_with("pcm") {
                    a.push("-b:a".into());
                    a.push(br.clone());
                }
            }
            audio_ready = true;
        } else if let Some(ac) = req.audio_codec.as_ref().filter(|s| !s.is_empty()) {
            if ac.eq_ignore_ascii_case("copy") && !req.extract_audio {
                a.push("-c:a".into());
                a.push("copy".into());
            } else if !ac.eq_ignore_ascii_case("copy") && !ac.eq_ignore_ascii_case("none") {
                let acn = normalize_audio_codec(ac);
                a.push("-c:a".into());
                a.push(acn.clone());
                if is_experimental_encoder(&acn) {
                    a.push("-strict".into());
                    a.push("experimental".into());
                }
                if let Some(br) = req
                    .audio_bitrate
                    .as_ref()
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"))
                {
                    if !acn.starts_with("pcm") {
                        a.push("-b:a".into());
                        a.push(br.clone());
                    }
                }
            }
            audio_ready = true;
        }
        if audio_ready {
            if let Some(sr) = req.sample_rate.filter(|s| *s > 0) {
                a.push("-ar".into());
                a.push(sr.to_string());
            }
            if let Some(ch) = req.channels.filter(|c| *c > 0) {
                a.push("-ac".into());
                a.push(ch.to_string());
            }
            if speed_on {
                af.push(atempo_chain(speed));
            }
        }
    }

    if !vf.is_empty() {
        a.push("-vf".into());
        a.push(vf.join(","));
    }
    if !af.is_empty() {
        a.push("-af".into());
        a.push(af.join(","));
    }

    // 清除元数据（隐私清理）
    if req.remove_metadata {
        a.push("-map_metadata".into());
        a.push("-1".into());
    }

    if req.faststart {
        match req.format.to_lowercase().as_str() {
            "mp4" | "mov" | "m4a" | "m4v" => {
                a.push("-movflags".into());
                a.push("+faststart".into());
            }
            _ => {}
        }
    }

    a.push(output.to_string_lossy().to_string());
    a
}

/* ==================== ffprobe ==================== */

pub fn probe_media(ctx: &Ctx, path: &str) -> anyhow::Result<MediaProbe> {
    let p = Path::new(path);
    if !p.is_file() {
        anyhow::bail!("文件不存在：{path}");
    }
    // 文档类输入（docx/xlsx/pptx/odt/ods/odp/pdf/md/txt…）不问 ffprobe：
    // ffprobe 只会回 “Invalid data found”，改由 docs 探测并把结果映射成 MediaProbe。
    if crate::docs::is_document_path(path) {
        return document_media_probe(path);
    }
    let ffprobe = ctx.tools.ffprobe()?.to_path_buf();
    let out = command_for(&ffprobe)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        anyhow::bail!("ffprobe 执行失败：{}", crate::ctx::decode_output(&out.stderr).trim());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| anyhow::anyhow!("ffprobe 输出解析失败：{e}"))?;
    Ok(parse_probe(&v, path))
}

pub fn parse_probe(v: &serde_json::Value, path: &str) -> MediaProbe {
    let fmt = v.get("format").cloned().unwrap_or(serde_json::Value::Null);
    let streams = v.get("streams").and_then(|s| s.as_array()).cloned().unwrap_or_default();

    let video = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("video"));
    let audio = streams
        .iter()
        .find(|s| s.get("codec_type").and_then(|c| c.as_str()) == Some("audio"));

    let fps = video.and_then(|v| {
        v.get("r_frame_rate")
            .and_then(|r| r.as_str())
            .and_then(parse_fraction)
    });

    MediaProbe {
        path: path.to_string(),
        format_name: fmt.get("format_name").and_then(|x| x.as_str()).map(|x| x.to_string()),
        duration: fmt
            .get("duration")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| video.and_then(|v| v.get("duration").and_then(|x| x.as_str())).and_then(|s| s.parse::<f64>().ok())),
        size: fmt
            .get("size")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| std::fs::metadata(path).ok().map(|m| m.len() as i64)),
        bit_rate: fmt
            .get("bit_rate")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<i64>().ok()),
        video_codec: video.and_then(|v| v.get("codec_name")).and_then(|x| x.as_str()).map(|x| x.to_string()),
        audio_codec: audio.and_then(|v| v.get("codec_name")).and_then(|x| x.as_str()).map(|x| x.to_string()),
        width: video.and_then(|v| v.get("width")).and_then(|x| x.as_i64()),
        height: video.and_then(|v| v.get("height")).and_then(|x| x.as_i64()),
        fps,
    }
}

/// 文档类输入 → MediaProbe 映射（format_name 形如 `document:docx`）
///
/// 走 docs::probe_document（zip 部件 / PDF 扫描），完全不依赖 ffprobe。
pub fn document_media_probe(path: &str) -> anyhow::Result<MediaProbe> {
    let v = crate::docs::probe_document(path);
    if !v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) {
        anyhow::bail!(
            "{}",
            v.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("文档探测失败")
        );
    }
    let kind = v
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("document")
        .to_string();
    Ok(MediaProbe {
        path: path.to_string(),
        format_name: Some(format!("document:{kind}")),
        size: std::fs::metadata(path).ok().map(|m| m.len() as i64),
        ..Default::default()
    })
}

pub fn parse_fraction(s: &str) -> Option<f64> {
    if let Some((a, b)) = s.split_once('/') {
        let n: f64 = a.parse().ok()?;
        let d: f64 = b.parse().ok()?;
        if d == 0.0 {
            return None;
        }
        Some(n / d)
    } else {
        s.parse().ok()
    }
}

/// 取媒体时长（秒）
pub fn media_duration(ctx: &Ctx, path: &str) -> Option<f64> {
    probe_media(ctx, path).ok().and_then(|p| p.duration)
}

/* ==================== 转换格式目录（按类型分组 + 本机真实能力探测） ==================== */

use std::collections::HashSet;

/// 引擎能力快照：ffmpeg 编码器 / 封装器 / 解封装器 + pandoc / poppler / ImageMagick 是否就绪
///
/// - 线上：`probe_media_caps` 真跑 `ffmpeg -hide_banner -encoders`、`-formats` 与
///   `magick -list format`，解析出真实名单；
/// - 测试：`MediaCaps::from_outputs` 喂一段真实 ffmpeg 输出样本，走同一条解析路径。
#[derive(Debug, Clone, Default)]
pub struct MediaCaps {
    pub ffmpeg: bool,
    pub pandoc: bool,
    pub poppler: bool,
    /// 受管 ImageMagick（bin/imagemagick/magick.exe）：ffmpeg 写不出 PSD 等格式时的图片引擎
    pub magick: bool,
    /// `magick -list format` 的格式名 → 读写标志位（`rw+` / `r--` / `-w+` …，已转大写）
    pub magick_formats: std::collections::HashMap<String, String>,
    pub encoders: HashSet<String>,
    /// 实验性编码器（标志位含 X）：需要 `-strict experimental` 才肯用。
    /// 拼参数时会补上该开关，所以它们同样算「本机写得出」（见 `is_experimental_encoder`）。
    pub experimental: HashSet<String>,
    pub muxers: HashSet<String>,
    pub demuxers: HashSet<String>,
}

impl MediaCaps {
    /// 从 ffmpeg 的两段真实输出构造快照（解析器与线上完全一致）
    pub fn from_outputs(encoders: &str, formats: &str) -> Self {
        let (demuxers, muxers) = parse_format_names(formats);
        Self {
            ffmpeg: true,
            pandoc: false,
            poppler: false,
            magick: false,
            magick_formats: std::collections::HashMap::new(),
            encoders: parse_encoder_names(encoders).into_iter().collect(),
            experimental: parse_experimental_encoders(encoders).into_iter().collect(),
            muxers: muxers.into_iter().collect(),
            demuxers: demuxers.into_iter().collect(),
        }
    }

    /// 追加一段真实的 `magick -list format` 输出（测试用，走线上同一条解析路径）
    pub fn with_magick_output(mut self, out: &str) -> Self {
        let formats = parse_magick_formats(out);
        self.magick = !formats.is_empty();
        self.magick_formats = formats;
        self
    }

    pub fn has_encoder(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.encoders.contains(*n))
    }
    pub fn has_muxer(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.muxers.contains(*n))
    }
    pub fn has_demuxer(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.demuxers.contains(*n))
    }
    /// ImageMagick 能否**写出**该格式（`magick -list format` 的标志位含 w）
    pub fn magick_can_write(&self, name: &str) -> bool {
        self.magick
            && self
                .magick_formats
                .get(&name.to_ascii_uppercase())
                .map(|mode| mode.contains('w'))
                .unwrap_or(false)
    }
    /// ImageMagick 能否**读出**该格式（标志位含 r）
    pub fn magick_can_read(&self, name: &str) -> bool {
        self.magick
            && self
                .magick_formats
                .get(&name.to_ascii_uppercase())
                .map(|mode| mode.contains('r'))
                .unwrap_or(false)
    }
}

/// 解析 `ffmpeg -hide_banner -encoders` 输出的行 → (标志位, 名称)
///
/// 真实输出形如（首列是 6 字符标志位，第二位是编码器名）：
/// ```text
///  V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
///  A....D libmp3lame           libmp3lame MP3 (MPEG audio layer 3) (codec mp3)
/// ```
/// 图例行（` V..... = Video`）、标题行（`Encoders:`）、分隔行（`------`）一律跳过。
fn encoder_lines(out: &str) -> impl Iterator<Item = (&str, &str)> + '_ {
    out.lines().filter_map(|line| {
        let l = line.trim();
        if l.is_empty() || l.contains(" = ") {
            return None;
        }
        let mut it = l.split_whitespace();
        let (flags, name) = (it.next()?, it.next()?);
        if !is_encoder_flags(flags) || name.contains('=') {
            return None;
        }
        Some((flags, name))
    })
}

/// 解析 `-encoders` 输出 → 编码器名（小写、V/A/S 三类，含实验性）
pub fn parse_encoder_names(out: &str) -> Vec<String> {
    encoder_lines(out).map(|(_, n)| n.to_ascii_lowercase()).collect()
}

/// 解析 `-encoders` 输出 → 实验性编码器名（标志位带 `X`：`A..X.D dca` / `A..X.D opus`）。
///
/// 这类编码器 ffmpeg 默认拒绝使用，必须显式 `-strict experimental`；本应用不传该参数，
/// 所以它们不能作为「本机可写该格式」的依据（例如 DTS 只剩 dca 一个编码器 → 置灰）。
pub fn parse_experimental_encoders(out: &str) -> Vec<String> {
    encoder_lines(out)
        .filter(|(flags, _)| flags.contains('X'))
        .map(|(_, n)| n.to_ascii_lowercase())
        .collect()
}

/// 是否是编码器标志位：`V....D` / `VF...D` / `A..X.D` / `S.....` …
fn is_encoder_flags(s: &str) -> bool {
    let mut ch = s.chars();
    match ch.next() {
        Some('V') | Some('A') | Some('S') => {}
        _ => return false,
    }
    let rest: Vec<char> = ch.collect();
    rest.len() == 5 && rest.iter().all(|c| matches!(c, '.' | 'F' | 'S' | 'X' | 'B' | 'D'))
}

/// 解析 `magick -list format` 输出 → 格式名（大写）→ 读写标志位
///
/// 真实输出形如（真机 ImageMagick 7.1.2-31 portable Q16-HDRI x64）：
/// ```text
///    Format  Mode  Description
/// -------------------------------------------------------------------------------
///       AVIF  rw+   AV1 Image File Format (1.23.2)
///        BMP* rw-   Microsoft Windows bitmap image
///       HEIC  r--   High Efficiency Image Format (1.23.2)
///       PSD* rw+   Adobe Photoshop bitmap
/// ```
/// 格式名可能带 `*`（原生支持）/ `@`（需外部委托）后缀；Mode 里的 `w` 才代表写得出来。
pub fn parse_magick_formats(out: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('-') {
            continue;
        }
        let mut it = l.split_whitespace();
        let (Some(name), Some(mode)) = (it.next(), it.next()) else { continue };
        let mode_l = mode.to_ascii_lowercase();
        // 图例行（`Format Mode Description`）与说明文字一律跳过
        if name.eq_ignore_ascii_case("format")
            || !mode_l.chars().all(|c| matches!(c, 'r' | 'w' | '+' | '-' | 'c'))
            || !(mode_l.contains('r') || mode_l.contains('w'))
        {
            continue;
        }
        let n = name.trim_end_matches(['*', '@']).to_ascii_uppercase();
        if n.is_empty() {
            continue;
        }
        map.insert(n, mode_l);
    }
    map
}

/// 解析 `ffmpeg -hide_banner -formats`（或 `-muxers`）输出 → (解封装器, 封装器)
///
/// 真实输出形如（首列标志位：D = 可解封装，E = 可封装；第二列是逗号分隔的格式名）：
/// ```text
///  D   mov,mp4,m4a,3gp,3g2,mj2 QuickTime / MOV
///  DE  matroska,webm   Matroska / WebM
///   E  mp4             MP4 (MPEG-4 Part 14)
/// ```
pub fn parse_format_names(out: &str) -> (Vec<String>, Vec<String>) {
    let mut demuxers: Vec<String> = Vec::new();
    let mut muxers: Vec<String> = Vec::new();
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let mut it = l.split_whitespace();
        let (Some(flags), Some(names)) = (it.next(), it.next()) else { continue };
        if flags.is_empty() || !flags.chars().all(|c| matches!(c, '.' | 'D' | 'E' | 'd')) {
            continue; // 标题行 / 分隔行 / 图例行
        }
        if names.starts_with('=') {
            continue; // 『 D.. = Demuxing supported』这类图例
        }
        for n in names.split(',') {
            let n = n.trim().to_ascii_lowercase();
            if n.is_empty() {
                continue;
            }
            if flags.contains('D') && !demuxers.contains(&n) {
                demuxers.push(n.clone());
            }
            if flags.contains('E') && !muxers.contains(&n) {
                muxers.push(n);
            }
        }
    }
    (demuxers, muxers)
}

/// 单个媒体目标格式的能力需求：封装器 / 编码器任一命中即视为可用
struct FmtSpec {
    id: &'static str,
    label: &'static str,
    note: &'static str,
    /// 可接受的封装器（任一命中即可）
    muxers: &'static [&'static str],
    /// 可接受的编码器（任一命中即可；空 = 不要求）
    encoders: &'static [&'static str],
    /// 作为输入源时可接受的解封装器（任一命中即可）
    demuxers: &'static [&'static str],
}

const fn f(
    id: &'static str,
    label: &'static str,
    note: &'static str,
    muxers: &'static [&'static str],
    encoders: &'static [&'static str],
    demuxers: &'static [&'static str],
) -> FmtSpec {
    FmtSpec { id, label, note, muxers, encoders, demuxers }
}

/// 视频目标格式（22 种）
const VIDEO_FORMATS: &[FmtSpec] = &[
    f("mp4", "MP4", "H.264 / AAC，兼容性最好", &["mp4", "ipod", "mov"], &["libx264", "libopenh264", "mpeg4", "h264_nvenc", "h264_qsv", "h264_amf", "libx265", "hevc_nvenc"], &["mov", "mp4", "m4v"]),
    f("mkv", "MKV", "Matroska，几乎任何编码都能装", &["matroska", "webm"], &["libx264", "libx265", "mpeg4", "libvpx-vp9", "ffv1", "av1_nvenc"], &["matroska", "webm"]),
    f("mov", "MOV", "QuickTime，剪辑软件友好", &["mov", "mp4", "ipod"], &["libx264", "mpeg4", "dvvideo", "prores_ks", "libx265"], &["mov"]),
    f("avi", "AVI", "老设备 / 老播放器", &["avi"], &["mpeg4", "libxvid", "mjpeg", "libx264", "ffv1", "rawvideo"], &["avi"]),
    f("webm", "WebM", "VP9 / AV1 + Opus，网页标准", &["webm", "matroska"], &["libvpx-vp9", "libvpx", "libsvtav1", "libaom-av1", "av1_nvenc"], &["webm", "matroska"]),
    f("flv", "FLV", "直播流 / 老网页播放器", &["flv"], &["flv", "libx264", "h263", "mpeg4", "h264_nvenc"], &["flv", "live_flv"]),
    f("wmv", "WMV", "Windows Media 视频", &["asf"], &["wmv2", "wmv1", "msmpeg4", "libx264", "mpeg4"], &["asf", "asf_o"]),
    f("m4v", "M4V", "苹果 MPEG-4 视频", &["m4v", "mp4", "mov", "ipod"], &["libx264", "mpeg4"], &["m4v", "mp4", "mov"]),
    f("mpg", "MPG", "MPEG-1/2 节目流", &["mpeg"], &["mpeg2video", "mpeg1video"], &["mpeg", "mpegvideo"]),
    f("mpeg", "MPEG", "MPEG 节目流（.mpeg 写法）", &["mpeg"], &["mpeg2video", "mpeg1video"], &["mpeg", "mpegvideo"]),
    f("ts", "TS", "MPEG-TS 传输流（录制 / 电视）", &["mpegts"], &["libx264", "mpeg2video", "h264_nvenc", "libx265"], &["mpegts"]),
    f("m2ts", "M2TS", "蓝光 M2TS 传输流", &["mpegts"], &["libx264", "mpeg2video", "h264_nvenc", "libx265"], &["mpegts", "mpegtsraw"]),
    f("3gp", "3GP", "手机 / 老设备", &["3gp", "3g2", "mp4"], &["libx264", "mpeg4", "h263"], &["3gp", "3g2", "mov"]),
    f("ogv", "OGV", "Ogg Theora / Vorbis", &["ogv", "ogg"], &["libtheora", "libvpx", "libvpx-vp9", "libsvtav1"], &["ogg"]),
    f("mxf", "MXF", "广播 / 专业摄像机：MXF 封装要求严格（mpeg2video / dnxhd + 固定分辨率帧率）", &["mxf"], &["mpeg2video", "dnxhd", "dvvideo", "libx264"], &["mxf"]),
    f("vob", "VOB", "DVD 视频对象", &["vob", "mpeg", "dvd"], &["mpeg2video"], &["mpeg"]),
    f("asf", "ASF", "ASF 容器", &["asf"], &["wmv2", "mpeg4", "msmpeg4"], &["asf", "asf_o"]),
    f("m2v", "M2V", "纯视频基本流（无音频）", &["mpeg2video", "mpeg1video", "rawvideo", "image2"], &["mpeg2video", "mpeg1video"], &["mpegvideo"]),
    f("gif", "GIF", "动图 / 表情包", &["gif"], &["gif"], &["gif", "image2"]),
    f("f4v", "F4V", "Flash MP4（H.264）", &["f4v", "mp4", "mov"], &["libx264", "mpeg4"], &["mov", "mp4", "m4v"]),
    f("dv", "DV", "DV 码流：仅支持 720×576 / 720×480 + dvvideo 编码（选项里设分辨率）", &["dv"], &["dvvideo"], &["dv"]),
    f("y4m", "Y4M", "YUV4MPEG 无损测试序列（体积大）", &["yuv4mpegpipe"], &["rawvideo"], &["yuv4mpegpipe"]),
];

/// 音频目标格式（21 种）
const AUDIO_FORMATS: &[FmtSpec] = &[
    f("mp3", "MP3", "通用性最好，320kbps 以内", &["mp3"], &["libmp3lame", "libshine", "libtwolame"], &["mp3"]),
    f("aac", "AAC", "AAC 音频（ADTS / M4A）", &["adts", "mp4", "ipod", "matroska"], &["aac", "libfdk_aac"], &["aac", "loas"]),
    f("flac", "FLAC", "无损压缩，体积约为 WAV 一半", &["flac", "matroska", "ogg"], &["flac"], &["flac"]),
    f("wav", "WAV", "PCM 无损，体积大", &["wav", "w64"], &["pcm_s16le", "pcm_s24le", "pcm_s32le", "pcm_f32le", "pcm_u8"], &["wav", "w64"]),
    f("opus", "OPUS", "语音 / 网页首选，低码率最强", &["opus", "ogg", "matroska"], &["libopus", "opus"], &["ogg", "matroska", "webm"]),
    f("m4a", "M4A", "AAC / ALAC 封装（苹果生态）", &["ipod", "mp4", "mov"], &["aac", "alac", "libmp3lame"], &["m4a", "mov", "mp4"]),
    f("ogg", "OGG", "Vorbis / Opus 容器", &["ogg"], &["libvorbis", "libopus", "flac"], &["ogg"]),
    f("wma", "WMA", "Windows Media 音频", &["asf"], &["wmav2", "wmav1"], &["asf"]),
    f("alac", "ALAC", "ALAC 无损：ffmpeg 无 .alac 封装器，按苹果惯例写进 M4A 容器", &["ipod", "mp4", "mov", "caf"], &["alac"], &["caf", "mov", "m4a", "mp4"]),
    f("aiff", "AIFF", "苹果 PCM", &["aiff", "caf"], &["pcm_s16be", "pcm_s24be", "pcm_s16le"], &["aiff"]),
    f("ac3", "AC3", "杜比数字 5.1", &["ac3", "matroska", "mpegts"], &["ac3", "ac3_fixed"], &["ac3"]),
    f("eac3", "EAC3", "增强型 AC-3（杜比数字+）", &["eac3", "matroska", "mpegts"], &["eac3"], &["eac3"]),
    f("dts", "DTS", "DTS 音频：由实验性编码器 dca 写出，应用会自动补 -strict experimental", &["dts", "matroska", "mpegts"], &["dca"], &["dts", "dtshd"]),
    f("amr", "AMR", "手机录音：需 8kHz 单声道（选项里把采样率设成 8000、声道 1）", &["amr"], &["libopencore_amrnb", "libvo_amrnbenc"], &["amr", "amrnb"]),
    f("ape", "APE", "Monkey's Audio：ffmpeg 只能解码，官方无便携引擎可引入 → 无法写出", &["ape"], &[], &["ape"]),
    f("mka", "MKA", "Matroska 音频（可多轨）", &["matroska", "webm"], &["flac", "libopus", "libvorbis", "aac", "libmp3lame"], &["matroska", "webm"]),
    f("mp2", "MP2", "MPEG-1 Layer II（广播 / 电视）", &["mp2", "mpegts", "matroska"], &["mp2", "mp2fixed", "libtwolame"], &["mp3", "mpegts"]),
    f("m4b", "M4B", "有声书（带章节标记）", &["ipod", "mp4", "mov"], &["aac", "alac"], &["m4a", "mov", "mp4"]),
    f("caf", "CAF", "苹果 Core Audio 格式", &["caf"], &["alac", "aac", "pcm_s16le", "libmp3lame"], &["caf"]),
    f("au", "AU", "Sun AU（老 Unix 音频）", &["au"], &["pcm_s16be", "pcm_mulaw", "pcm_alaw"], &["au"]),
    f("tta", "TTA", "无损（TTA）", &["tta"], &["tta"], &["tta"]),
];

/// 图片目标格式（22 种）
const IMAGE_FORMATS: &[FmtSpec] = &[
    f("png", "PNG", "无损、支持透明（image2）", &["image2", "apng", "png"], &["png", "apng"], &["image2", "png_pipe"]),
    f("jpg", "JPG", "JPEG，照片默认", &["image2", "mjpeg"], &["mjpeg", "ljpeg"], &["image2", "jpeg_pipe"]),
    f("jpeg", "JPEG", "JPEG（.jpeg 写法）", &["image2", "mjpeg"], &["mjpeg", "ljpeg"], &["image2", "jpeg_pipe"]),
    f("webp", "WEBP", "网页图片，压缩率高", &["webp", "image2"], &["libwebp", "libwebp_anim"], &["image2", "webp_pipe"]),
    f("bmp", "BMP", "无压缩位图", &["image2", "bmp"], &["bmp"], &["image2", "bmp_pipe"]),
    f("tiff", "TIFF", "印刷 / 扫描", &["image2", "tiff"], &["tiff"], &["image2", "tiff_pipe"]),
    f("ico", "ICO", "图标（多尺寸，≤256px）", &["ico"], &["bmp", "png"], &["image2", "ico"]),
    f("avif", "AVIF", "AV1 图片（需 AV1 编码器）", &["avif", "image2"], &["libaom-av1", "libsvtav1", "librav1e", "av1_nvenc"], &["image2", "mov", "mp4"]),
    f("gif", "GIF", "动图", &["gif"], &["gif"], &["image2", "gif"]),
    f("tga", "TGA", "Targa（游戏贴图）", &["image2", "targa"], &["targa"], &["image2"]),
    f("ppm", "PPM", "PNM 家族：彩色", &["image2"], &["ppm", "pam"], &["image2", "ppm_pipe"]),
    f("pgm", "PGM", "PNM 家族：灰度", &["image2"], &["pgm", "pam"], &["image2", "pgm_pipe"]),
    f("pbm", "PBM", "PNM 家族：黑白", &["image2"], &["pbm", "pam"], &["image2", "pbm_pipe"]),
    f("pnm", "PNM", "PNM 家族统称：按 PPM 写出（-f image2 -c:v ppm）", &["image2"], &["ppm", "pgm", "pbm", "pam"], &["image2", "ppm_pipe", "pgm_pipe", "pbm_pipe", "pam_pipe"]),
    f("xbm", "XBM", "位图文本（可直接嵌进 C 代码）", &["image2", "xbm"], &["xbm"], &["image2", "xbm_pipe"]),
    f("xwd", "XWD", "X Window Dump", &["image2", "xwd"], &["xwd"], &["image2", "xwd_pipe"]),
    f("heic", "HEIC", "HEIF/HEVC 图片：ffmpeg 无 .heic 封装器，本机 ImageMagick 的 HEIC 只支持读取（r--）→ 写不出", &["heif", "heic"], &["libheif", "libx265"], &["heic", "heif"]),
    f("pcx", "PCX", "PC Paintbrush（老式位图）", &["image2"], &["pcx"], &["image2", "pcx_pipe"]),
    f("exr", "EXR", "OpenEXR（影视合成）", &["image2"], &["exr"], &["image2", "exr_pipe"]),
    f("psd", "PSD", "Photoshop 文档：ffmpeg 写不出，由受管 ImageMagick 引擎写出（PSD rw+）", &["image2", "psd"], &["psd"], &["image2", "psd_pipe"]),
    f("dds", "DDS", "DirectDraw 贴图：ffmpeg 只能读，由受管 ImageMagick 引擎写出（DDS rw+）", &["dds"], &["dds"], &["image2", "dds_pipe"]),
    f("svg", "SVG", "矢量图：ffmpeg 只能读（svg_pipe）；ImageMagick 写出的 SVG 内部是栅格嵌入（非矢量重绘），暂不启用", &["svg"], &["svg"], &["image2", "svg_pipe"]),
];

/// 文档目标格式：输出能力看 `docs::supported_targets`，输入能力看 `docs::route_for`
struct DocSpec {
    id: &'static str,
    label: &'static str,
    note: &'static str,
    /// 在 `docs::supported_targets` 里的目标名（None = 无法作为输出格式）
    target: Option<&'static str>,
    engines: &'static [&'static str],
}

const fn d(
    id: &'static str,
    label: &'static str,
    note: &'static str,
    target: Option<&'static str>,
    engines: &'static [&'static str],
) -> DocSpec {
    DocSpec { id, label, note, target, engines }
}

/// 文档格式（23 种）
const DOC_FORMATS: &[DocSpec] = &[
    d("docx", "DOCX", "Word 文档（OOXML）：原生可读，写回需 pandoc", Some("docx"), &["native", "pandoc"]),
    d("doc", "DOC", "旧版 Word（OLE2）：需先在 Office/WPS 另存为 docx", None, &[]),
    d("odt", "ODT", "OpenDocument 文本文档", Some("odt"), &["native", "pandoc"]),
    d("rtf", "RTF", "RTF 富文本", Some("rtf"), &["pandoc"]),
    d("txt", "TXT", "纯文本（原生，无需外部引擎）", Some("txt"), &["native"]),
    d("md", "MD", "Markdown", Some("md"), &["native", "pandoc"]),
    d("html", "HTML", "网页 HTML", Some("html"), &["native", "pandoc"]),
    d("htm", "HTM", "网页 HTML（.htm 写法）", Some("html"), &["native", "pandoc"]),
    d("pdf", "PDF", "PDF：poppler 可解析为 txt/png，写出 PDF 需 Office/LaTeX", None, &["poppler"]),
    d("epub", "EPUB", "电子书 EPUB", Some("epub"), &["pandoc"]),
    d("xlsx", "XLSX", "Excel 表格（OOXML）：原生可读，写出需 Office/WPS", None, &["native"]),
    d("xls", "XLS", "旧版 Excel（OLE2）：需另存为 xlsx", None, &[]),
    d("ods", "ODS", "OpenDocument 表格：原生可读", None, &["native"]),
    d("csv", "CSV", "逗号分隔表格（原生）", Some("csv"), &["native"]),
    d("tsv", "TSV", "制表符分隔表格：原生可读", None, &["native"]),
    d("pptx", "PPTX", "PowerPoint 演示（OOXML）", Some("pptx"), &["native", "pandoc"]),
    d("ppt", "PPT", "旧版 PowerPoint（OLE2）：需另存为 pptx", None, &[]),
    d("odp", "ODP", "OpenDocument 演示：原生可读", None, &["native"]),
    d("rst", "RST", "reStructuredText", Some("rst"), &["pandoc"]),
    d("latex", "LATEX", "LaTeX（.tex）", Some("tex"), &["pandoc"]),
    d("json", "JSON", "Pandoc JSON / JSON 数据（当前引擎目标清单未包含）", None, &["pandoc"]),
    d("xml", "XML", "XML 文档（当前引擎目标清单未包含）", None, &["pandoc"]),
    d("org", "ORG", "Emacs Org-mode", Some("org"), &["pandoc"]),
];

/* ---- 探测：ffmpeg / pandoc / poppler 真实能力 ---- */

fn exe_name_conv(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// 在 PATH 中查找可执行文件（受管目录之外的回退）
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let full = dir.join(name);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}

/// 定位 ffmpeg：ctx.tools（启动时已解析的绝对路径）→ 受管目录 → PATH → tools::resolve_tool
fn ffmpeg_bin(ctx: &Ctx) -> Option<PathBuf> {
    if let Some(p) = ctx.tools.ffmpeg.clone().filter(|p| p.is_file()) {
        return Some(p);
    }
    let managed = ctx.dirs.bin.join(exe_name_conv("ffmpeg"));
    if managed.is_file() {
        return Some(managed);
    }
    if let Some(p) = which_on_path(&exe_name_conv("ffmpeg")) {
        return Some(p);
    }
    crate::tools::resolve_tool(ctx, "ffmpeg", ctx.settings.ffmpeg_path.as_ref()).0
}

fn run_capture(exe: &Path, args: &[&str]) -> Option<String> {
    let out = command_for(exe).args(args).stdin(Stdio::null()).output().ok()?;
    let text = crate::ctx::decode_output(&out.stdout);
    if !text.trim().is_empty() {
        return Some(text);
    }
    let err = crate::ctx::decode_output(&out.stderr);
    if err.trim().is_empty() {
        None
    } else {
        Some(err)
    }
}

/* ---- ImageMagick 引擎：ffmpeg 写不出的图片格式（PSD / DDS …） ---- */

/// 需要 ImageMagick 才能写出的目标格式：格式 id → magick 格式名。
///
/// 只收**真机实测写得出来**的：psd（PSD `rw+`）、dds（DDS `rw+`）。
/// heic / heif 刻意不在列：受管 ImageMagick 7.1.2-31 的 HEIC 标志位是 `r--`（只读），
/// 让它写只会得到一个扩展名叫 .heic 的 PNG（实测 1878 B PNG 数据 / “no encode delegate” 警告）。
const MAGICK_TARGETS: &[(&str, &str)] = &[("psd", "PSD"), ("dds", "DDS")];

/// 需要 ImageMagick 解码的输入：ffmpeg 完全没有 HEIC/HEIF 解封装器，对真实 PSD 也会解码报错
const MAGICK_INPUT_EXTS: &[&str] = &["psd", "heic", "heif"];

/// 输入交给 ImageMagick 解码时所需的 magick 格式名
pub fn magick_input_format(id: &str) -> Option<&'static str> {
    match id.to_ascii_lowercase().as_str() {
        "psd" => Some("PSD"),
        "heic" => Some("HEIC"),
        "heif" => Some("HEIF"),
        _ => None,
    }
}

/// 目标格式对应的 ImageMagick 输出格式名（None = 不需要 / 交给 ffmpeg）
pub fn magick_target(format: &str) -> Option<&'static str> {
    MAGICK_TARGETS
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(format))
        .map(|(_, m)| *m)
}

/// 该输入是否必须交给 ImageMagick（按扩展名）
pub fn magick_input_ext(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    MAGICK_INPUT_EXTS.iter().find(|e| **e == ext).copied()
}

/// 定位 ImageMagick：受管目录 bin/imagemagick → bin → PATH
///
/// 解析规则与「依赖检测」页共用一处：`crate::tools::resolve_imagemagick`（避免两处各写一套顺序）。
fn magick_bin(ctx: &Ctx) -> Option<PathBuf> {
    crate::tools::resolve_imagemagick(ctx)
}

/// 真机探测 ImageMagick：`magick -list format` 给出每个格式的读写能力
fn probe_magick_caps(ctx: &Ctx, caps: &mut MediaCaps) {
    let Some(magick) = magick_bin(ctx) else { return };
    let Some(text) = run_capture(&magick, &["-list", "format"]) else { return };
    let formats = parse_magick_formats(&text);
    if formats.is_empty() {
        return;
    }
    caps.magick = true;
    caps.magick_formats = formats;
}

/// 真实探测本机引擎能力（只跑一次就够，见 [`caps_cached`]）
fn probe_media_caps(ctx: &Ctx) -> MediaCaps {
    // 文档引擎：pandoc / poppler（docs 模块已有「受管目录 → PATH」的解析链，直接复用）
    let engines = crate::docs::capabilities(ctx);
    let mut caps = MediaCaps {
        pandoc: engines.get("pandoc").and_then(|v| v.as_bool()).unwrap_or(false),
        poppler: engines.get("poppler").and_then(|v| v.as_bool()).unwrap_or(false),
        ..Default::default()
    };
    let Some(ffmpeg) = ffmpeg_bin(ctx) else {
        probe_magick_caps(ctx, &mut caps);
        return caps;
    };
    let enc = run_capture(&ffmpeg, &["-hide_banner", "-encoders"]);
    let fmt = run_capture(&ffmpeg, &["-hide_banner", "-formats"]);
    if enc.is_none() && fmt.is_none() {
        probe_magick_caps(ctx, &mut caps);
        return caps; // ffmpeg 存在但跑不起来：按不可用处理，格式全部置灰
    }
    caps.ffmpeg = true;
    if let Some(text) = enc {
        caps.encoders = parse_encoder_names(&text).into_iter().collect();
        caps.experimental = parse_experimental_encoders(&text).into_iter().collect();
    }
    if let Some(text) = fmt {
        let (demuxers, muxers) = parse_format_names(&text);
        caps.demuxers = demuxers.into_iter().collect();
        caps.muxers = muxers.into_iter().collect();
    }
    probe_magick_caps(ctx, &mut caps);
    caps
}

/// 进程内缓存的能力快照（探测成功才缓存；失败下次再试）
fn caps_cached(ctx: &Ctx) -> MediaCaps {
    static CAPS: OnceLock<MediaCaps> = OnceLock::new();
    if let Some(c) = CAPS.get() {
        return c.clone();
    }
    let caps = probe_media_caps(ctx);
    if caps.ffmpeg {
        let _ = CAPS.set(caps.clone());
    }
    caps
}

/* ---- 目录组装（纯函数，可单测） ---- */

/// 图片 / 文档的输入判定优先看扩展名，其余（视频 / 音频）看 ffmpeg 是否有对应解封装器
fn media_input_ok(spec: &FmtSpec, caps: &MediaCaps) -> bool {
    let probe = format!("probe.{}", spec.id);
    if is_image_path(&probe) || crate::docs::is_document_path(&probe) {
        // .heic/.heif：ffmpeg 侧一个解封装器都没有，只有能读的 ImageMagick 才算可输入
        if let Some(m) = magick_input_format(spec.id) {
            if !caps.has_demuxer(spec.demuxers) {
                return caps.magick_can_read(m);
            }
        }
        return true;
    }
    caps.ffmpeg && caps.has_demuxer(spec.demuxers)
}

/// 该图片目标是否由 ImageMagick 写出（ffmpeg 写不出的格式）
fn magick_output_ok(spec: &FmtSpec, caps: &MediaCaps) -> bool {
    magick_target(spec.id)
        .map(|m| caps.magick_can_write(m))
        .unwrap_or(false)
}

fn media_entry(spec: &FmtSpec, caps: &MediaCaps) -> serde_json::Value {
    let by_ffmpeg = caps.ffmpeg
        && (spec.muxers.is_empty() || caps.has_muxer(spec.muxers))
        && (spec.encoders.is_empty() || caps.has_encoder(spec.encoders));
    let by_magick = magick_output_ok(spec, caps);
    let mut engines: Vec<&str> = Vec::new();
    if by_ffmpeg {
        engines.push("ffmpeg");
    }
    if by_magick {
        engines.push("imagemagick");
    }
    serde_json::json!({
        "id": spec.id,
        "label": spec.label,
        "note": spec.note,
        "available": by_ffmpeg || by_magick,
        "engines": engines,
        "input": media_input_ok(spec, caps),
    })
}

/// 文档输入通路：扩展名判定 + 复用 `docs::route_for` 的真实路由结果 + 本机引擎可用性
fn doc_input_ok(id: &str, caps: &MediaCaps, natives: &[String]) -> bool {
    use crate::docs::Route;
    let probe = format!("probe.{id}");
    if !crate::docs::is_document_path(&probe) && !natives.iter().any(|n| n.as_str() == id) {
        return false;
    }
    for target in ["md", "txt"] {
        match crate::docs::route_for(Path::new(&probe), target) {
            Route::Native => return true,
            Route::Pandoc if caps.pandoc => return true,
            Route::PopplerPdf if caps.poppler => return true,
            _ => {}
        }
    }
    false
}

fn doc_entry(spec: &DocSpec, caps: &MediaCaps, targets: &[String], natives: &[String]) -> serde_json::Value {
    let input = doc_input_ok(spec.id, caps, natives);
    let output_ok = spec
        .target
        .map(|t| targets.iter().any(|x| x.as_str() == t))
        .unwrap_or(false);
    serde_json::json!({
        "id": spec.id,
        "label": spec.label,
        "note": spec.note,
        "available": output_ok || input,
        "engines": spec.engines,
        "input": input,
    })
}

/// 组装格式目录：`{"total":88,"groups":[{id,label,count,formats:[…]}],"engines":{…}}`
///
/// `available` 的含义是「本机引擎能处理该格式（作为目标或来源）」，
/// 列表长度与引擎无关：引擎缺失的格式仍然在列，只是 `available=false`（UI 置灰）。
pub fn build_catalog(caps: &MediaCaps) -> serde_json::Value {
    let targets = crate::docs::supported_targets(caps.pandoc, caps.poppler);
    let natives = crate::docs::native_formats();

    let video: Vec<serde_json::Value> = VIDEO_FORMATS.iter().map(|s| media_entry(s, caps)).collect();
    let audio: Vec<serde_json::Value> = AUDIO_FORMATS.iter().map(|s| media_entry(s, caps)).collect();
    let image: Vec<serde_json::Value> = IMAGE_FORMATS.iter().map(|s| media_entry(s, caps)).collect();
    let document: Vec<serde_json::Value> =
        DOC_FORMATS.iter().map(|s| doc_entry(s, caps, &targets, &natives)).collect();

    let total = video.len() + audio.len() + image.len() + document.len();
    // group 里带 count：前端 normalizeCatalog 会用它展示「分类计数」，缺省时会回落到 formats.length
    let group = |id: &str, label: &str, formats: Vec<serde_json::Value>| {
        let count = formats.len();
        serde_json::json!({"id": id, "label": label, "count": count, "formats": formats})
    };
    let groups = vec![
        group("video", "视频", video),
        group("audio", "音频", audio),
        group("image", "图片", image),
        group("document", "文档", document),
    ];
    serde_json::json!({
        "total": total,
        "groups": groups,
        "engines": {
            "ffmpeg": caps.ffmpeg,
            "pandoc": caps.pandoc,
            "poppler": caps.poppler,
            "imagemagick": caps.magick,
        },
    })
}

/// 转换格式目录：按 视频 / 音频 / 图片 / 文档 分组，`available` 取自本机真实引擎能力
pub fn convert_formats(ctx: &Ctx) -> serde_json::Value {
    build_catalog(&caps_cached(ctx))
}

#[cfg(test)]
mod catalog_tests {
    //! 格式目录测试：喂真实 ffmpeg 输出样本，走与线上完全一致的解析 + 组装路径。

    use super::*;

    /// 真机 ffmpeg 8.0.1（gyan.dev full build）`-hide_banner -encoders` 输出的真实片段
    const SAMPLE_ENCODERS: &str = r#"
 V....D apng                 APNG (Animated Portable Network Graphics) image
 V....D libaom-av1           libaom AV1 (codec av1)
 V....D librav1e             librav1e AV1 (codec av1)
 V..... libsvtav1            SVT-AV1(Scalable Video Technology for AV1) encoder (codec av1)
 V....D av1_nvenc            NVIDIA NVENC av1 encoder (codec av1)
 V....D bmp                  BMP (Windows and OS/2 bitmap)
 VFS..D dnxhd                VC3/DNxHD
 VFS..D dvvideo              DV (Digital Video)
 V.S..D ffv1                 FFmpeg video codec #1
 V....D flv                  FLV / Sorenson Spark / Sorenson H.263 (Flash Video) (codec flv1)
 V....D gif                  GIF (Graphics Interchange Format)
 V....D h263                 H.263 / H.263-1996
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_amf             AMD AMF H.264 Encoder (codec h264)
 V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 V..... h264_qsv             H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (Intel Quick Sync Video acceleration) (codec h264)
 V....D libx265              libx265 H.265 / HEVC (codec hevc)
 V....D hevc_nvenc           NVIDIA NVENC hevc encoder (codec hevc)
 VF...D ljpeg                Lossless JPEG
 VFS..D mjpeg                MJPEG (Motion JPEG)
 V.S..D mpeg1video           MPEG-1 video
 V.S..D mpeg2video           MPEG-2 video
 V.S..D mpeg4                MPEG-4 part 2
 V....D libxvid              libxvidcore MPEG-4 part 2 (codec mpeg4)
 V....D msmpeg4              MPEG-4 part 2 Microsoft variant version 3 (codec msmpeg4v3)
 V....D pam                  PAM (Portable AnyMap) image
 V....D pbm                  PBM (Portable BitMap) image
 V....D pgm                  PGM (Portable GrayMap) image
 VF...D png                  PNG (Portable Network Graphics) image
 V....D ppm                  PPM (Portable PixelMap) image
 VFS... prores_ks            Apple ProRes (iCodec Pro) (codec prores)
 VF...D rawvideo             raw video
 V....D targa                Truevision Targa image
 V....D libtheora            libtheora Theora (codec theora)
 VF...D tiff                 TIFF image
 V....D libvpx               libvpx VP8 (codec vp8)
 V....D libvpx-vp9           libvpx VP9 (codec vp9)
 V....D libwebp_anim         libwebp WebP image (codec webp)
 V....D libwebp              libwebp WebP image (codec webp)
 V....D wmv1                 Windows Media Video 7
 V....D wmv2                 Windows Media Video 8
 V....D xbm                  XBM (X BitMap) image
 V....D xwd                  XWD (X Window Dump) image
 VF...D exr                  OpenEXR image
 V....D pcx                  PC Paintbrush PCX image
 A....D aac                  AAC (Advanced Audio Coding)
 A....D ac3                  ATSC A/52A (AC-3)
 A....D ac3_fixed            ATSC A/52A (AC-3) (codec ac3)
 A....D alac                 ALAC (Apple Lossless Audio Codec)
 A....D libopencore_amrnb    OpenCORE AMR-NB (Adaptive Multi-Rate Narrow-Band) (codec amr_nb)
 A..X.D dca                  DCA (DTS Coherent Acoustics) (codec dts)
 A....D eac3                 ATSC A/52 E-AC-3
 A....D flac                 FLAC (Free Lossless Audio Codec)
 A....D mp2                  MP2 (MPEG audio layer 2)
 A....D mp2fixed             MP2 fixed point (MPEG audio layer 2) (codec mp2)
 A....D libtwolame           libtwolame MP2 (MPEG audio layer 2) (codec mp2)
 A....D libmp3lame           libmp3lame MP3 (MPEG audio layer 3) (codec mp3)
 A....D libshine             libshine MP3 (MPEG audio layer 3) (codec mp3)
 A..X.D opus                 Opus
 A....D libopus              libopus Opus (codec opus)
 A....D pcm_alaw             PCM A-law / G.711 A-law
 A....D pcm_f32le            PCM 32-bit floating point little-endian
 A....D pcm_mulaw            PCM mu-law / G.711 mu-law
 A....D pcm_s16be            PCM signed 16-bit big-endian
 A....D pcm_s16le            PCM signed 16-bit little-endian
 A....D pcm_s24be            PCM signed 24-bit big-endian
 A....D pcm_s24le            PCM signed 24-bit little-endian
 A....D pcm_s32le            PCM signed 32-bit little-endian
 A....D pcm_u8               PCM unsigned 8-bit
 A....D tta                  TTA (True Audio)
 A....D libvorbis            libvorbis (codec vorbis)
 A....D wmav1                Windows Media Audio 1
 A....D wmav2                Windows Media Audio 2
"#;

    /// 同机 `-hide_banner -formats` 输出的真实片段
    const SAMPLE_FORMATS: &str = r#"
  E  3g2             3GP2 (3GPP2 file format)
  E  3gp             3GP (3GPP file format)
 D   aac             raw ADTS AAC (Advanced Audio Coding)
 DE  ac3             raw AC-3
  E  adts            ADTS AAC (Advanced Audio Coding)
 DE  aiff            Audio IFF
 DE  amr             3GPP AMR
 D   amrnb           raw AMR-NB
 D   ape             Monkey's Audio
 DE  apng            Animated Portable Network Graphics
 DE  asf             ASF (Advanced / Active Streaming Format)
 D   asf_o           ASF (Advanced / Active Streaming Format)
 DE  au              Sun AU
 DE  avi             AVI (Audio Video Interleaved)
  E  avif            AVIF
 D   bmp_pipe        piped bmp sequence
 DE  caf             Apple CAF (Core Audio Format)
 DE  dts             raw DTS
 D   dtshd           raw DTS-HD
  E  dvd             MPEG-2 PS (DVD VOB)
 DE  eac3            raw E-AC-3
 DE  flac            raw FLAC
 DE  flv             FLV (Flash Video)
 DE  gif             CompuServe Graphics Interchange Format (GIF)
 D   gif_pipe        piped gif sequence
 DE  ico             Microsoft Windows ICO
 DE  image2          image2 sequence
  E  ipod            iPod H.264 MP4 (MPEG-4 Part 14)
 D   jpeg_pipe       piped jpeg sequence
 D   live_flv        live RTMP FLV (Flash Video)
 D   loas            LOAS AudioSyncStream
 DE  m4v             raw MPEG-4 video
  E  matroska        Matroska
 D   matroska,webm   Matroska / WebM
 DE  mjpeg           raw MJPEG video
  E  mov             QuickTime / MOV
 D   mov,mp4,m4a,3gp,3g2,mj2 QuickTime / MOV
  E  mp2             MP2 (MPEG audio layer 2)
 DE  mp3             MP3 (MPEG audio layer 3)
  E  mp4             MP4 (MPEG-4 Part 14)
 DE  mpeg            MPEG-1 Systems / MPEG program stream
  E  mpeg1video      raw MPEG-1 video
  E  mpeg2video      raw MPEG-2 video
 DE  mpegts          MPEG-TS (MPEG-2 Transport Stream)
 D   mpegtsraw       raw MPEG-TS (MPEG-2 Transport Stream)
 D   mpegvideo       raw MPEG video
 DE  mxf             MXF (Material eXchange Format)
 DE  ogg             Ogg
  E  ogv             Ogg Video
  E  opus            Ogg Opus
 D   pam_pipe        piped pam sequence
 D   pbm_pipe        piped pbm sequence
 D   pgm_pipe        piped pgm sequence
 D   png_pipe        piped png sequence
 D   ppm_pipe        piped ppm sequence
 D   tiff_pipe       piped tiff sequence
 DE  tta             TTA (True Audio)
  E  vob             MPEG-2 PS (VOB)
 DE  w64             Sony Wave64
 DE  wav             WAV / WAVE (Waveform Audio)
  E  webm            WebM
  E  webp            WebP
 D   webp_pipe       piped webp sequence
 D   xbm_pipe        piped xbm sequence
 D   xwd_pipe        piped xwd sequence
 D   dds_pipe        piped dds sequence
 DE  dv              DV (Digital Video)
 D   exr_pipe        piped exr sequence
  E  f4v             F4V Adobe Flash Video
 D   pcx_pipe        piped pcx sequence
 D   psd_pipe        piped psd sequence
 D   svg_pipe        piped svg sequence
 DE  yuv4mpegpipe    YUV4MPEG pipe
"#;

    /// 满血快照：ffmpeg 样本 + pandoc + poppler
    fn full_caps() -> MediaCaps {
        let mut c = MediaCaps::from_outputs(SAMPLE_ENCODERS, SAMPLE_FORMATS);
        c.pandoc = true;
        c.poppler = true;
        c
    }

    /// 真机 ImageMagick 7.1.2-31 portable Q16-HDRI x64 `magick -list format` 的真实片段
    /// （受管目录 bin/imagemagick/magick.exe）
    const MAGICK_FORMATS: &str = r#"Format  Mode  Description
-------------------------------------------------------------------------------
      3FR  r--   Hasselblad CFV/H3D39II Raw Format (0.22.2-Release)
        A* rw+   Raw alpha samples
     APNG  rw+   Animated Portable Network Graphics
   ASHLAR* -w+   Image sequence laid out in continuous irregular courses
     AVIF  rw+   AV1 Image File Format (1.23.2)
      BMP* rw-   Microsoft Windows bitmap image
      DDS* rw+   Microsoft DirectDraw Surface
     HEIC  r--   High Efficiency Image Format (1.23.2)
     HEIF  r--   High Efficiency Image Format (1.23.2)
      ICO* rw+   Microsoft icon
     JPEG* rw-   Joint Photographic Experts Group JFIF format (libjpeg-turbo 3.2.0)
     MSVG* rw+   ImageMagick's own SVG internal renderer
      PNG* rw-   Portable Network Graphics (libpng 1.6.58)
      PSD* rw+   Adobe Photoshop bitmap
      SVG  rw+   Scalable Vector Graphics (RSVG 2.40.20)
     TIFF* rw+   Tagged Image File Format (LIBTIFF, Version 4.7.2)
     WEBP* rw+   WebP Image Format (libwebp 1.6.0 [0210])
"#;

    /// 满血快照 + 受管 ImageMagick（PSD / DDS 由它写出）
    fn full_caps_with_magick() -> MediaCaps {
        full_caps().with_magick_output(MAGICK_FORMATS)
    }

    fn find_group<'a>(cat: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        cat["groups"]
            .as_array()
            .expect("groups 必须是数组")
            .iter()
            .find(|g| g["id"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("缺少分组 {id}"))
    }

    fn find_format<'a>(cat: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
        for g in cat["groups"].as_array().expect("groups 必须是数组") {
            for f in g["formats"].as_array().expect("formats 必须是数组") {
                if f["id"].as_str() == Some(id) {
                    return f;
                }
            }
        }
        panic!("格式目录里没有 {id}")
    }

    fn ids(cat: &serde_json::Value, group: &str) -> Vec<String> {
        find_group(cat, group)["formats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /* ---------- 解析器 ---------- */

    #[test]
    fn ffmpeg_encoder_parser_reads_real_output() {
        let names = parse_encoder_names(SAMPLE_ENCODERS);
        for need in [
            "libx264", "libx265", "aac", "libmp3lame", "libopus", "libvorbis", "flac", "alac", "ac3", "eac3", "dca", "tta",
            "pcm_s16le", "png", "mjpeg", "gif", "libwebp", "bmp", "tiff", "xbm", "xwd", "libsvtav1", "libaom-av1", "libtheora",
            "mpeg2video", "h264_nvenc", "wmav2", "libopencore_amrnb", "targa", "ppm", "pgm", "pbm", "pam",
        ] {
            assert!(names.contains(&need.to_string()), "未从真实 -encoders 输出里解析出 {need}");
        }
        // 图例 / 标题行不能被当成编码器
        assert!(!names.iter().any(|n| n.contains('=')), "图例行被误解析：{names:?}");
        assert!(!names.iter().any(|n| n == "encoders:" || n == "------"));
        // 全量解析：样本 70 行编码器
        assert!(names.len() >= 60, "解析到的编码器太少：{}", names.len());
        // 大小写归一化
        let lower = parse_encoder_names(" V....D LIBX264  libx264 H.264\n");
        assert_eq!(lower, vec!["libx264".to_string()]);
        // 实验性编码器（标志位含 X）单独识别：ffmpeg 不传 -strict experimental 会拒用
        let exp = parse_experimental_encoders(SAMPLE_ENCODERS);
        assert!(exp.contains(&"dca".to_string()), "dca（A..X.D）应识别为实验性：{exp:?}");
        assert!(exp.contains(&"opus".to_string()), "原生 opus（A..X.D）应识别为实验性");
        assert!(!exp.contains(&"libopus".to_string()), "libopus 不是实验性编码器");
        assert!(!exp.contains(&"libx264".to_string()));
    }

    #[test]
    fn ffmpeg_format_parser_splits_demuxer_and_muxer() {
        let (demuxers, muxers) = parse_format_names(SAMPLE_FORMATS);
        for n in ["mp4", "matroska", "webm", "mpegts", "asf", "avi", "mp3", "wav", "flac", "image2", "mpeg", "gif"] {
            assert!(muxers.contains(&n.to_string()), "{n} 应是可用封装器：{muxers:?}");
            assert!(demuxers.contains(&n.to_string()), "{n} 应是可用解封装器：{demuxers:?}");
        }
        // 只解封装、不可封装
        assert!(demuxers.contains(&"png_pipe".to_string()));
        assert!(demuxers.contains(&"mov".to_string()));
        assert!(demuxers.contains(&"m4a".to_string()), "mov,mp4,m4a… 需要按逗号拆开");
        assert!(!muxers.contains(&"ape".to_string()), "APE 只能解码");
        assert!(!muxers.contains(&"heif".to_string()));
        // 图例行（『 D.. = Demuxing supported』）不能混进来
        assert!(!demuxers.iter().any(|n| n.starts_with('=') || n.contains('=')));
        assert!(!muxers.iter().any(|n| n.contains('=')));
    }

    /* ---------- 目录本身 ---------- */

    #[test]
    fn catalog_has_more_than_60_formats() {
        let cat = build_catalog(&full_caps());
        let total = cat["total"].as_i64().expect("total 必须是数字");
        assert!(total > 60, "格式总数应 > 60，实际 {total}");
        let listed: usize = cat["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["formats"].as_array().unwrap().len())
            .sum();
        assert_eq!(listed as i64, total, "total 必须等于各分组实际条目数");
        // 每个分组都要带 count（前端 normalizeCatalog 直接读它做分类计数）
        for g in cat["groups"].as_array().unwrap() {
            let n = g["formats"].as_array().unwrap().len();
            assert_eq!(g["count"].as_i64(), Some(n as i64), "分组 {} 的 count 与条目数不一致", g["id"]);
        }
        // 引擎状态随目录一起返回
        for e in ["ffmpeg", "pandoc", "poppler"] {
            assert_eq!(cat["engines"][e], serde_json::json!(true), "engines.{e}");
        }
    }

    /// 字段契约：与前端 `src/services/formatCatalog.ts` 的 ConvertFormatItem 一一对应，
    /// 多字段/缺字段都会让 UI 静默降级，所以这里逐字段卡死。
    #[test]
    fn catalog_entries_match_declared_schema() {
        let cat = build_catalog(&full_caps());
        assert!(cat["total"].is_i64());
        assert!(cat["engines"].is_object());
        for g in cat["groups"].as_array().unwrap() {
            let keys: Vec<String> = g.as_object().unwrap().keys().cloned().collect();
            assert_eq!(keys.len(), 4, "分组字段应为 id/label/count/formats：{keys:?}");
            for k in ["id", "label", "count", "formats"] {
                assert!(g.get(k).is_some(), "分组缺少 {k}");
            }
            assert!(g["id"].is_string() && g["label"].is_string() && g["count"].is_i64());
            for f in g["formats"].as_array().unwrap() {
                let mut keys: Vec<String> = f.as_object().unwrap().keys().cloned().collect();
                keys.sort();
                assert_eq!(
                    keys,
                    vec!["available", "engines", "id", "input", "label", "note"],
                    "格式条目字段必须与前端 ConvertFormatItem 完全一致：{f}"
                );
                assert!(f["id"].is_string() && !f["id"].as_str().unwrap().is_empty());
                assert!(f["label"].is_string() && f["available"].is_boolean() && f["input"].is_boolean());
                assert!(f["engines"].is_array() && f["note"].is_string());
                assert_eq!(f["id"].as_str().unwrap(), f["id"].as_str().unwrap().to_lowercase(), "id 必须是小写扩展名");
            }
        }
    }

    #[test]
    fn catalog_has_four_groups_each_with_enough_formats() {
        let cat = build_catalog(&full_caps());
        let groups = cat["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 4, "必须是 视频/音频/图片/文档 四组");
        for (id, label) in [("video", "视频"), ("audio", "音频"), ("image", "图片"), ("document", "文档")] {
            let g = find_group(&cat, id);
            assert_eq!(g["label"].as_str(), Some(label));
            let n = g["formats"].as_array().unwrap().len();
            assert!(n >= 8, "分组 {id} 只有 {n} 种格式");
        }
        // 任务书点名的覆盖范围必须都在
        let video = ids(&cat, "video");
        for id in [
            "mp4", "mkv", "mov", "avi", "webm", "flv", "wmv", "m4v", "mpg", "mpeg", "ts", "m2ts", "3gp", "ogv", "mxf", "vob",
            "asf", "m2v", "gif", "f4v", "dv", "y4m",
        ] {
            assert!(video.contains(&id.to_string()), "视频缺少 {id}");
        }
        let audio = ids(&cat, "audio");
        for id in [
            "mp3", "aac", "flac", "wav", "opus", "m4a", "ogg", "wma", "alac", "aiff", "ac3", "eac3", "dts", "amr", "ape", "mka",
            "mp2", "m4b", "caf", "au", "tta",
        ] {
            assert!(audio.contains(&id.to_string()), "音频缺少 {id}");
        }
        let image = ids(&cat, "image");
        for id in [
            "png", "jpg", "webp", "bmp", "tiff", "ico", "avif", "gif", "tga", "ppm", "pgm", "pbm", "pnm", "xbm", "xwd", "pcx",
            "exr", "psd", "dds", "svg",
        ] {
            assert!(image.contains(&id.to_string()), "图片缺少 {id}");
        }
        let doc = ids(&cat, "document");
        for id in [
            "docx", "doc", "odt", "rtf", "txt", "md", "html", "htm", "pdf", "epub", "xlsx", "xls", "ods", "csv", "tsv",
            "pptx", "ppt", "odp", "rst", "latex", "json", "xml",
        ] {
            assert!(doc.contains(&id.to_string()), "文档缺少 {id}");
        }
        // 各分组条目数去重（同一组内不得重复 id）
        for gid in ["video", "audio", "image", "document"] {
            let mut v = ids(&cat, gid);
            let n = v.len();
            v.sort();
            v.dedup();
            assert_eq!(v.len(), n, "分组 {gid} 有重复条目");
        }
    }

    #[test]
    fn catalog_core_formats_available_on_full_engine() {
        let cat = build_catalog(&full_caps());
        for id in ["mp4", "mp3", "png", "docx", "pdf"] {
            let f = find_format(&cat, id);
            assert_eq!(f["available"], serde_json::json!(true), "{id} 在本机应可用：{f}");
            assert!(f["engines"].as_array().map(|a| !a.is_empty()).unwrap_or(false), "{id} 应标注引擎");
            assert!(f["label"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "{id} 缺少 label");
            assert!(f["note"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "{id} 缺少 note");
        }
        // 图片 / 文档入口必须能作为输入源
        for id in ["png", "jpg", "webp", "gif", "mp4", "mp3", "docx", "pdf", "xlsx", "md"] {
            assert_eq!(find_format(&cat, id)["input"], serde_json::json!(true), "{id} 应可作输入源");
        }
        // 靠 ffmpeg 解封装的格式：本机有 demuxer 才算可输入
        assert_eq!(find_format(&cat, "mkv")["input"], serde_json::json!(true));
    }

    #[test]
    fn catalog_greys_out_formats_without_engine_support() {
        let cat = build_catalog(&full_caps());
        // ffmpeg 8 无 heif 封装器 → HEIC 保留在列但置灰（UI 灰显）
        let heic = find_format(&cat, "heic");
        assert_eq!(heic["available"], serde_json::json!(false), "HEIC 应置灰：{heic}");
        assert_eq!(heic["id"].as_str(), Some("heic"), "置灰也必须保留在列表里");
        assert_eq!(heic["input"], serde_json::json!(false), "ffmpeg 无 HEIC 解封装器，没装 ImageMagick 时也不能当输入");
        // APE 只能解码，不能写出
        assert_eq!(find_format(&cat, "ape")["available"], serde_json::json!(false), "APE 无法写出");
        // DTS（实验性 dca + -strict experimental）/ ALAC（写进 M4A）/ PNM（image2 + ppm）本机写得出来
        for id in ["dts", "alac", "pnm"] {
            let f = find_format(&cat, id);
            assert_eq!(f["available"], serde_json::json!(true), "{id} 本机 ffmpeg 写得出，不该置灰：{f}");
            assert!(f["engines"].as_array().unwrap().iter().any(|e| e == "ffmpeg"), "{id} 应标注 ffmpeg 引擎：{f}");
        }
        // 需要特定参数的格式（引擎支持，note 里写明约束）仍算可用
        for id in ["mxf", "dv", "amr"] {
            let f = find_format(&cat, id);
            assert_eq!(f["available"], serde_json::json!(true), "{id} 本机 ffmpeg 支持（含参数约束）");
            assert!(!f["note"].as_str().unwrap().is_empty());
        }
        // 只能读、不能写且没有引擎接手的图片格式：置灰但可作输入
        for id in ["psd", "dds", "svg"] {
            let f = find_format(&cat, id);
            assert_eq!(f["available"], serde_json::json!(false), "{id} ffmpeg 写不出、也没有 ImageMagick，应置灰");
            assert_eq!(f["input"], serde_json::json!(true), "{id} 应可作输入（*_pipe 解封装器）");
        }
        // 旧版二进制 Office：无任何通路
        for id in ["doc", "ppt", "xls"] {
            assert_eq!(find_format(&cat, id)["available"], serde_json::json!(false), "{id} 应置灰");
        }
        // xlsx / ods / pdf 虽然写不出，但本机原生 / poppler 能读 → 可用（input=true）
        for id in ["xlsx", "ods", "pdf"] {
            assert_eq!(find_format(&cat, id)["available"], serde_json::json!(true), "{id} 可作输入");
            assert_eq!(find_format(&cat, id)["input"], serde_json::json!(true), "{id} input");
        }

        // 没有任何外部引擎时：列表长度不变，媒体格式全部置灰，原生文档仍可用
        let bare = build_catalog(&MediaCaps::default());
        assert_eq!(bare["total"], cat["total"], "格式清单长度与引擎无关");
        for id in ["mp4", "mp3", "png", "heic", "ape", "pdf"] {
            assert_eq!(find_format(&bare, id)["available"], serde_json::json!(false), "无引擎时 {id} 应置灰");
        }
        assert_eq!(find_format(&bare, "mp4")["input"], serde_json::json!(false), "无 ffmpeg 时不得声称能解 MP4");
        assert_eq!(find_format(&bare, "png")["input"], serde_json::json!(true), "图片入口按扩展名判定");
        assert_eq!(find_format(&bare, "docx")["available"], serde_json::json!(true), "docx→md 走原生解析，不需要外部引擎");
        // 缺 pandoc 时 docx 写回能力随之消失（但原生输入仍在）
        let no_pandoc = MediaCaps { poppler: false, ..full_caps() };
        let no_pandoc = MediaCaps { pandoc: false, ..no_pandoc };
        assert_eq!(find_format(&build_catalog(&no_pandoc), "rtf")["available"], serde_json::json!(false), "RTF 输出需 pandoc");
        assert_eq!(find_format(&build_catalog(&no_pandoc), "docx")["input"], serde_json::json!(true));
    }

    /* ---------- ImageMagick 引擎（PSD / DDS / HEIC 输入） ---------- */

    #[test]
    fn magick_format_parser_reads_real_output() {
        let m = parse_magick_formats(MAGICK_FORMATS);
        // 真机标志位原样保留：PSD 可写、HEIC 只读（这就是不能把 HEIC 交给 magick 的原因）
        assert_eq!(m.get("PSD").map(String::as_str), Some("rw+"), "{m:?}");
        assert_eq!(m.get("DDS").map(String::as_str), Some("rw+"), "{m:?}");
        assert_eq!(m.get("HEIC").map(String::as_str), Some("r--"), "{m:?}");
        assert_eq!(m.get("BMP").map(String::as_str), Some("rw-"), "带 * 后缀的格式名要去掉后缀：{m:?}");
        assert_eq!(m.get("ASHLAR").map(String::as_str), Some("-w+"), "只写不读的格式也要保留：{m:?}");
        // 表头 / 分隔线不能混进来
        assert!(!m.contains_key("FORMAT") && !m.contains_key("MODE"), "{m:?}");
        assert!(m.len() > 10, "真实样本应解析出十多个格式：{}", m.len());
    }

    #[test]
    fn catalog_with_magick_engine_enables_psd_and_dds() {
        let caps = full_caps_with_magick();
        assert!(caps.magick && caps.magick_can_write("PSD") && !caps.magick_can_write("HEIC"));
        let cat = build_catalog(&caps);
        assert_eq!(cat["engines"]["imagemagick"], serde_json::json!(true), "目录要暴露 ImageMagick 就绪状态");

        for id in ["psd", "dds"] {
            let f = find_format(&cat, id);
            assert_eq!(f["available"], serde_json::json!(true), "装了 ImageMagick 后 {id} 必须可选：{f}");
            assert!(
                f["engines"].as_array().unwrap().iter().any(|e| e == "imagemagick"),
                "{id} 的 engines 要标出 imagemagick：{f}"
            );
            assert_eq!(f["input"], serde_json::json!(true));
        }
        // HEIC 仍然置灰：ImageMagick 的 HEIC 是 r--（只写会产出改名 PNG），绝不能标成可用
        let heic = find_format(&cat, "heic");
        assert_eq!(heic["available"], serde_json::json!(false), "HEIC 即使有 ImageMagick 也写不出：{heic}");
        assert_eq!(heic["input"], serde_json::json!(true), "但 ImageMagick 读得出 HEIC（r--）→ 可作输入");
        // SVG 依然不启用（magick 写出的是栅格嵌入）
        assert_eq!(find_format(&cat, "svg")["available"], serde_json::json!(false), "SVG 矢量重绘不成立，保持置灰");

        // 去掉 magick → PSD / DDS 立即回到置灰，机制确实挂在引擎上
        let no_magick = build_catalog(&full_caps());
        assert_eq!(find_format(&no_magick, "psd")["available"], serde_json::json!(false));
        assert_eq!(find_format(&no_magick, "heic")["input"], serde_json::json!(false));
    }

    #[test]
    fn magick_routing_matches_declared_formats() {
        assert_eq!(magick_target("PSD"), Some("PSD"));
        assert_eq!(magick_target("dds"), Some("DDS"));
        assert_eq!(magick_target("heic"), None, "HEIC 只能读，不能交给 magick 写出");
        assert_eq!(magick_target("png"), None, "png 仍走 ffmpeg");
        assert_eq!(magick_input_ext(r"D:\a\b\photo.HEIC"), Some("heic"));
        assert_eq!(magick_input_ext(r"D:\a\b\poster.psd"), Some("psd"));
        assert_eq!(magick_input_ext(r"D:\a\b\clip.mp4"), None);
    }

    #[test]
    fn catalog_against_this_machine_engine() {
        // 本机真实探测（报告用）：直接用应用自己的目录解析链找 ffmpeg / pandoc / poppler / imagemagick。
        // 断言与机器无关（清单长度固定），同时把真实计数与置灰清单打到 stderr 供人工核对，
        // 并把完整目录落到 convproof/catalog.json（前端队列卡片用例直接读它，保证两边看到的是同一份）。
        let ctx = Ctx::new(
            crate::ctx::AppDirs::new(),
            crate::ctx::ToolPaths::default(),
            crate::models::AppSettings::default(),
        );
        let cat = convert_formats(&ctx);
        let total = cat["total"].as_i64().unwrap();
        let mut line = String::new();
        let mut greyed: Vec<String> = Vec::new();
        for g in cat["groups"].as_array().unwrap() {
            let fs = g["formats"].as_array().unwrap();
            let avail = fs.iter().filter(|f| f["available"] == serde_json::json!(true)).count();
            line.push_str(&format!(
                " {}(可用 {}/{})",
                g["id"].as_str().unwrap(),
                avail,
                fs.len()
            ));
            for f in fs.iter().filter(|f| f["available"] != serde_json::json!(true)) {
                greyed.push(f["id"].as_str().unwrap_or_default().to_string());
            }
        }
        eprintln!(
            "[umi] convert_formats 真机探测：total={total}{} engines={}",
            line, cat["engines"]
        );
        eprintln!("[umi] 置灰格式（{}）：{}", greyed.len(), greyed.join(", "));
        assert!(total > 60, "格式总数应 > 60，实际 {total}");
        let dir = crate::ctx::AppDirs::new().data.join("convproof");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join("catalog.json"),
            serde_json::to_string_pretty(&cat).unwrap(),
        );
    }
}

/* ==================== 执行转换 ==================== */

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ConvertTick {
    pub seconds: Option<f64>,
    pub percent: Option<f64>,
    pub speed: Option<String>,
    pub done: bool,
}

pub fn parse_convert_line(line: &str, total_sec: Option<f64>) -> Option<ConvertTick> {
    let (time_re, prog_re, speed_re) = res();
    let mut t = ConvertTick::default();
    let mut hit = false;
    if let Some(c) = time_re.captures(line) {
        let h: f64 = c[1].parse().unwrap_or(0.0);
        let m: f64 = c[2].parse().unwrap_or(0.0);
        let s: f64 = c[3].parse().unwrap_or(0.0);
        let frac = format!("0.{}", &c[4]);
        let sec = h * 3600.0 + m * 60.0 + s + frac.parse::<f64>().unwrap_or(0.0);
        t.seconds = Some(sec);
        if let Some(total) = total_sec.filter(|x| *x > 0.01) {
            t.percent = Some(((sec / total) * 100.0).clamp(0.0, 99.9));
        }
        hit = true;
    }
    if let Some(c) = prog_re.captures(line) {
        if &c[1] == "end" {
            t.done = true;
            t.percent = Some(100.0);
        }
        hit = true;
    }
    if let Some(c) = speed_re.captures(line) {
        if &c[1] != "N/A" {
            t.speed = Some(c[1].to_string());
        }
        hit = true;
    }
    if hit {
        Some(t)
    } else {
        None
    }
}

#[derive(Debug)]
pub struct ConvertOutcome {
    pub exit_code: i32,
    pub stderr_tail: String,
    pub killed: bool,
}

/// 构建 ImageMagick 参数（纯函数，可单测）
///
/// magick 按输出扩展名挑编码器，所以不需要 `-f`；`-strip` 对应「清除元数据」。
pub fn build_magick_args(req: &ConvertRequest, output: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![req.input_file.clone()];
    if req.remove_metadata {
        a.push("-strip".into());
    }
    a.push(output.to_string_lossy().to_string());
    a
}

/// ImageMagick 转换执行体：ffmpeg 写不出的图片格式（PSD / DDS）与只有它认的输入（heic/heif/psd）。
///
/// 与文档分支一样没有进度流：开始发 5%、结束发 100% + done。
pub fn run_magick_convert<F>(
    ctx: &Ctx,
    req: &ConvertRequest,
    output: &Path,
    mut on_tick: F,
) -> anyhow::Result<ConvertOutcome>
where
    F: FnMut(ConvertTick),
{
    let magick = magick_bin(ctx)
        .ok_or_else(|| anyhow::anyhow!("未找到 ImageMagick 引擎（bin/imagemagick/magick.exe）"))?;
    if let Some(p) = output.parent() {
        ensure_dir(p)?;
    }
    on_tick(ConvertTick {
        seconds: None,
        percent: Some(5.0),
        speed: None,
        done: false,
    });
    let args = build_magick_args(req, output);
    crate::ctx::cwarn(&format!("[magick] {}", args.join(" ")));
    let out = command_for(&magick)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("启动 ImageMagick 失败：{e}"))?;
    let err = crate::ctx::decode_output(&out.stderr);
    if !out.status.success() || !output.is_file() {
        anyhow::bail!("{}", last_error_line(&err, "ImageMagick 转换失败"));
    }
    on_tick(ConvertTick {
        seconds: None,
        percent: Some(100.0),
        speed: None,
        done: true,
    });
    Ok(ConvertOutcome {
        exit_code: 0,
        stderr_tail: String::new(),
        killed: false,
    })
}

/// 取 stderr 的最后一行非空内容作为可读错误
fn last_error_line(stderr: &str, fallback: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .next_back()
        .map(|l| l.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn run_convert<F>(
    ctx: &Ctx,
    req: &ConvertRequest,
    output: &Path,
    mut on_tick: F,
    register_pid: impl Fn(u32),
    is_canceled: impl Fn() -> bool,
) -> anyhow::Result<ConvertOutcome>
where
    F: FnMut(ConvertTick),
{
    // 文档类输入：绝不走 ffmpeg（ffmpeg 不认 docx/xlsx/pptx/odt/pdf 这些容器），
    // 改由 docs 原生解析或 pandoc / poppler 外部引擎完成。
    if crate::docs::is_document_path(&req.input_file) {
        return run_document_convert(ctx, req, output, on_tick);
    }

    // 图片引擎分流：目标只有 ImageMagick 写得出来（PSD / DDS），
    // 或输入只有 ImageMagick 认得（.heic/.heif 容器、真实 PSD）。
    let wants_magick = magick_target(&req.format).is_some();
    let input_ext = magick_input_ext(&req.input_file);
    if wants_magick || input_ext.is_some() {
        if magick_bin(ctx).is_some() {
            return run_magick_convert(ctx, req, output, on_tick);
        }
        if wants_magick {
            anyhow::bail!(
                "{} 需要 ImageMagick 引擎：受管目录 bin/imagemagick 里没有找到 magick.exe",
                req.format.to_uppercase()
            );
        }
        if matches!(input_ext, Some("heic") | Some("heif")) {
            anyhow::bail!(
                "本机 ffmpeg 没有 HEIC/HEIF 解封装器，无法读取该输入；请安装 ImageMagick 引擎（bin/imagemagick）"
            );
        }
        // .psd：ffmpeg 有 psd 解码器（对复杂 PSD 可能失败），没有 ImageMagick 时仍按老路走
    }

    let ffmpeg = ctx.tools.ffmpeg()?.to_path_buf();
    if let Some(p) = output.parent() {
        ensure_dir(p)?;
    }
    let total = media_duration(ctx, &req.input_file);
    let args = build_ffmpeg_args(req, output);

    let mut cmd = command_for(&ffmpeg);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 ffmpeg 失败：{e}"))?;
    let pid = child.id();
    register_pid(pid);

    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let mut readers = Vec::new();
    if let Some(so) = child.stdout.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || read_lines(so, tx2, false)));
    }
    if let Some(se) = child.stderr.take() {
        let tx2 = tx.clone();
        readers.push(std::thread::spawn(move || read_lines(se, tx2, true)));
    }
    drop(tx);

    let mut errs: Vec<String> = Vec::new();
    let mut killed = false;
    let started = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok((is_err, line)) => {
                if let Some(t) = parse_convert_line(&line, total) {
                    on_tick(t);
                } else if is_err && !line.trim().is_empty() {
                    errs.push(line);
                    if errs.len() > 40 {
                        errs.remove(0);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if is_canceled() && !killed {
            kill_tree(pid);
            killed = true;
        }
        if started.elapsed() > Duration::from_secs(12 * 3600) && !killed {
            kill_tree(pid);
            killed = true;
            errs.push("转换超过 12 小时，已自动中止".into());
        }
        if let Ok(Some(_)) = child.try_wait() {
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(80)) {
                    Ok((is_err, line)) => {
                        if let Some(t) = parse_convert_line(&line, total) {
                            on_tick(t);
                        } else if is_err && !line.trim().is_empty() {
                            errs.push(line);
                        }
                    }
                    Err(_) => break,
                }
            }
            break;
        }
    }

    let status = child.wait()?;
    for h in readers {
        let _ = h.join();
    }
    Ok(ConvertOutcome {
        exit_code: status.code().unwrap_or(-1),
        stderr_tail: errs.join("\n"),
        killed,
    })
}

/// 文档转换执行体：原生解析 → pandoc → poppler，按 [`crate::docs::route_for`] 分流。
///
/// 与媒体分支不同，文档转换不产生 ffmpeg 进度流：
/// 开始发一次 5% 的 tick、结束后发一次 100% + done，让 UI 状态正常推进。
pub fn run_document_convert<F>(
    ctx: &Ctx,
    req: &ConvertRequest,
    output: &Path,
    mut on_tick: F,
) -> anyhow::Result<ConvertOutcome>
where
    F: FnMut(ConvertTick),
{
    use crate::docs::Route;

    let src = PathBuf::from(&req.input_file);
    if let Some(dir) = output.parent() {
        if !dir.as_os_str().is_empty() {
            ensure_dir(dir)?;
        }
    }
    on_tick(ConvertTick {
        seconds: None,
        percent: Some(5.0),
        speed: None,
        done: false,
    });

    let target = crate::docs::normalize_target(&req.format);
    match crate::docs::route_for(&src, &target) {
        Route::Native => crate::docs::native_convert(&src, output, &target)?,
        Route::Pandoc | Route::PopplerPdf => {
            let args = crate::docs::external_convert(ctx, &src, output, &target)?;
            crate::ctx::cwarn(&format!("[docs] {}", args.join(" ")));
        }
        Route::Unsupported(msg) => anyhow::bail!("{msg}"),
    }

    if !output.is_file() {
        anyhow::bail!("转换未生成输出文件：{}", output.display());
    }
    on_tick(ConvertTick {
        seconds: None,
        percent: Some(100.0),
        speed: None,
        done: true,
    });
    Ok(ConvertOutcome {
        exit_code: 0,
        stderr_tail: String::new(),
        killed: false,
    })
}

fn read_lines<R: Read>(mut rd: R, tx: mpsc::Sender<(bool, String)>, is_err: bool) {
    let mut buf = [0u8; 8192];
    let mut lb = LineBuffer::new();
    loop {
        match rd.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for l in lb.push(&buf[..n]) {
                    if tx.send((is_err, l)).is_err() {
                        return;
                    }
                }
            }
        }
    }
    if let Some(l) = lb.flush() {
        let _ = tx.send((is_err, l));
    }
}

/// 提取一帧缩略图（UI 预览用）
pub fn extract_thumbnail(ctx: &Ctx, video: &str, at_sec: f64, out: &Path) -> anyhow::Result<()> {
    let ffmpeg = ctx.tools.ffmpeg()?.to_path_buf();
    if let Some(p) = out.parent() {
        ensure_dir(p)?;
    }
    let status = command_for(&ffmpeg)
        .args([
            "-hide_banner",
            "-y",
            "-ss",
            &format!("{at_sec:.2}"),
            "-i",
            video,
            "-frames:v",
            "1",
            "-q:v",
            "3",
            &out.to_string_lossy().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("缩略图提取失败");
    }
    Ok(())
}

pub fn new_convert_task(req: &ConvertRequest, output: &Path, created: i64) -> crate::models::ConvertTask {
    crate::models::ConvertTask {
        id: crate::ctx::short_id(),
        input_file: req.input_file.clone(),
        output_file: output.to_string_lossy().to_string(),
        format: req.format.clone(),
        codec: req.video_codec.clone(),
        status: TaskStatus::Pending,
        progress: 0.0,
        error: None,
        created_time: created,
    }
}

#[cfg(test)]
mod tests {
    //! 文档路由测试：输入是文档时绝不能碰 ffmpeg。
    //! 这里的 ctx 刻意不带 ffmpeg / ffprobe，只要没报「未找到 ffmpeg」就说明路由正确。

    use super::*;
    use crate::docs::testing::{min_docx, min_xlsx, test_ctx, tmp_dir};

    fn doc_request(input: &Path, format: &str) -> ConvertRequest {
        ConvertRequest {
            input_file: input.to_string_lossy().to_string(),
            format: format.to_string(),
            ..Default::default()
        }
    }

    /* ---------------- ffmpeg / ImageMagick 参数拼装 ---------------- */

    /// 目标格式 = 输出扩展名（ALAC 例外：没有 .alac 封装器，按苹果惯例写 M4A）
    #[test]
    fn output_ext_follows_container_rules() {
        assert_eq!(output_ext("MP4"), "mp4");
        assert_eq!(output_ext("pnm"), "pnm");
        assert_eq!(output_ext("alac"), "m4a", "ffmpeg 无 .alac 封装器，必须落到 M4A");
    }

    #[test]
    fn dts_target_locks_experimental_dca_with_strict() {
        // UI 传的编码可能完全不相关（这里是 aac），DTS 只认 dca，而 dca 必须 -strict experimental
        let req = ConvertRequest {
            input_file: "in.wav".into(),
            format: "dts".into(),
            video_codec: None,
            audio_codec: Some("aac".into()),
            ..Default::default()
        };
        let a = build_ffmpeg_args(&req, Path::new("out.dts"));
        let cs = a.windows(2).find(|w| w[0] == "-c:a").map(|w| w[1].clone());
        assert_eq!(cs.as_deref(), Some("dca"), "{a:?}");
        let strict = a
            .windows(2)
            .any(|w| w[0] == "-strict" && w[1] == "experimental");
        assert!(strict, "dca 属实验性编码器，必须带 -strict experimental：{a:?}");
        assert_eq!(a.last().map(String::as_str), Some("out.dts"));
    }

    #[test]
    fn alac_target_locks_alac_encoder() {
        let req = ConvertRequest {
            input_file: "in.wav".into(),
            format: "alac".into(),
            video_codec: None,
            audio_codec: Some("copy".into()), // 用户选了 copy，后端也必须纠正
            ..Default::default()
        };
        let a = build_ffmpeg_args(&req, Path::new("out.m4a"));
        let cs = a.windows(2).find(|w| w[0] == "-c:a").map(|w| w[1].clone());
        assert_eq!(cs.as_deref(), Some("alac"), "{a:?}");
        assert!(!a.iter().any(|x| x == "copy"), "ALAC 目标不能透传 copy：{a:?}");
    }

    #[test]
    fn pnm_target_writes_image2_ppm() {
        // .pnm 既无封装器也不是 image2 的扩展名标签：必须显式 -f image2 -c:v ppm
        let req = ConvertRequest {
            input_file: "in.png".into(),
            format: "pnm".into(),
            ..Default::default()
        };
        let a = build_ffmpeg_args(&req, Path::new("out.pnm"));
        for pair in [["-f", "image2"], ["-c:v", "ppm"], ["-frames:v", "1"]] {
            assert!(
                a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]),
                "缺少 {} {}：{a:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(a.last().map(String::as_str), Some("out.pnm"));
    }

    #[test]
    fn ico_target_forces_bmp_and_clamps_size() {
        // ico 封装器只收位图且不自己挑编码器（默认会写出 4 字节空文件）
        let req = ConvertRequest {
            input_file: "in.png".into(),
            format: "ico".into(),
            ..Default::default()
        };
        let a = build_ffmpeg_args(&req, Path::new("out.ico"));
        assert!(a.windows(2).any(|w| w[0] == "-c:v" && w[1] == "bmp"), "{a:?}");
        let vf = a.windows(2).find(|w| w[0] == "-vf").map(|w| w[1].clone());
        assert!(vf.unwrap_or_default().contains("256"), "必须缩到 ≤256：{a:?}");
    }

    #[test]
    fn container_locked_video_targets_force_codec() {
        for (fmt, want) in [("m2v", "mpeg2video"), ("y4m", "rawvideo"), ("dv", "dvvideo")] {
            let req = ConvertRequest {
                input_file: "in.mp4".into(),
                format: fmt.into(),
                video_codec: Some("libx264".into()), // UI 默认编码，容器会拒绝
                audio_codec: Some("aac".into()),
                ..Default::default()
            };
            let a = build_ffmpeg_args(&req, Path::new(&format!("out.{fmt}")));
            let vc = a.windows(2).find(|w| w[0] == "-c:v").map(|w| w[1].clone());
            assert_eq!(vc.as_deref(), Some(want), "{fmt} 必须锁定 {want}：{a:?}");
        }
        // 普通格式不能被动到 copy
        let req = ConvertRequest {
            input_file: "in.mp4".into(),
            format: "mkv".into(),
            video_codec: Some("copy".into()),
            audio_codec: Some("copy".into()),
            ..Default::default()
        };
        let a = build_ffmpeg_args(&req, Path::new("out.mkv"));
        assert_eq!(a.iter().filter(|x| *x == "copy").count(), 2, "copy 应原样透传：{a:?}");
    }

    #[test]
    fn magick_args_are_minimal_and_support_strip() {
        let req = ConvertRequest {
            input_file: "D:/img/photo.png".into(),
            format: "psd".into(),
            ..Default::default()
        };
        let a = build_magick_args(&req, Path::new("D:/img/photo.psd"));
        assert_eq!(a, vec!["D:/img/photo.png", "D:/img/photo.psd"], "{a:?}");

        let req = ConvertRequest {
            remove_metadata: true,
            ..req
        };
        let a = build_magick_args(&req, Path::new("D:/img/photo.psd"));
        assert!(a.contains(&"-strip".to_string()), "{a:?}");
    }

    /* ---------------- 真机端到端（#[ignore]，只在有受管 ffmpeg/ImageMagick 的机器上跑） ---------------- */

    /// 用受管目录里的真实引擎构造 ctx（与线上 build_ctx 同一条解析链）
    fn machine_ctx() -> Ctx {
        let dirs = crate::ctx::AppDirs::new();
        let mut ctx = Ctx::new(
            dirs,
            crate::ctx::ToolPaths::default(),
            crate::models::AppSettings::default(),
        );
        ctx.tools = crate::tools::resolve_all(&ctx);
        ctx
    }

    fn ffmpeg_make(ctx: &Ctx, args: &[&str]) -> bool {
        let Ok(ff) = ctx.tools.ffmpeg() else { return false };
        crate::ctx::command_for(ff)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn describe(path: &Path) -> String {
        let n = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let head: String = std::fs::read(path)
            .map(|b| {
                b.iter()
                    .take(4)
                    .map(|c| format!("{c:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        format!("{} = {n} 字节（magic {head}）", path.display())
    }

    /// 稳定的证据目录（跑完留在磁盘上，报告里直接引用这些绝对路径）
    fn proof_dir(tag: &str) -> PathBuf {
        let dir = crate::ctx::AppDirs::new().data.join("convproof").join(tag);
        std::fs::create_dir_all(&dir).expect("建证据目录失败");
        dir
    }

    /// 真机证据：ffmpeg 能写但原先被置灰的格式（dts / alac / pnm）+ 原先写坏的 ico 全部真跑一遍
    #[test]
    #[ignore = "真机端到端：需要受管目录里的 ffmpeg"]
    fn real_machine_greyed_formats_now_convert() {
        let ctx = machine_ctx();
        assert!(ctx.tools.ffmpeg.is_some(), "没有受管 ffmpeg，无法跑真机用例");
        let dir = proof_dir("greyed");
        let wav = dir.join("src.wav");
        let png = dir.join("src.png");
        assert!(
            ffmpeg_make(&ctx, &["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-c:a", "pcm_s16le", &wav.to_string_lossy()]),
            "生成测试 wav 失败"
        );
        assert!(
            ffmpeg_make(&ctx, &["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:duration=0.04", "-frames:v", "1", &png.to_string_lossy()]),
            "生成测试 png 失败"
        );

        let cases: Vec<(&str, &str, ConvertRequest)> = vec![
            (
                "dts",
                "src.dts",
                ConvertRequest { input_file: wav.to_string_lossy().into(), format: "dts".into(), video_codec: None, audio_codec: Some("aac".into()), ..Default::default() },
            ),
            (
                "alac",
                "src.m4a",
                ConvertRequest { input_file: wav.to_string_lossy().into(), format: "alac".into(), video_codec: None, audio_codec: Some("copy".into()), ..Default::default() },
            ),
            (
                "pnm",
                "src.pnm",
                ConvertRequest { input_file: png.to_string_lossy().into(), format: "pnm".into(), ..Default::default() },
            ),
            (
                "ico",
                "src.ico",
                ConvertRequest { input_file: png.to_string_lossy().into(), format: "ico".into(), ..Default::default() },
            ),
        ];

        // 幂等：把上一次跑出来的产物清掉，否则 output_path_for 会给出 "src (1).dts" 这样的避让名
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("src") && !n.ends_with(".wav") && !n.ends_with(".png") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }

        for (name, file, req) in cases {
            let out = output_path_for(&PathBuf::from(&req.input_file), &dir, &req.format);
            assert_eq!(out.file_name().and_then(|f| f.to_str()), Some(file), "{name} 输出路径不符合预期");
            let mut ticks = Vec::new();
            let outcome = run_convert(&ctx, &req, &out, |t| ticks.push(t), |_| {}, || false)
                .unwrap_or_else(|e| panic!("{name} 转换失败：{e}"));
            assert_eq!(outcome.exit_code, 0, "{name} 退出码非 0：{}", outcome.stderr_tail);
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            assert!(size > 1000, "{name} 输出太小（{size} 字节），说明写了个空文件");
            assert_eq!(ticks.last().and_then(|t| t.percent), Some(100.0), "{name} 没有完成 tick");
            eprintln!("[真机] {name}: {}", describe(&out));
        }

        // dts 产物必须是真的 DTS 音频（而不是空壳）
        let dts = dir.join("src.dts");
        let probe = probe_media(&ctx, &dts.to_string_lossy()).expect("ffprobe 读不出 dts 产物");
        eprintln!("[真机] dts ffprobe: format={:?} codec={:?}", probe.format_name, probe.audio_codec);
        assert!(probe.audio_codec.as_deref() == Some("dts"), "DTS 产物编码不对：{probe:?}");
        // alac 产物是 M4A 容器、ALAC 编码
        let m4a = dir.join("src.m4a");
        let probe = probe_media(&ctx, &m4a.to_string_lossy()).expect("ffprobe 读不出 alac 产物");
        eprintln!("[真机] alac ffprobe: format={:?} codec={:?}", probe.format_name, probe.audio_codec);
        assert!(probe.audio_codec.as_deref() == Some("alac"), "ALAC 产物编码不对：{probe:?}");
        // pnm 必须是 P6 位图
        let pnm = dir.join("src.pnm");
        let head = std::fs::read(&pnm).unwrap();
        assert!(head.starts_with(b"P6"), "PNM 产物不是 P6 位图：{:?}", &head[..2]);
    }

    /// 真机证据：PSD 由受管 ImageMagick 写出（ffmpeg 写不出），并把真实任务 JSON 打出来给前端卡片用例
    #[test]
    #[ignore = "真机端到端：需要受管目录里的 ImageMagick"]
    fn real_machine_psd_via_imagemagick() {
        let ctx = machine_ctx();
        let caps = probe_media_caps(&ctx);
        assert!(caps.magick, "受管目录里没有 ImageMagick（bin/imagemagick/magick.exe）");
        assert!(caps.magick_can_write("PSD"), "ImageMagick 不支持写 PSD");

        let dir = proof_dir("psd");
        // 幂等：清掉上一次的产物，保证证据路径稳定在 素材.psd
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_lowercase().ends_with(".psd") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let png = dir.join("素材.png");
        assert!(
            ffmpeg_make(&ctx, &["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:duration=0.04", "-frames:v", "1", &png.to_string_lossy()]),
            "生成测试 png 失败"
        );
        let req = ConvertRequest {
            input_file: png.to_string_lossy().into(),
            format: "psd".into(),
            ..Default::default()
        };
        let out = output_path_for(&png, &dir, "psd");
        let outcome = run_convert(&ctx, &req, &out, |_| {}, |_| {}, || false)
            .unwrap_or_else(|e| panic!("PSD 转换失败：{e}"));
        assert_eq!(outcome.exit_code, 0);
        let bytes = std::fs::read(&out).expect("读不到 PSD 产物");
        assert!(bytes.starts_with(b"8BPS"), "PSD 魔数不对，产物不是真 PSD：{:?}", &bytes[..4.min(bytes.len())]);
        eprintln!("[真机] PSD: {} ", describe(&out));

        // 依赖检测里 ImageMagick 的呈现（名称 / 版本 / 来源 / 受管路径）
        for st in crate::tools::detect(&ctx) {
            if st.name == "imagemagick" {
                eprintln!(
                    "[真机] 依赖检测 imagemagick: found={} version={:?} source={} path={:?} managed={:?} hint={}",
                    st.found, st.version, st.source, st.path, st.managed_path, st.hint
                );
            }
        }

        // 前端队列卡片用例要用的真实任务 JSON（字段与 start_convert 落库的完全一致）
        let task = new_convert_task(&req, &out, 1_790_500_000_000);
        eprintln!("[真机] ConvertTask JSON = {}", serde_json::to_string(&task).unwrap());
        let in_bytes = std::fs::metadata(&png).unwrap().len();
        eprintln!("[真机] 输入字节 = {in_bytes}");
        eprintln!("[真机] 输出字节 = {}", bytes.len());
        let payload = serde_json::json!({
            "task": task,
            "input_bytes": in_bytes,
            "output_bytes": bytes.len(),
            "input_name": png.file_name().and_then(|s| s.to_str()),
            "output_name": out.file_name().and_then(|s| s.to_str()),
        });
        let _ = std::fs::write(
            dir.join("task.json"),
            serde_json::to_string_pretty(&payload).unwrap(),
        );
    }

    #[test]
    fn document_input_converts_without_ffmpeg() {
        let dir = tmp_dir("conv_doc");
        let src = dir.join("季度总结.docx");
        min_docx(&src).unwrap();
        let ctx = test_ctx(&dir); // 无 ffmpeg / ffprobe
        let out = dir.join("季度总结.md");

        let mut ticks = Vec::new();
        let outcome = run_convert(
            &ctx,
            &doc_request(&src, "md"),
            &out,
            |t| ticks.push(t),
            |_| {},
            || false,
        )
        .unwrap();

        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stderr_tail.is_empty());
        assert!(!outcome.killed);
        let md = std::fs::read_to_string(&out).unwrap();
        assert!(md.contains("# 季度总结"), "{md}");
        assert!(md.contains("| 张三 | 1234 |"), "表格：{md}");
        assert!(ticks.iter().any(|t| t.done), "完成时应发出 done tick：{ticks:?}");
        assert_eq!(ticks.last().and_then(|t| t.percent), Some(100.0));
    }

    #[test]
    fn document_input_reports_clear_error_for_legacy_binary() {
        let dir = tmp_dir("conv_legacy");
        let src = dir.join("旧报告.doc");
        std::fs::write(&src, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1").unwrap();
        let ctx = test_ctx(&dir);
        let err = run_convert(
            &ctx,
            &doc_request(&src, "md"),
            &dir.join("旧报告.md"),
            |_| {},
            |_| {},
            || false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("旧版二进制"), "应给出可执行的建议：{err}");
    }

    #[test]
    fn video_input_still_goes_through_ffmpeg() {
        let dir = tmp_dir("conv_video");
        let src = dir.join("clip.mp4");
        std::fs::write(&src, b"not a real video").unwrap();
        let ctx = test_ctx(&dir);
        let err = run_convert(
            &ctx,
            &doc_request(&src, "mp4"),
            &dir.join("clip.out.mp4"),
            |_| {},
            |_| {},
            || false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("ffmpeg"), "非文档输入仍走 ffmpeg 分支：{err}");
    }

    #[test]
    fn document_probe_maps_to_media_probe_and_auto_target() {
        let dir = tmp_dir("conv_probe");
        let ctx = test_ctx(&dir);

        let xlsx = dir.join("台账.xlsx");
        min_xlsx(&xlsx).unwrap();
        let probe = probe_media(&ctx, &xlsx.to_string_lossy()).unwrap(); // 不需要 ffprobe
        assert_eq!(probe.format_name.as_deref(), Some("document:xlsx"));
        assert!(probe.size.unwrap_or(0) > 0);
        assert!(probe.duration.is_none());

        let t = auto_target(&probe);
        assert_eq!(t.format, "csv", "电子表格自动检测应给 csv：{}", t.reason);
        assert!(t.reason.contains("xlsx"), "{}", t.reason);

        let docx = dir.join("报告.docx");
        min_docx(&docx).unwrap();
        let t = auto_target(&probe_media(&ctx, &docx.to_string_lossy()).unwrap());
        assert_eq!(t.format, "md");
        assert_eq!(t.video, "none");

        // 文档存在但内容损坏 → 返回可读错误（而不是 ffprobe 的二进制乱码）
        let broken = dir.join("broken.docx");
        std::fs::write(&broken, b"junk").unwrap();
        let err = probe_media(&ctx, &broken.to_string_lossy()).unwrap_err().to_string();
        assert!(err.len() > 4 && !err.contains("ffprobe"), "{err}");

        // 文件不存在
        let err = probe_media(&ctx, &dir.join("nope.docx").to_string_lossy()).unwrap_err().to_string();
        assert!(err.contains("文件不存在"), "{err}");
    }
}
