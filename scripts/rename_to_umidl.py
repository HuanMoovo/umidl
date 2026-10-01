"""把程序显示名统一改为 Umidl（1.3.3）

约定：
- 显示名 / 包名 / 窗口标题 / 安装目录 → Umidl（exe 名同时改为 umidl.exe，由 Cargo 包名决定）
- 标识符 com.umi.downloader 与数据目录 %APPDATA%/umi-downloader 保持不变 → 用户数据、设置、下载记录全部保留
- 默认下载目录 Downloads/umi Downloader → Downloads/Umidl（仅影响新装默认值，已有设置不动）
"""
import io
import os
import sys

# (文件, [(旧, 新), ...], 期望命中数)
EDITS = [
    ('src-tauri/tauri.conf.json', [
        ('"productName": "umi Downloader"', '"productName": "Umidl"'),
        ('"title": "umi Downloader"', '"title": "Umidl"'),
        ('"copyright": "umi Downloader"', '"copyright": "Umidl"'),
        ('"longDescription": "umi Downloader 是一款', '"longDescription": "Umidl 是一款'),
    ]),
    ('package.json', [
        ('"name": "umi-downloader"', '"name": "umidl"'),
        ('"description": "umi Downloader - ', '"description": "Umidl - '),
    ]),
    ('index.html', [
        ('<title>umi Downloader</title>', '<title>Umidl</title>'),
    ]),
    ('src-tauri/Cargo.toml', [
        ('name = "umi-downloader"', 'name = "umidl"'),
        ('description = "umi Downloader - ', 'description = "Umidl - '),
    ]),
    ('src/components/TitleBar.vue', [
        ("|| 'umi Downloader'", "|| 'Umidl'"),
    ]),
    ('src/components/SideBar.vue', [
        ('class="text-[13px] font-bold tracking-wide s-text">umi<', 'class="text-[13px] font-bold tracking-wide s-text">Umidl<'),
    ]),
    ('src/App.vue', [
        ("$t('正在加载 umi Downloader…')", "$t('正在加载 Umidl…')"),
    ]),
    ('src/i18n/locales/zh.ts', [
        ('"正在加载 umi Downloader…": "正在加载 umi Downloader…"', '"正在加载 Umidl…": "正在加载 Umidl…"'),
    ]),
    ('src/i18n/locales/en.ts', [
        ('"正在加载 umi Downloader…": "Loading umi Downloader…"', '"正在加载 Umidl…": "Loading Umidl…"'),
    ]),
    ('src/i18n/locales/ja.ts', [
        ('"正在加载 umi Downloader…": "umi Downloader を読み込み中…"', '"正在加载 Umidl…": "Umidl を読み込み中…"'),
    ]),
    ('src/i18n/locales/fr.ts', [
        ('"正在加载 umi Downloader…": "Chargement d\'umi Downloader…"', '"正在加载 Umidl…": "Chargement d\'Umidl…"'),
    ]),
    ('src/types/index.ts', [
        ('* umi Downloader —— 前后端共享类型定义', '* Umidl —— 前后端共享类型定义'),
    ]),
    ('src/assets/logo.svg', [
        ('aria-label="umi Downloader"', 'aria-label="Umidl"'),
        ('<title>umi Downloader</title>', '<title>Umidl</title>'),
    ]),
    ('src/assets/logo-mark.svg', [
        ('aria-label="umi Downloader"', 'aria-label="Umidl"'),
        ('<title>umi Downloader</title>', '<title>Umidl</title>'),
    ]),
    ('src-tauri/src/ctx.rs', [
        ('.join("umi Downloader")', '.join("Umidl")'),
    ]),
    ('src-tauri/src/lib.rs', [
        ('//! umi Downloader 后端核心', '//! Umidl 后端核心'),
        ('notify_finish("umi Downloader · 下载完成"', 'notify_finish("Umidl · 下载完成"'),
        ('notify_finish("umi Downloader · 转换完成"', 'notify_finish("Umidl · 转换完成"'),
        ('notify_finish("umi Downloader · 字幕生成完成"', 'notify_finish("Umidl · 字幕生成完成"'),
        ('.expect("umi Downloader 启动失败")', '.expect("Umidl 启动失败")'),
        ('println!("=== umi Downloader 后端自检 ===\\n")', 'println!("=== Umidl 后端自检 ===\\n")'),
    ]),
    ('scripts/capture_window.py', [
        ('TITLE = "umi Downloader"', 'TITLE = "Umidl"'),
    ]),
    ('scripts/click_window.py', [
        ('TITLE = "umi Downloader"', 'TITLE = "Umidl"'),
    ]),
    ('scripts/verify_icon.py', [
        ('Get-Process umi-downloader -ErrorAction', 'Get-Process umidl -ErrorAction'),
    ]),
]

# 版本 1.3.2 → 1.3.3
VERSION_FILES = ['package.json', 'src-tauri/Cargo.toml', 'src-tauri/tauri.conf.json', 'src/components/TitleBar.vue']


def main() -> int:
    bad = 0
    for path, pairs in EDITS:
        if not os.path.exists(path):
            print(f'⚠ 缺失 {path}')
            bad += 1
            continue
        raw = open(path, encoding='utf-8', newline='').read()
        for old, new in pairs:
            n = raw.count(old)
            if n != 1:
                print(f'❌ {path}: 期望命中 1 次，实际 {n} 次 → {old[:56]}')
                bad += 1
                continue
            raw = raw.replace(old, new)
        open(path, 'w', encoding='utf-8', newline='').write(raw)
        print(f'✓ {path}')
    for path in VERSION_FILES:
        raw = open(path, encoding='utf-8', newline='').read()
        if '1.3.2' in raw:
            open(path, 'w', encoding='utf-8', newline='').write(raw.replace('1.3.2', '1.3.3'))
            print(f'✓ {path} → 1.3.3')
    print('失败项:', bad)
    return 0 if bad == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
