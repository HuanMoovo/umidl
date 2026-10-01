"""窗口 / 任务栏图标实机校验

枚举 umi Downloader 进程的全部顶层窗口，找出真正带图标的那个，
把它的图标（WM_GETICON → GCLP_HICON）与打包图标（src-tauri/icons/32x32.png）做像素比对。

用法：python scripts/verify_icon.py
"""
import os
import subprocess
import sys

from PIL import Image

OUT = '.tmp'
REF = os.path.join('src-tauri', 'icons', '32x32.png')

PS = r'''
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class WI {
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetClassLongPtr(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  public static List<IntPtr> Wins = new List<IntPtr>();
  public static uint Want = 0;
  public static bool Cb(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == Want) Wins.Add(h);
    return true;
  }
  public static string Info(IntPtr h) {
    StringBuilder t = new StringBuilder(512), c = new StringBuilder(256);
    GetWindowTextW(h, t, 512); GetClassNameW(h, c, 256);
    IntPtr ic = SendMessage(h, 0x007F, (IntPtr)1, IntPtr.Zero);
    string src = "WMICON";
    if (ic == IntPtr.Zero) { ic = GetClassLongPtr(h, -14); src = "CLASSICON"; }
    if (ic == IntPtr.Zero) { ic = GetClassLongPtr(h, -34); src = "SMALLICON"; }
    return h.ToString() + "|" + src + "|" + (ic != IntPtr.Zero ? "1" : "0") + "|" +
           (IsWindowVisible(h) ? "vis" : "hid") + "|" + c.ToString() + "|" + t.ToString() + "|" + ic.ToString();
  }
  public static void SetWant(uint p) { Want = p; Wins.Clear(); }
}
'@
$p = Get-Process umidl -ErrorAction SilentlyContinue
if (-not $p) { Write-Output "NORUN"; exit }
[WI]::SetWant($p.Id)
[WI]::EnumWindows([WI+EnumProc]{ param($h,$l) [WI]::Cb($h,$l) }, [IntPtr]::Zero) | Out-Null
$hits = @()
foreach ($h in [WI]::Wins) {
  $line = [WI]::Info($h)
  Write-Output ("WIN " + $line)
  $parts = $line.Split("|")
  if ($parts[2] -eq "1" -and $parts[3] -eq "vis") { $hits += ,$parts }
}
if ($hits.Count -eq 0) { Write-Output "NOICONWINDOW"; exit }
$best = $hits[0]
$ic = [IntPtr][int64]$best[6]
$icon = [System.Drawing.Icon]::FromHandle($ic)
$bmp = $icon.ToBitmap()
$bmp.Save("D:\Projects\umi-downloader\.tmp\window_icon_live.png", [System.Drawing.Imaging.ImageFormat]::Png)
Write-Output ("PICKED " + $best[1] + " " + $best[4] + " " + $bmp.Width + "x" + $bmp.Height + " title=" + $best[5])
'''


def run_ps(script: str) -> str:
    p = subprocess.run(
        ['powershell', '-NoProfile', '-Command', '[Console]::OutputEncoding=[Text.Encoding]::UTF8;' + script],
        capture_output=True,
    )
    return p.stdout.decode('utf-8', 'replace').strip()


def mean_diff(a: Image.Image, b: Image.Image) -> float:
    a = a.convert('RGB').resize((32, 32), Image.LANCZOS)
    b = b.convert('RGB').resize((32, 32), Image.LANCZOS)
    pa, pb = a.load(), b.load()
    total = 0.0
    for y in range(32):
        for x in range(32):
            total += sum(abs(pa[x, y][i] - pb[x, y][i]) for i in range(3)) / 3
    return total / (32 * 32)


def main() -> int:
    os.makedirs(OUT, exist_ok=True)
    out = run_ps(PS)
    for line in out.splitlines():
        if line.startswith('WIN '):
            parts = line[4:].split('|')
            print(f"  窗口 {parts[0]:>8} · {parts[1]:<9} · 图标={parts[2]} · {parts[3]} · {parts[4][:26]} · {parts[5][:24]}")
    picked = [l for l in out.splitlines() if l.startswith('PICKED')]
    if not picked:
        print('❌ 没找到带图标的可见窗口：', out.splitlines()[-1] if out else '无输出')
        return 1
    print('  选中：', picked[0])
    got = Image.open(os.path.join(OUT, 'window_icon_live.png'))
    ref = Image.open(REF)
    d = mean_diff(got, ref)
    print(f'窗口图标 vs 打包图标：平均像素差 {d:.1f} / 255')
    ok = d < 30
    z = 8
    a = got.convert('RGBA').resize((got.width * z, got.height * z), Image.NEAREST)
    b = ref.convert('RGBA').resize((ref.width * z, ref.height * z), Image.NEAREST)
    sheet = Image.new('RGB', (a.width + b.width + 60, max(a.height, b.height) + 40), (245, 245, 250))
    sheet.paste(a, (20, 20), a)
    sheet.paste(b, (a.width + 40, 20), b)
    sheet.save(os.path.join(OUT, 'window_icon_compare.png'))
    print(('✅ ' if ok else '❌ ') + '窗口图标 = 应用图标（任务栏/任务管理器显示的就是它）')
    print('对比图: .tmp/window_icon_compare.png（左=窗口实际图标，右=打包图标）')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
