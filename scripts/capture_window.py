"""截取 umi Downloader 窗口（用于界面验证）"""
import ctypes
import ctypes.wintypes as wt
import sys
import time

from PIL import ImageGrab

TITLE = "Umidl"


def find_window(title: str):
    u = ctypes.windll.user32
    hwnd = u.FindWindowW(None, title)
    if hwnd:
        return hwnd
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)
    def cb(h, l):
        if u.IsWindowVisible(h):
            buf = ctypes.create_unicode_buffer(512)
            u.GetWindowTextW(h, buf, 512)
            if title.lower() in buf.value.lower():
                found.append(h)
                return False
        return True

    u.EnumWindows(cb, 0)
    return found[0] if found else 0


def capture(out: str, settle: float = 2.5) -> int:
    time.sleep(settle)
    u = ctypes.windll.user32
    hwnd = find_window(TITLE)
    if not hwnd:
        print("未找到窗口:", TITLE)
        return 1
    u.ShowWindow(hwnd, 9)  # SW_RESTORE
    u.SetForegroundWindow(hwnd)
    time.sleep(0.6)
    r = wt.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
    img.save(out)
    print(f"已保存 {out}  {img.size[0]}x{img.size[1]}")
    return 0


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "shot.png"
    settle = float(sys.argv[2]) if len(sys.argv) > 2 else 2.5
    raise SystemExit(capture(out, settle))
