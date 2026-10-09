use serde::{Deserialize, Serialize};

/* ============================ 通用 ============================ */

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Parsing,
    Downloading,
    Paused,
    Converting,
    Extracting,
    Transcribing,
    Done,
    Error,
    Canceled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Parsing => "parsing",
            TaskStatus::Downloading => "downloading",
            TaskStatus::Paused => "paused",
            TaskStatus::Converting => "converting",
            TaskStatus::Extracting => "extracting",
            TaskStatus::Transcribing => "transcribing",
            TaskStatus::Done => "done",
            TaskStatus::Error => "error",
            TaskStatus::Canceled => "canceled",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "pending" => TaskStatus::Pending,
            "parsing" => TaskStatus::Parsing,
            "downloading" => TaskStatus::Downloading,
            "paused" => TaskStatus::Paused,
            "converting" => TaskStatus::Converting,
            "extracting" => TaskStatus::Extracting,
            "transcribing" => TaskStatus::Transcribing,
            "done" => TaskStatus::Done,
            "error" => TaskStatus::Error,
            "canceled" => TaskStatus::Canceled,
            _ => TaskStatus::Pending,
        }
    }
}

/* ============================ 下载 ============================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoFormat {
    pub format_id: String,
    pub ext: String,
    pub resolution: String,
    pub fps: Option<f64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<i64>,
    pub filesize_text: Option<String>,
    pub note: Option<String>,
    pub has_video: bool,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubtitleTrack {
    pub lang: String,
    pub name: Option<String>,
    pub ext: String,
    /// 是否为自动生成字幕（YouTube 等平台）
    pub auto: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaInfo {
    pub id: String,
    pub title: String,
    /// 远程封面地址
    pub thumbnail: Option<String>,
    /// 已缓存到本地的封面路径（可离线显示 / 直接另存）
    pub thumbnail_local: Option<String>,
    pub thumbnail_width: Option<i64>,
    pub thumbnail_height: Option<i64>,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub webpage_url: String,
    pub extractor: Option<String>,
    pub description: Option<String>,
    pub filesize_approx: Option<i64>,
    pub formats: Vec<VideoFormat>,
    /// 兼容旧字段：字幕语言列表
    pub subtitles: Vec<String>,
    /// 字幕轨道明细（含自动字幕）
    pub subtitle_tracks: Vec<SubtitleTrack>,
    pub is_playlist: bool,
    pub playlist_count: Option<i64>,
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub upload_date: Option<String>,
    /// 探测耗时（毫秒），用于性能观测
    pub probe_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadTask {
    pub id: String,
    pub title: String,
    pub url: String,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub status: TaskStatus,
    pub progress: f64,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub downloaded: Option<i64>,
    pub total: Option<i64>,
    pub file_path: Option<String>,
    pub error: Option<String>,
    pub format_note: Option<String>,
    pub created_time: i64,
    /// 运行时标注：文件是否还在磁盘上（不入库，list_downloads 时计算）
    #[serde(default)]
    pub file_exists: bool,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    #[serde(default)]
    pub format_id: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub output_dir: Option<String>,
    #[serde(default)]
    pub audio_format: Option<String>,
    #[serde(default)]
    pub merge_container: Option<String>,
    #[serde(default)]
    pub embed_thumbnail: bool,
    #[serde(default)]
    pub embed_subs: bool,
    #[serde(default)]
    pub filename_template: Option<String>,
    #[serde(default)]
    pub playlist: bool,
    /// 限速（如 "500K"、"2M"），留空表示不限速
    #[serde(default)]
    pub rate_limit: Option<String>,
    /// 单独下载字幕时指定的语言（可多选），留空表示全部
    #[serde(default)]
    pub subtitle_langs: Vec<String>,
    /// 单独保存封面文件
    #[serde(default)]
    pub save_thumbnail: bool,
    /// 封面远程地址（mode = thumb 时使用，避免再次解析）
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    /// 任务显示名（封面 / 字幕等轻量任务用）
    #[serde(default)]
    pub title_hint: Option<String>,
}

impl Default for DownloadRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            format_id: None,
            mode: default_mode(),
            output_dir: None,
            audio_format: None,
            merge_container: Some("mp4".into()),
            embed_thumbnail: false,
            embed_subs: false,
            filename_template: None,
            playlist: false,
            rate_limit: None,
            subtitle_langs: Vec::new(),
            save_thumbnail: false,
            thumbnail_url: None,
            title_hint: None,
        }
    }
}

fn default_mode() -> String {
    "best".to_string()
}

/* ============================ 转换 ============================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConvertTask {
    pub id: String,
    pub input_file: String,
    pub output_file: String,
    pub format: String,
    pub codec: Option<String>,
    pub status: TaskStatus,
    pub progress: f64,
    pub error: Option<String>,
    pub created_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertRequest {
    pub input_file: String,
    #[serde(default)]
    pub output_dir: Option<String>,
    pub format: String,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub bitrate: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub crf: Option<i64>,
    #[serde(default)]
    pub extract_audio: bool,
    #[serde(default)]
    pub mute: bool,
    /* ---- 更多转换选项 ---- */
    #[serde(default)]
    pub audio_bitrate: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<i64>,
    #[serde(default)]
    pub channels: Option<i64>,
    #[serde(default)]
    pub fps: Option<i64>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub hwaccel: bool,
    #[serde(default)]
    pub faststart: bool,
    #[serde(default)]
    pub remove_metadata: bool,
    /// 自动检测出的最佳目标格式（仅用于回显，不参与转码）
    #[serde(default)]
    pub auto_format: Option<String>,
}

impl Default for ConvertRequest {
    fn default() -> Self {
        Self {
            input_file: String::new(),
            output_dir: None,
            format: "mp4".into(),
            video_codec: Some("libx264".into()),
            audio_codec: Some("aac".into()),
            bitrate: None,
            resolution: None,
            crf: Some(23),
            extract_audio: false,
            mute: false,
            audio_bitrate: None,
            sample_rate: None,
            channels: None,
            fps: None,
            speed: None,
            hwaccel: false,
            faststart: true,
            remove_metadata: false,
            auto_format: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaProbe {
    pub path: String,
    pub format_name: Option<String>,
    pub duration: Option<f64>,
    pub size: Option<i64>,
    pub bit_rate: Option<i64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
}

/* ============================ 字幕 ============================ */

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubtitleTask {
    pub id: String,
    pub video_path: String,
    pub language: String,
    pub model: String,
    pub subtitle_path: Option<String>,
    /// 本次任务是否要求「额外生成一份英文（译文）」及其目标语言。
    /// 队列行据此标出「→ 英文」，重启后也能认出来（后端从 request 列回填）。
    #[serde(default)]
    pub translate_to: Option<String>,
    pub status: TaskStatus,
    pub progress: f64,
    pub error: Option<String>,
    pub created_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleRequest {
    pub video_path: String,
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub translate_to: Option<String>,
    #[serde(default = "default_sub_format")]
    pub output_format: String,
    #[serde(default)]
    pub output_dir: Option<String>,
}

fn default_lang() -> String {
    "auto".into()
}
fn default_sub_format() -> String {
    "srt".into()
}

/* ============================ 工具 / 设置 ============================ */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: String, // bundled | system | managed | missing
    pub managed_path: Option<String>,
    pub hint: String,
    pub size_hint: Option<String>,
    /// 实际使用的下载源主机名（一键下载的工具/模型才有；用来告诉用户「走的哪个源」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 本平台是否能自动下载安装（false = 没有官方静态包，界面不显示「下载」按钮）
    #[serde(default)]
    pub installable: bool,
}

/// 单个工具的安装 / 校验结果：批量「下载所选」逐条返回，一条失败不影响其它工具
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstallOutcome {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolStatus>,
    /// 失败原因（带稳定错误码，前端可本地化）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolInstallOutcome {
    pub fn done(name: &str, status: ToolStatus) -> Self {
        Self { name: name.to_string(), ok: true, status: Some(status), error: None }
    }
    pub fn failed(name: &str, error: impl Into<String>) -> Self {
        Self { name: name.to_string(), ok: false, status: None, error: Some(error.into()) }
    }
}

/// 受管工具目录现状（设置页回显「生效目录」）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDirInfo {
    /// 生效目录（绝对路径，界面直接回显这个值）
    pub dir: String,
    /// 默认目录（数据目录下的 bin/）
    pub default_dir: String,
    /// 设置里写的原始值（空 = 使用默认目录）
    pub configured: String,
    /// 是否自定义目录
    pub custom: bool,
    /// 设置里的值是否合法（非法时运行时已回退默认目录，界面据此提示）
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 更换工具目录的结果（含迁移报告：搬了什么、删了什么、剩了什么）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDirChange {
    pub info: ToolDirInfo,
    /// 变更前的生效目录
    pub previous_dir: String,
    /// 是否执行了迁移
    pub migrated: bool,
    pub files_copied: usize,
    pub bytes_copied: u64,
    /// 新目录里已存在（同体积，未重复复制）的条目数
    pub already_present: usize,
    /// 已从旧目录删除的条目数
    pub removed_old: usize,
    /// 旧目录里仍存在（删除失败 / 被占用）的条目
    pub leftovers: Vec<String>,
    /// 未迁移的条目（例如半截的 *.downloading 残片）
    pub skipped: Vec<String>,
}

/// Whisper 语音模型（内置 + 自定义），供设置页展示「已下载 / 未下载 / 当前使用」
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperModelInfo {
    /// 存进设置的标识：内置用短名（base），自定义用文件名（ggml-xxx.bin）
    pub name: String,
    /// 磁盘文件名
    pub file: String,
    pub builtin: bool,
    pub downloaded: bool,
    pub size_bytes: u64,
    /// 预计下载体积（人类可读）
    pub size_hint: String,
    /// 质量星级（自定义模型为空）
    pub quality: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)] // 旧版本配置文件缺字段时按默认值补全，避免升级后设置被重置
pub struct AppSettings {
    pub download_dir: String,
    pub ytdlp_path: Option<String>,
    pub ffmpeg_path: Option<String>,
    pub whisper_path: Option<String>,
    pub whisper_model: String,
    /// dark | light（「跟随系统」已移除：历史 'system' 值在读取时回退 dark）
    pub theme: String,
    /// 主题强调色
    #[serde(default = "default_accent")]
    pub accent: String,
    /// 界面语言：zh | en | ja | fr
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    /// 自定义 LOGO 图片路径（空 = 使用内置矢量 LOGO）
    #[serde(default)]
    pub custom_logo: Option<String>,
    /// 同时下载任务数（1..=8；越界值在读取时自动限幅）
    #[serde(default = "default_concurrency", deserialize_with = "clamp_concurrency")]
    pub concurrency: i64,
    pub proxy: Option<String>,
    pub cookies_file: Option<String>,
    pub keep_original: bool,
    /* ---- 下载默认值 ---- */
    #[serde(default = "default_container")]
    pub default_container: String,
    #[serde(default = "default_audio_fmt")]
    pub default_audio_format: String,
    #[serde(default = "default_quality")]
    pub default_quality: String,
    #[serde(default)]
    pub filename_template: Option<String>,
    #[serde(default)]
    pub rate_limit: Option<String>,
    /// 解析完成后自动加入下载
    #[serde(default = "default_true")]
    pub auto_start: bool,
    /// 完成后自动打开下载目录
    #[serde(default)]
    pub open_folder_when_done: bool,
    #[serde(default = "default_true")]
    pub notify_on_finish: bool,
    /* ---- 转换默认值 ---- */
    #[serde(default = "default_resolution")]
    pub default_resolution: String,
    #[serde(default = "default_crf")]
    pub default_crf: i64,
    /// 自动检测输入并推荐输出格式
    #[serde(default = "default_true")]
    pub auto_detect_format: bool,
    #[serde(default)]
    pub hwaccel: bool,
    /* ---- 字幕默认值 ---- */
    #[serde(default = "default_lang")]
    pub subtitle_language: String,
    #[serde(default = "default_sub_format")]
    pub subtitle_format: String,
    /* ---- 界面 / 性能 ---- */
    /// 背景粒子动画（关闭可显著降低占用）
    #[serde(default = "default_true")]
    pub animation: bool,
    /// low | medium | high
    #[serde(default = "default_anim_quality")]
    pub animation_quality: String,
    #[serde(default)]
    pub compact_cards: bool,
    /* ---- 外观 / 布局（1.4 新增） ---- */
    /// 背景粒子形状：circle | diamond | square
    #[serde(default = "default_particle_shape")]
    pub particle_shape: String,
    /// 下载队列布局：card | table
    #[serde(default = "default_queue_layout")]
    pub queue_layout: String,
    /// 自定义强调色 #RRGGBB（空 = 使用预设 accent）
    #[serde(default)]
    pub accent_custom: String,
    /* ---- 系统集成（1.4 新增） ---- */
    /// 开机自启动
    #[serde(default)]
    pub launch_at_login: bool,
    /// 启动时检查更新
    #[serde(default = "default_true")]
    pub check_update_on_start: bool,
    /* ---- 全局限速（令牌桶，1.4 新增） ---- */
    #[serde(default)]
    pub speed_limit_enabled: bool,
    /// KB/s；<=0 表示不限速
    #[serde(default)]
    pub speed_limit_kb: i64,
    /* ---- 引擎 / 协议（1.4 新增） ---- */
    /// auto | ytdlp | aria2
    #[serde(default = "default_engine")]
    pub engine: String,
    /// single | playlist（分P / 合集）
    #[serde(default = "default_playlist_mode")]
    pub playlist_mode: String,
    /// system | custom | off
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: String,
    /* ---- 多线程 / 连接数（1.6 新增，全部带限幅） ---- */
    /// aria2 每服务器连接数（--max-connection-per-server，1..=16）
    #[serde(default = "default_aria2_connections", deserialize_with = "clamp_aria2_connections")]
    pub aria2_connections: i64,
    /// aria2 分段数（--split，1..=16）
    #[serde(default = "default_aria2_split", deserialize_with = "clamp_aria2_split")]
    pub aria2_split: i64,
    /// aria2 最小分片 MB（--min-split-size，1..=64，传给 aria2 时 *1024*1024）
    #[serde(default = "default_aria2_min_split_mb", deserialize_with = "clamp_aria2_min_split_mb")]
    pub aria2_min_split_mb: i64,
    /// yt-dlp 分片并发（--concurrent-fragments，1..=16）
    #[serde(default = "default_ytdlp_concurrency", deserialize_with = "clamp_ytdlp_concurrency")]
    pub ytdlp_concurrency: i64,
    /* ---- 受管工具目录（1.9 新增） ---- */
    /// 受管工具安装目录（绝对路径；空 = 数据目录下的 bin/）。
    /// 这是工具解析 / 调用 / 安装 / 清理的**唯一来源**，不改系统 PATH。
    /// 非法值（相对路径 / 乱码）不会被信任：运行时统一回落到默认目录（见 tools::effective_tool_dir）。
    #[serde(default)]
    pub tool_dir: String,
    /* ---- 界面模式 / 外观风格（1.10 新增） ---- */
    /// 界面模式：simple 简单（默认，只显示常用项） | advanced 高级（全部可见）
    #[serde(default = "default_ui_mode")]
    pub ui_mode: String,
    /// 外观风格：glass 玻璃拟态（默认） | mono 黑白简约（纯黑 / 纯白、去模糊）
    #[serde(default = "default_appearance_style")]
    pub appearance_style: String,
    /// 自定义渐变强调色 "起色,止色"（两个 #RRGGBB；空 = 不用渐变）
    #[serde(default)]
    pub accent_gradient: String,
}

/* ---- 多线程 / 连接数：取值范围（UI 与调度共用，改动只需改这里） ---- */
/// 同时任务数范围
pub const CONCURRENCY_RANGE: (i64, i64) = (1, 8);
/// aria2 每服务器连接数范围
pub const ARIA2_CONNECTIONS_RANGE: (i64, i64) = (1, 16);
/// aria2 分段数范围
pub const ARIA2_SPLIT_RANGE: (i64, i64) = (1, 16);
/// aria2 最小分片 MB 范围
pub const ARIA2_MIN_SPLIT_MB_RANGE: (i64, i64) = (1, 64);
/// yt-dlp 分片并发范围
pub const YTDLP_CONCURRENCY_RANGE: (i64, i64) = (1, 16);

fn clamp_range(v: i64, range: (i64, i64)) -> i64 {
    v.clamp(range.0, range.1)
}

/// 读取设置时限幅：越界值（含 0 / 负数 / 超大值）一律夹到合法区间，
/// 保证旧配置文件、手改 json、前端异常输入都不会把引擎参数带崩。
fn clamp_concurrency<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(clamp_range(Deserialize::deserialize(d)?, CONCURRENCY_RANGE))
}
fn clamp_aria2_connections<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(clamp_range(Deserialize::deserialize(d)?, ARIA2_CONNECTIONS_RANGE))
}
fn clamp_aria2_split<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(clamp_range(Deserialize::deserialize(d)?, ARIA2_SPLIT_RANGE))
}
fn clamp_aria2_min_split_mb<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(clamp_range(Deserialize::deserialize(d)?, ARIA2_MIN_SPLIT_MB_RANGE))
}
fn clamp_ytdlp_concurrency<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(clamp_range(Deserialize::deserialize(d)?, YTDLP_CONCURRENCY_RANGE))
}

fn default_aria2_connections() -> i64 {
    ARIA2_CONNECTIONS_RANGE.1
}
fn default_aria2_split() -> i64 {
    ARIA2_SPLIT_RANGE.1
}
fn default_aria2_min_split_mb() -> i64 {
    ARIA2_MIN_SPLIT_MB_RANGE.0
}
fn default_ytdlp_concurrency() -> i64 {
    YTDLP_CONCURRENCY_RANGE.0
}
fn default_concurrency() -> i64 {
    3
}

fn default_particle_shape() -> String {
    "circle".into()
}
fn default_queue_layout() -> String {
    "card".into()
}
fn default_engine() -> String {
    "auto".into()
}
fn default_playlist_mode() -> String {
    "single".into()
}
fn default_proxy_mode() -> String {
    "system".into()
}

fn default_accent() -> String {
    "violet".into()
}
fn default_ui_language() -> String {
    "zh".into()
}
fn default_container() -> String {
    "mp4".into()
}
fn default_audio_fmt() -> String {
    "mp3".into()
}
fn default_quality() -> String {
    "best".into()
}
fn default_resolution() -> String {
    "原分辨率".into()
}
fn default_crf() -> i64 {
    23
}
fn default_true() -> bool {
    true
}
fn default_anim_quality() -> String {
    "medium".into()
}
fn default_ui_mode() -> String {
    "simple".into()
}
fn default_appearance_style() -> String {
    "glass".into()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            download_dir: String::new(),
            ytdlp_path: None,
            ffmpeg_path: None,
            whisper_path: None,
            whisper_model: "base".into(),
            theme: "dark".into(),
            accent: default_accent(),
            ui_language: default_ui_language(),
            custom_logo: None,
            concurrency: 3,
            proxy: None,
            cookies_file: None,
            keep_original: true,
            default_container: default_container(),
            default_audio_format: default_audio_fmt(),
            default_quality: default_quality(),
            filename_template: None,
            rate_limit: None,
            auto_start: true,
            open_folder_when_done: false,
            notify_on_finish: true,
            default_resolution: default_resolution(),
            default_crf: default_crf(),
            auto_detect_format: true,
            hwaccel: false,
            subtitle_language: default_lang(),
            subtitle_format: default_sub_format(),
            animation: true,
            animation_quality: default_anim_quality(),
            compact_cards: false,
            particle_shape: default_particle_shape(),
            queue_layout: default_queue_layout(),
            accent_custom: String::new(),
            launch_at_login: false,
            check_update_on_start: true,
            speed_limit_enabled: false,
            speed_limit_kb: 0,
            engine: default_engine(),
            playlist_mode: default_playlist_mode(),
            proxy_mode: default_proxy_mode(),
            aria2_connections: default_aria2_connections(),
            aria2_split: default_aria2_split(),
            aria2_min_split_mb: default_aria2_min_split_mb(),
            ytdlp_concurrency: default_ytdlp_concurrency(),
            // 工具目录默认空 = 使用「数据目录/bin」（见 tools::effective_tool_dir）
            tool_dir: String::new(),
            ui_mode: default_ui_mode(),
            appearance_style: default_appearance_style(),
            accent_gradient: String::new(),
        }
    }
}

/* ============================ 自检 ============================ */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub id: String,
    pub group: String,
    pub name: String,
    pub status: String, // pending | running | pass | fail | skip
    pub detail: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestReport {
    pub started_at: i64,
    pub finished_at: i64,
    pub total: i64,
    pub passed: i64,
    pub failed: i64,
    pub skipped: i64,
    pub cases: Vec<TestCase>,
}

/* ============================ 测试 ============================ */

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧版 settings.json（没有多线程/连接数字段）必须能读，且取默认值
    #[test]
    fn legacy_settings_get_new_thread_defaults() {
        let legacy = r#"{"download_dir":"D:\\Videos","whisper_model":"small","theme":"light","concurrency":5}"#;
        let s: AppSettings = serde_json::from_str(legacy).expect("旧配置必须能读");
        assert_eq!(s.download_dir, "D:\\Videos");
        assert_eq!(s.concurrency, 5, "合法旧值应原样保留");
        assert_eq!(s.aria2_connections, 16, "每服务器连接数默认 16");
        assert_eq!(s.aria2_split, 16, "分段数默认 16");
        assert_eq!(s.aria2_min_split_mb, 1, "最小分片默认 1MB");
        assert_eq!(s.ytdlp_concurrency, 1, "yt-dlp 分片并发默认 1");

        // 完全空的 json → 走 Default
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        let d = AppSettings::default();
        assert_eq!(
            (s.aria2_connections, s.aria2_split, s.aria2_min_split_mb, s.ytdlp_concurrency),
            (d.aria2_connections, d.aria2_split, d.aria2_min_split_mb, d.ytdlp_concurrency)
        );
        assert_eq!(s.concurrency, 3, "同时任务数默认 3");
    }

    /// 越界值一律限幅：0 / 负数 / 超大值都不能带崩引擎参数
    #[test]
    fn settings_clamp_out_of_range_threads() {
        let s: AppSettings = serde_json::from_str(
            r#"{"aria2_connections":0,"aria2_split":999,"aria2_min_split_mb":999,"ytdlp_concurrency":-3,"concurrency":999}"#,
        )
        .unwrap();
        assert_eq!(s.aria2_connections, 1, "0 → 下限 1");
        assert_eq!(s.aria2_split, 16, "999 → 上限 16");
        assert_eq!(s.aria2_min_split_mb, 64, "999 → 上限 64MB");
        assert_eq!(s.ytdlp_concurrency, 1, "-3 → 下限 1");
        assert_eq!(s.concurrency, 8, "同时任务数上限 8");

        let s: AppSettings = serde_json::from_str(r#"{"concurrency":0,"aria2_connections":-9}"#).unwrap();
        assert_eq!(s.concurrency, 1, "0 → 下限 1（不再退化为默认 3）");
        assert_eq!(s.aria2_connections, 1);

        // 区间内的值原样保留
        let s: AppSettings =
            serde_json::from_str(r#"{"aria2_connections":4,"aria2_split":6,"aria2_min_split_mb":8,"ytdlp_concurrency":12,"concurrency":3}"#)
                .unwrap();
        assert_eq!(
            (s.aria2_connections, s.aria2_split, s.aria2_min_split_mb, s.ytdlp_concurrency),
            (4, 6, 8, 12)
        );
        assert_eq!(s.concurrency, 3);
    }

    /// 序列化 → 反序列化往返：设置值必须原样落盘（供设置页回显）
    #[test]
    fn settings_roundtrip_keeps_thread_values() {
        let mut s = AppSettings::default();
        s.aria2_connections = 12;
        s.aria2_split = 8;
        s.aria2_min_split_mb = 32;
        s.ytdlp_concurrency = 5;
        let text = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.aria2_connections, 12);
        assert_eq!(back.aria2_split, 8);
        assert_eq!(back.aria2_min_split_mb, 32);
        assert_eq!(back.ytdlp_concurrency, 5);
    }

    /// 界面模式 / 外观风格 / 自定义渐变：老配置缺字段取默认（simple / glass / 空），显式值原样往返
    #[test]
    fn settings_ui_mode_and_appearance_defaults() {
        // 老配置（完全没有这三个字段）→ 默认 simple / glass / 空渐变
        let s: AppSettings = serde_json::from_str(r#"{"download_dir":"D:\\Videos"}"#).unwrap();
        assert_eq!(s.ui_mode, "simple", "界面模式默认 simple");
        assert_eq!(s.appearance_style, "glass", "外观风格默认 glass");
        assert_eq!(s.accent_gradient, "", "渐变默认空串");

        // 完全空的 json → 走 Default，与逐字段默认值一致
        let d = AppSettings::default();
        assert_eq!(d.ui_mode, "simple");
        assert_eq!(d.appearance_style, "glass");
        assert_eq!(d.accent_gradient, "");

        // 显式值往返不丢
        let s: AppSettings =
            serde_json::from_str(r##"{"ui_mode":"advanced","appearance_style":"mono","accent_gradient":"#7c4dff,#22d3ee"}"##)
                .unwrap();
        assert_eq!(s.ui_mode, "advanced");
        assert_eq!(s.appearance_style, "mono");
        assert_eq!(s.accent_gradient, "#7c4dff,#22d3ee");
        let back: AppSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(
            (back.ui_mode, back.appearance_style, back.accent_gradient),
            ("advanced".into(), "mono".into(), "#7c4dff,#22d3ee".into())
        );
    }

    /// 任务状态字符串往返；老库里出现未知状态时仍回落 pending（不 panic、不丢任务）
    #[test]
    fn status_roundtrip_and_unknown_fallback() {
        for s in [
            TaskStatus::Pending,
            TaskStatus::Parsing,
            TaskStatus::Downloading,
            TaskStatus::Paused,
            TaskStatus::Converting,
            TaskStatus::Extracting,
            TaskStatus::Transcribing,
            TaskStatus::Done,
            TaskStatus::Error,
            TaskStatus::Canceled,
        ] {
            assert_eq!(TaskStatus::from_str(s.as_str()), s);
        }
        assert_eq!(TaskStatus::from_str("weird-future-state"), TaskStatus::Pending);
    }
}
