"""生成 1x1 透明 PNG 的 Rust 字节数组（供 selftest 构造合法图片），并直接插入新用例。"""
import re
import io
from PIL import Image

# ── 1) 生成最小合法 PNG
buf = io.BytesIO()
Image.new('RGBA', (1, 1), (0, 0, 0, 0)).save(buf, format='PNG')
png = buf.getvalue()
rows = []
for i in range(0, len(png), 12):
    rows.append('        ' + ', '.join('0x%02X' % b for b in png[i:i + 12]) + ',')
const_block = ('    /// 最小合法 PNG（1x1 透明）：自检用来验证自定义 LOGO 的落盘流程\n'
               '    const PNG_1PX: [u8; %d] = [\n%s\n    ];\n\n' % (len(png), '\n'.join(rows)))
print('PNG %d 字节' % len(png))

path = 'src-tauri/src/selftest.rs'
raw = open(path, encoding='utf-8', newline='').read()
nl = '\r\n' if '\r\n' in raw else '\n'

case = '''    case!("logo.custom", "外观", "自定义 LOGO：格式校验 / 4MB 上限 / 落盘 / 换格式清理", false, |e: &CaseEnv| {
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
        std::fs::write(&svg, b"<svg xmlns=\\"http://www.w3.org/2000/svg\\"/>").map_err(|x| x.to_string())?;
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
'''

anchor = '    case!("download.reveal_target"'
assert anchor in raw
# 在 reveal_target 用例之前插入（保持文件顺序可读）
raw = raw.replace(anchor, case.replace('\n', nl) + anchor, 1)

# PNG 常量：放到 run_all 之前（模块作用域）
if 'PNG_1PX' not in raw.split('case!(')[0]:
    m = re.search(r'pub fn run_all', raw)
    assert m, '未找到 run_all'
    raw = raw[:m.start()] + const_block.replace('\n', nl) + raw[m.start():]

open(path, 'w', encoding='utf-8', newline='').write(raw)
print('selftest.rs: 已插入 logo.custom 用例 + PNG 常量')
