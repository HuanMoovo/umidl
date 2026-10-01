"""补齐带插值的文案：改用 vue-i18n 命名占位符（{name}/{n}/{count}...）。"""

import io
import os

EDITS = [
    # ── TaskCard
    ("src/components/TaskCard.vue",
     '<span v-if="eta">剩余 {{ eta }}</span>',
     '<span v-if="eta">{{ $t(\'剩余 {t}\', { t: eta }) }}</span>'),

    # ── TitleBar 标题走路由 meta（中文原文即 key）
    ("src/components/TitleBar.vue",
     '<h1 class="s-text text-[15px] font-semibold tracking-wide">{{ title }}</h1>',
     '<h1 class="s-text text-[15px] font-semibold tracking-wide">{{ $t(title) }}</h1>'),

    # ── Download
    ("src/views/Download.vue",
     "${t.auto ? ' · 自动' : ''}",
     "${t.auto ? tr(' · 自动') : ''}"),
    ("src/views/Download.vue",
     "message.success(`解析成功：${res.title}`)",
     "message.success(tr('解析成功：{title}', { title: res.title }))"),
    ("src/views/Download.vue",
     "message.success(`已加入${label}下载任务`)",
     "message.success(tr('已加入{label}下载任务', { label }))"),
    ("src/views/Download.vue",
     '<span v-if="bestFormatBytes"> · 最高画质 {{ bestFormatBytes }}</span>',
     '<span v-if="bestFormatBytes">{{ $t(\' · 最高画质 {size}\', { size: bestFormatBytes }) }}</span>'),
    ("src/views/Download.vue",
     '<span v-if="probeMs"> · 解析 {{ probeMs }}ms</span>',
     '<span v-if="probeMs">{{ $t(\' · 解析 {ms}ms\', { ms: probeMs }) }}</span>'),
    ("src/views/Download.vue",
     '播放 {{ formatCount(info.view_count) }}',
     '{{ $t(\'播放 {count}\', { count: formatCount(info.view_count) }) }}'),
    ("src/views/Download.vue",
     '点赞 {{ formatCount(info.like_count) }}',
     '{{ $t(\'点赞 {count}\', { count: formatCount(info.like_count) }) }}'),
    ("src/views/Download.vue",
     '字幕 {{ info.subtitle_tracks.length }} 种',
     '{{ $t(\'字幕 {count} 种\', { count: info.subtitle_tracks.length }) }}'),
    ("src/views/Download.vue",
     "`${subLangs.length}/${info.subtitle_tracks.length} 种`",
     "$t('{n}/{total} 种', { n: subLangs.length, total: info.subtitle_tracks.length })"),
    ("src/views/Download.vue",
     '封面 {{ info.thumbnail_width }}×{{ info.thumbnail_height }}',
     '{{ $t(\'封面 {w}×{h}\', { w: info.thumbnail_width, h: info.thumbnail_height }) }}'),
    ("src/views/Download.vue",
     '可选清晰度（{{ formatOptions.length }}）',
     '{{ $t(\'可选清晰度（{count}）\', { count: formatOptions.length }) }}'),
    ("src/views/Download.vue",
     '字幕轨道（{{ info.subtitle_tracks.length }}）',
     '{{ $t(\'字幕轨道（{count}）\', { count: info.subtitle_tracks.length }) }}'),
    ("src/views/Download.vue",
     "{ v: 'video', t: '指定清晰度' },\n                  { v: 'best', t: '最佳画质' },\n                  { v: 'audio', t: '仅音频' },",
     "{ v: 'video', t: $t('指定清晰度') },\n                  { v: 'best', t: $t('最佳画质') },\n                  { v: 'audio', t: $t('仅音频') },"),

    # ── Subtitle
    ("src/views/Subtitle.vue",
     "label: cur.toLowerCase().endsWith('.bin') ? `自定义 · ${cur}` : cur,",
     "label: cur.toLowerCase().endsWith('.bin') ? tr('自定义 · {name}', { name: cur }) : cur,"),
    ("src/views/Subtitle.vue",
     '<span v-else>尚未下载语音模型（{{ model }}），请在设置页下载后再使用。</span>',
     '<span v-else>{{ $t(\'尚未下载语音模型（{model}），请在设置页下载后再使用。\', { model }) }}</span>'),
    ("src/views/Subtitle.vue",
     ":subtitle=\"`${t.language === 'auto' ? '自动检测' : t.language} · ${t.model}`\"",
     ":subtitle=\"`${t.language === 'auto' ? $t('自动检测') : t.language} · ${t.model}`\""),

    # ── Converter
    ("src/views/Converter.vue",
     "({ label: s === 1 ? '原速' : `${s}x`, value: s })",
     "({ label: s === 1 ? tr('原速') : `${s}x`, value: s })"),
    ("src/views/Converter.vue",
     "({ label: c === 'copy' ? 'copy（不重编码 / 最快）' : c, value: c as string })",
     "({ label: c === 'copy' ? tr('copy（不重编码 / 最快）') : c, value: c as string })"),
    ("src/views/Converter.vue",
     "({ label: c === 'copy' ? 'copy（不重编码）' : c, value: c as string })",
     "({ label: c === 'copy' ? tr('copy（不重编码）') : c, value: c as string })"),

    # ── Settings：消息与选项
    ("src/views/Settings.vue",
     "message.success(`已切换到 ${m.builtin ? m.name : m.file}`)",
     "message.success(tr('已切换到 {name}', { name: m.builtin ? m.name : m.file }))"),
    ("src/views/Settings.vue",
     "message.success(`${m.name} 模型已下载并启用`)",
     "message.success(tr('{name} 模型已下载并启用', { name: m.name }))"),
    ("src/views/Settings.vue",
     "message.success(`已删除 ${m.file}`)",
     "message.success(tr('已删除 {name}', { name: m.file }))"),
    ("src/views/Settings.vue",
     "message.success(`已导入自定义模型${file ? `并切换为 ${file}` : ''}`)",
     "message.success(file ? tr('已导入自定义模型并切换为 {name}', { name: file }) : tr('已导入自定义模型'))"),
    ("src/views/Settings.vue",
     "message.success(`${name} 安装完成`)",
     "message.success(tr('{name} 安装完成', { name }))"),
    ("src/views/Settings.vue",
     "{ k: 'tools', label: '依赖工具' },\n          { k: 'general', label: '通用设置' },\n          { k: 'appearance', label: '外观' },\n          { k: 'defaults', label: '默认参数' },",
     "{ k: 'tools', label: $t('依赖工具') },\n          { k: 'general', label: $t('通用设置') },\n          { k: 'appearance', label: $t('外观') },\n          { k: 'defaults', label: $t('默认参数') },"),
    ("src/views/Settings.vue",
     "当前模型 {{ settings.whisper_model }} 的文件不存在，字幕识别会失败 —— 请点「下载」或切换到已下载的模型。",
     "{{ $t('当前模型 {model} 的文件不存在，字幕识别会失败 —— 请点「下载」或切换到已下载的模型。', { model: settings.whisper_model }) }}"),
    ("src/views/Settings.vue",
     "已下载 · 占用 {{ formatBytes(m.size_bytes) }}",
     "{{ $t('已下载 · 占用 {size}', { size: formatBytes(m.size_bytes) }) }}"),
    ("src/views/Settings.vue",
     '<span v-else class="s-text-3 text-[10px]">未下载 · 约 {{ m.size_hint }}</span>',
     '<span v-else class="s-text-3 text-[10px]">{{ $t(\'未下载 · 约 {size}\', { size: m.size_hint }) }}</span>'),
    ("src/views/Settings.vue",
     "{{ m.builtin ? `${m.quality} · 越大越准，速度越慢` : $t('自定义模型') }}",
     "{{ m.builtin ? $t('{stars} · 越大越准，速度越慢', { stars: m.quality }) : $t('自定义模型') }}"),
    ("src/views/Settings.vue",
     "{ label: '低（省电，粒子最少）', value: 'low' },\n              { label: '中（推荐）', value: 'medium' },\n              { label: '高（粒子最多）', value: 'high' },",
     "{ label: $t('低（省电，粒子最少）'), value: 'low' },\n              { label: $t('中（推荐）'), value: 'medium' },\n              { label: $t('高（粒子最多）'), value: 'high' },"),
    ("src/views/Settings.vue",
     "{ label: '最佳画质', value: 'best' },\n                { label: '优先 1080p', value: '1080' },\n                { label: '优先 720p', value: '720' },\n                { label: '优先 480p', value: '480' },\n                { label: '仅音频', value: 'audio' },",
     "{ label: $t('最佳画质'), value: 'best' },\n                { label: $t('优先 1080p'), value: '1080' },\n                { label: $t('优先 720p'), value: '720' },\n                { label: $t('优先 480p'), value: '480' },\n                { label: $t('仅音频'), value: 'audio' },"),
    ("src/views/Settings.vue",
     "['原分辨率', '1920x1080', '1280x720', '854x480'].map((r) => ({ label: r, value: r }))",
     "['原分辨率', '1920x1080', '1280x720', '854x480'].map((r) => ({ label: r === '原分辨率' ? tr('原分辨率') : r, value: r }))"),
    ("src/views/Settings.vue",
     "{ label: '接近原始画质（14）', value: 14 },\n                { label: '极速 · 体积大（18）', value: 18 },\n                { label: '标准 · 推荐（23）', value: 23 },\n                { label: '高压缩 · 体积小（28）', value: 28 },",
     "{ label: $t('接近原始画质（14）'), value: 14 },\n                { label: $t('极速 · 体积大（18）'), value: 18 },\n                { label: $t('标准 · 推荐（23）'), value: 23 },\n                { label: $t('高压缩 · 体积小（28）'), value: 28 },"),
    ("src/views/Settings.vue",
     "{ label: '自动检测', value: 'auto' },\n                { label: '中文', value: 'zh' },\n                { label: '英语', value: 'en' },\n                { label: '日语', value: 'ja' },\n                { label: '韩语', value: 'ko' },",
     "{ label: $t('自动检测'), value: 'auto' },\n                { label: $t('中文'), value: 'zh' },\n                { label: $t('英语'), value: 'en' },\n                { label: $t('日语'), value: 'ja' },\n                { label: $t('韩语'), value: 'ko' },"),

    # ── utils.ts：探针描述与自动检测理由（带插值）
    ("src/services/utils.ts",
     "`最高 ${vf[0].resolution || tr('未知')}`",
     "tr('最高 {res}', { res: vf[0].resolution || tr('未知') })"),
    ("src/services/utils.ts",
     "`${info.playlist_count} 个视频`",
     "tr('{n} 个视频', { n: info.playlist_count })"),
    ("src/services/utils.ts",
     "`视频 ${p.video_codec}`",
     "tr('视频 {codec}', { codec: p.video_codec })"),
    ("src/services/utils.ts",
     "`音频 ${p.audio_codec}`",
     "tr('音频 {codec}', { codec: p.audio_codec })"),
    ("src/services/utils.ts",
     "`${(n / 10000).toFixed(n < 100000 ? 1 : 0)}万`",
     "tr('{n}万', { n: Number((n / 10000).toFixed(n < 100000 ? 1 : 0)) })"),
    ("src/services/utils.ts",
     "`${(n / 100000000).toFixed(2)}亿`",
     "tr('{n}亿', { n: Number((n / 100000000).toFixed(2)) })"),
    ("src/services/utils.ts",
     "`${base}（自动）`",
     "tr('{base}（自动）', { base })"),
    ("src/services/utils.ts",
     "`源为无损音频（${a || tr('未知')}），推荐 FLAC 无损导出`",
     "tr('源为无损音频（{codec}），推荐 FLAC 无损导出', { codec: a || tr('未知') })"),
    ("src/services/utils.ts",
     "`源音频编码 ${a || tr('未知')} 兼容性一般，推荐转 MP3（通用性最好）`",
     "tr('源音频编码 {codec} 兼容性一般，推荐转 MP3（通用性最好）', { codec: a || tr('未知') })"),
    ("src/services/utils.ts",
     "`源为 ${v}${hasAudio ? ` + ${a}` : ''}，可直接无损封装为 MP4（不重编码，速度最快）`",
     "tr('源为 {codecs}，可直接无损封装为 MP4（不重编码，速度最快）', { codecs: `${v}${hasAudio ? ` + ${a}` : ''}` })"),
    ("src/services/utils.ts",
     "`源为 ${v}${hasAudio ? ` + ${a}` : ''}，可直接无损封装为 WebM`",
     "tr('源为 {codecs}，可直接无损封装为 WebM', { codecs: `${v}${hasAudio ? ` + ${a}` : ''}` })"),
    ("src/services/utils.ts",
     "`源编码 ${v} 通用性不足，推荐重编码为 MP4（H.264 + AAC，兼容性最好）`",
     "tr('源编码 {codec} 通用性不足，推荐重编码为 MP4（H.264 + AAC，兼容性最好）', { codec: v })"),
]


def main():
    applied, failed = 0, []
    for path, old, new in EDITS:
        s = open(path, encoding="utf-8", newline="").read()
        if old not in s:
            failed.append((path, old[:60]))
            continue
        s = s.replace(old, new)
        open(path, "w", encoding="utf-8", newline="").write(s)
        applied += 1
    print(f"应用 {applied}/{len(EDITS)}")
    for p, o in failed:
        print("  未匹配:", p, "|", o)


main()
