"""点击窗口内坐标 + 截图（用于验收：切到下载页看队列封面）"""
import ctypes, sys, time
from ctypes import wintypes
from PIL import ImageGrab

u32 = ctypes.windll.user32
u32.SetProcessDPIAware()

TITLE = "Umidl"


def find_hwnd():
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
    def cb(hwnd, _):
        if u32.IsWindowVisible(hwnd):
            n = u32.GetWindowTextLengthW(hwnd)
            if n:
                buf = ctypes.create_unicode_buffer(n + 1)
                u32.GetWindowTextW(hwnd, buf, n + 1)
                if buf.value.strip() == TITLE:
                    found.append(hwnd)
        return True

    u32.EnumWindows(cb, 0)
    if not found:
        return None
    # 取面积最大的那个（避免匹配到隐藏/最小化的辅助窗口）
    def area(h):
        r = RECT()
        u32.GetWindowRect(h, ctypes.byref(r))
        return max(0, r.right - r.left) * max(0, r.bottom - r.top)

    return max(found, key=area)


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


def rect(h):
    r = RECT()
    u32.GetWindowRect(h, ctypes.byref(r))
    return r


def click(x, y):
    u32.SetCursorPos(int(x), int(y))
    time.sleep(0.15)
    u32.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
    time.sleep(0.06)
    u32.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "click"
    out = sys.argv[2] if len(sys.argv) > 2 else None
    h = find_hwnd()
    if not h:
        print("窗口未找到")
        return
    r = rect(h)
    # 最小化状态先还原，再强制置前台（SetForegroundWindow 常被系统忽略，用 ALT 轻敲解锁）
    if u32.IsIconic(h):
        u32.ShowWindow(h, 9)  # SW_RESTORE
        time.sleep(0.8)
    for _ in range(3):
        u32.SetForegroundWindow(h)
        time.sleep(0.25)
        if u32.GetForegroundWindow() == h:
            break
        # ALT 轻敲：解锁前台切换限制
        u32.keybd_event(0x12, 0, 0, 0)      # ALT down
        u32.keybd_event(0x12, 0, 0x0002, 0)  # ALT up
        u32.ShowWindow(h, 5)                 # SW_SHOW
        time.sleep(0.2)
    time.sleep(0.6)
    if u32.GetForegroundWindow() != h:
        print("警告：窗口未取得前台焦点，截图可能被其它窗口遮挡")
    if cmd == "click":
        dx, dy = int(sys.argv[3]), int(sys.argv[4])
        click(r.left + dx, r.top + dy)
        time.sleep(1.2)
    elif cmd == "move+shot" and len(sys.argv) > 4:
        dx, dy = int(sys.argv[3]), int(sys.argv[4])
        u32.SetCursorPos(r.left + dx, r.top + dy)
        time.sleep(0.4)
    if out:
        time.sleep(0.3)
        ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom), all_screens=True).save(out)
        print(f"截图 {out} · 窗口 {r.left},{r.top} {r.right - r.left}x{r.bottom - r.top}")
    else:
        print(f"窗口 {r.left},{r.top} {r.right - r.left}x{r.bottom - r.top}")


main()
