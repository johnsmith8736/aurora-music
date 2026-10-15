//! 时间源实现。
//!
//! 生产使用 `SystemClock`，测试使用 `FakeClock`。
//! `domain/` 与 `app/` 层只认 `Clock` trait，不直接读系统时钟。

use std::sync::Mutex;

use crate::domain::ports::Clock;

/// 生产时间源。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// 测试时间源。时间只在显式 `advance` 时推进，因此依赖时间的断言完全确定。
pub struct FakeClock {
    now: Mutex<i64>,
}

impl FakeClock {
    /// 从固定时刻开始，避免测试依赖真实日期。
    pub fn new(start_unix: i64) -> Self {
        Self {
            now: Mutex::new(start_unix),
        }
    }

    pub fn advance(&self, seconds: i64) {
        let mut n = self.now.lock().unwrap_or_else(|e| e.into_inner());
        *n += seconds;
    }
}

impl Clock for FakeClock {
    fn now_unix(&self) -> i64 {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_only_moves_when_told() {
        let c = FakeClock::new(1_000_000);
        assert_eq!(c.now_unix(), 1_000_000);
        assert_eq!(c.now_unix(), 1_000_000, "读取不应改变时间");
        c.advance(60);
        assert_eq!(c.now_unix(), 1_000_060);
    }

    #[test]
    fn system_clock_is_plausible() {
        let t = SystemClock.now_unix();
        // 2020-01-01 之后、2100 年之前
        assert!(t > 1_577_836_800, "系统时钟明显偏早: {t}");
        assert!(t < 4_102_444_800, "系统时钟明显偏晚: {t}");
    }
}
