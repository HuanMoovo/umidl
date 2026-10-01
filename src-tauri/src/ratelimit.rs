//! 基于令牌桶（token bucket）的全局限速器
//!
//! 设计要点：
//! - 纯数学内核 [`Bucket`] 只依赖显式时间步长，便于单元测试（不依赖真实时钟）。
//! - [`GlobalLimiter`] 是进程级单例，按字节数发令牌；`kb == 0` 表示不限速。
//! - 外部引擎（yt-dlp / aria2c）由各自的原生限速参数节流，本模块负责
//!   应用自身发起的 HTTP 请求（封面、缩略图、更新清单等），并作为
//!   全局配置的唯一来源。

use std::sync::Mutex;
use std::time::Duration;

/// 纯令牌桶内核：容量 = 1 秒的速率（允许 1 秒突发）
#[derive(Debug, Clone)]
pub struct Bucket {
    rate: f64,     // 每秒令牌数（字节）
    tokens: f64,   // 当前令牌
    capacity: f64, // 桶容量
}

impl Bucket {
    pub fn new(bytes_per_sec: u64) -> Self {
        let rate = bytes_per_sec as f64;
        Self {
            rate,
            tokens: rate,
            capacity: rate,
        }
    }

    /// 改变速率：保留已积累令牌，并按新容量夹紧
    pub fn set_rate(&mut self, bytes_per_sec: u64) {
        self.rate = bytes_per_sec as f64;
        self.capacity = self.rate;
        if self.tokens > self.capacity {
            self.tokens = self.capacity;
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate as u64
    }

    /// 推进时钟：按经过的时间补充令牌
    pub fn refill(&mut self, dt: Duration) {
        if self.rate <= 0.0 {
            return;
        }
        self.tokens = (self.tokens + self.rate * dt.as_secs_f64()).min(self.capacity);
    }

    /// 尝试取走 `n` 字节；返回还差多少字节（0 = 立即成功）
    pub fn try_take(&mut self, n: u64) -> u64 {
        let want = n as f64;
        if self.tokens >= want {
            self.tokens -= want;
            0
        } else {
            let missing = want - self.tokens;
            self.tokens = 0.0;
            missing as u64
        }
    }

    /// 取走 `n` 字节需要等待多久
    pub fn wait_for(&self, missing_bytes: u64) -> Duration {
        if self.rate <= 0.0 || missing_bytes == 0 {
            return Duration::ZERO;
        }
        Duration::from_secs_f64(missing_bytes as f64 / self.rate)
    }
}

/// 进程级限速器。`configure(kb)`：0 或负数 = 不限速。
pub struct GlobalLimiter {
    inner: Mutex<Option<Bucket>>,
    last: Mutex<std::time::Instant>,
}

/// 取锁：锁中毒（持锁线程 panic 后）也照样使用内部数据，避免连锁 panic（BUG-10）。
///
/// 限速器的临界区只有「令牌桶数值 / 上次补充时间」这类可以安全复用的状态，
/// 中毒时 `into_inner()` 取回数据远比再 panic 一次好：后者会让整个进程在
/// 某个下载线程 panic 后进入“处处 panic”的状态。
fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl GlobalLimiter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            last: Mutex::new(std::time::Instant::now()),
        }
    }

    /// 设置全局限速（KB/s；<=0 关闭限速）
    pub fn configure(&self, kb: i64) {
        let mut g = lock_or_recover(&self.inner);
        if kb <= 0 {
            *g = None;
        } else {
            let bytes = (kb as u64).saturating_mul(1024);
            match g.as_mut() {
                Some(b) => b.set_rate(bytes),
                None => *g = Some(Bucket::new(bytes)),
            }
        }
    }

    pub fn enabled(&self) -> bool {
        lock_or_recover(&self.inner).is_some()
    }

    pub fn rate_kb(&self) -> i64 {
        lock_or_recover(&self.inner)
            .as_ref()
            .map(|b| (b.rate() / 1024) as i64)
            .unwrap_or(0)
    }

    /// 阻塞式取令牌（用于同步上下文；纯计算，不真的 sleep 超过需要的时间）
    pub fn wait_time(&self, bytes: u64) -> Duration {
        let mut g = lock_or_recover(&self.inner);
        let Some(b) = g.as_mut() else {
            return Duration::ZERO;
        };
        let mut now = lock_or_recover(&self.last);
        b.refill(now.elapsed());
        *now = std::time::Instant::now();
        let missing = b.try_take(bytes);
        b.wait_for(missing)
    }

    /// 异步取令牌：限速开启时按令牌桶算法节流
    pub async fn acquire(&self, bytes: u64) {
        let wait = self.wait_time(bytes);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// 全局单例：进程内唯一限速器
pub static LIMITER: std::sync::LazyLock<GlobalLimiter> = std::sync::LazyLock::new(GlobalLimiter::new);

impl Default for GlobalLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_burst_then_throttles() {
        let mut b = Bucket::new(1000); // 1000 B/s，容量 1000
        assert_eq!(b.try_take(600), 0, "桶满时可立即取 600");
        assert_eq!(b.try_take(600), 200, "只剩 400，还差 200");
        // 补 0.2 秒 → 200 字节
        b.refill(Duration::from_millis(200));
        assert_eq!(b.try_take(200), 0, "补充后正好够");
    }

    #[test]
    fn bucket_caps_at_capacity() {
        let mut b = Bucket::new(1000);
        b.refill(Duration::from_secs(60));
        assert_eq!(b.try_take(1000), 0);
        assert_eq!(b.try_take(1), 1, "最多只能攒 1 秒的量，不能无限攒");
    }

    #[test]
    fn wait_for_is_linear() {
        let b = Bucket::new(2000);
        assert_eq!(b.wait_for(1000), Duration::from_millis(500));
        assert_eq!(b.wait_for(0), Duration::ZERO);
    }

    #[test]
    fn configure_zero_disables() {
        let l = GlobalLimiter::new();
        l.configure(512);
        assert!(l.enabled());
        assert_eq!(l.rate_kb(), 512);
        l.configure(0);
        assert!(!l.enabled(), "0 = 不限速");
        assert_eq!(l.wait_time(10_000_000), Duration::ZERO);
    }

    #[test]
    fn configure_updates_rate_in_place() {
        let l = GlobalLimiter::new();
        l.configure(1024);
        l.configure(2048);
        assert_eq!(l.rate_kb(), 2048);
        assert!(l.enabled());
    }

    /// BUG-10：持锁线程 panic 后锁中毒，旧的 `lock().unwrap()` 会让后续每次调用连锁 panic
    #[test]
    fn poisoned_lock_does_not_panic() {
        let l = GlobalLimiter::new();
        l.configure(1024);

        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 测试里不打印预期的 panic 噪音
        // 子模块可以直接访问私有字段（同 crate 的 mod tests 是其后代），无需任何 unsafe
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = l.inner.lock().unwrap();
            panic!("模拟持锁线程 panic → 锁中毒");
        }));
        assert!(r.is_err());
        let r2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = l.last.lock().unwrap();
            panic!("模拟持锁线程 panic → last 锁中毒");
        }));
        assert!(r2.is_err());
        std::panic::set_hook(prev_hook);

        assert!(l.inner.is_poisoned(), "前置条件：inner 锁必须已中毒");
        assert!(l.last.is_poisoned(), "前置条件：last 锁必须已中毒");

        // 中毒后全部公开接口照常工作（旧实现此处会 panic）
        assert!(l.enabled());
        assert_eq!(l.rate_kb(), 1024);
        assert!(l.wait_time(1024) <= Duration::from_secs(2));
        l.configure(2048);
        assert_eq!(l.rate_kb(), 2048);
        l.configure(0);
        assert!(!l.enabled());
        assert_eq!(l.wait_time(10_000_000), Duration::ZERO);
    }
}
