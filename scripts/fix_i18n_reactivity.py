"""切语言要能实时刷新：把模块级 tr() 常量改成函数（否则只在模块加载时求值一次）。"""

import os

EDITS = [
    # ── theme.ts
    ("src/services/theme.ts",
     "export const THEME_MODES: { value: ThemeMode; label: string; hint: string }[] = [",
     "export function themeModes(): { value: ThemeMode; label: string; hint: string }[] {\n  return ["),
    ("src/services/theme.ts",
     "  { value: 'system', label: tr('跟随系统'), hint: tr('随系统昼夜自动切换') },\n]",
     "    { value: 'system', label: tr('跟随系统'), hint: tr('随系统昼夜自动切换') },\n  ]\n}"),
    ("src/services/theme.ts",
     "export const ACCENTS: { value: AccentName; label: string; hex: string }[] = [",
     "export function accents(): { value: AccentName; label: string; hex: string }[] {\n  return ["),
    ("src/services/theme.ts",
     "  { value: 'blue', label: tr('天空蓝'), hex: '#3b82f6' },\n]",
     "    { value: 'blue', label: tr('天空蓝'), hex: '#3b82f6' },\n  ]\n}"),

    # ── whisper.ts
    ("src/services/whisper.ts", "export const LANGUAGES = [", "export function languages() {\n  return ["),
    ("src/services/whisper.ts", "export const MODELS = [", "export function models() {\n  return ["),

    # ── ffmpeg.ts
    ("src/services/ffmpeg.ts", "export const QUALITY_PRESETS = [", "export function qualityPresets() {\n  return ["),

    # ── downloader.ts
    ("src/services/downloader.ts", "export const SUPPORTED_SITES = [", "export function supportedSites() {\n  return ["),

    # ── SideBar：导航项改 computed
    ("src/components/SideBar.vue", "const items = [", "const items = computed(() => ["),
    ("src/components/SideBar.vue",
     "  { to: '/settings', label: t('设置'), icon: SettingsOutline, key: 'settings' },\n]",
     "  { to: '/settings', label: tr('设置'), icon: SettingsOutline, key: 'settings' },\n])"),

    # ── TitleBar
    ("src/components/TitleBar.vue",
     "import { applyTheme, THEME_MODES } from '@/services/theme'",
     "import { applyTheme, themeModes } from '@/services/theme'"),
    ("src/components/TitleBar.vue",
     "const modes = THEME_MODES.map((m) => ({\n  ...m,\n  icon: m.value === 'light' ? SunnyOutline : m.value === 'dark' ? MoonOutline : ContrastOutline,\n}))",
     "const modes = computed(() =>\n  themeModes().map((m) => ({\n    ...m,\n    icon: m.value === 'light' ? SunnyOutline : m.value === 'dark' ? MoonOutline : ContrastOutline,\n  })),\n)"),

    # ── Settings
    ("src/views/Settings.vue",
     "import { ACCENTS, THEME_MODES, applyTheme } from '@/services/theme'",
     "import { accents, applyTheme, themeModes } from '@/services/theme'"),
    ("src/views/Settings.vue", 'v-for="t in THEME_MODES"', 'v-for="t in themeModes()"'),
    ("src/views/Settings.vue", 'v-for="a in ACCENTS"', 'v-for="a in accents()"'),

    # ── Subtitle
    ("src/views/Subtitle.vue",
     "import { whisper, LANGUAGES, OUTPUT_FORMATS, MODELS } from '@/services/whisper'",
     "import { whisper, languages, OUTPUT_FORMATS, models } from '@/services/whisper'"),
    ("src/views/Subtitle.vue",
     "  const list: { label: string; value: string }[] = MODELS.map((m) => ({",
     "  const list: { label: string; value: string }[] = models().map((m) => ({"),
    ("src/views/Subtitle.vue",
     "const langOptions = LANGUAGES.map((l) => ({ label: l.label, value: l.value as string }))",
     "const langOptions = computed(() => languages().map((l) => ({ label: l.label, value: l.value as string })))"),

    # ── Converter
    ("src/views/Converter.vue", "  QUALITY_PRESETS,", "  qualityPresets,"),
    ("src/views/Converter.vue",
     "const qualityOptions = QUALITY_PRESETS.map((p) => ({ label: `${p.label}（CRF ${p.crf}）`, value: p.crf }))",
     "const qualityOptions = computed(() => qualityPresets().map((p) => ({ label: `${p.label}（CRF ${p.crf}）`, value: p.crf })))"),

    # ── Home
    ("src/views/Home.vue", "import { SUPPORTED_SITES } from '@/services/downloader'", "import { supportedSites } from '@/services/downloader'"),
    ("src/views/Home.vue", 'v-for="s in SUPPORTED_SITES"', 'v-for="s in supportedSites()"'),
]


def main():
    failed = []
    for path, old, new in EDITS:
        s = open(path, encoding="utf-8", newline="").read()
        if old not in s:
            failed.append((path, old[:70]))
            continue
        open(path, "w", encoding="utf-8", newline="").write(s.replace(old, new, 1))
    print(f"应用 {len(EDITS) - len(failed)}/{len(EDITS)}")
    for p, o in failed:
        print("  未匹配:", p, "|", o)


main()
