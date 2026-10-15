//! 随机源实现。
//!
//! 生产使用 `SystemRandom`，测试使用 `FakeRandom`。
//! `app/` 层只认 `RandomSource` trait，不直接读系统时钟或随机源。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::ports::RandomSource;

/// 生产随机源。
///
/// 洗牌**不需要**不可预测性 —— FR-3.5 反而要求顺序可复现，
/// 因此这里直接用纳秒时间戳经 splitmix64 打散即可：
/// 连续两次切换随机的间隔远大于 1 纳秒，必然得到不同种子。
///
/// 刻意不引入 `rand` 依赖，与本项目既有的依赖取向一致。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRandom;

impl RandomSource for SystemRandom {
    fn seed(&self) -> u64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        mix(nanos)
    }
}

/// 测试随机源：每次调用返回**可复现**的确定性序列。
///
/// 与 `FakeClock` 一样是**生产类型**（非 `#[cfg(test)]`）——
/// 它同时被单元测试与 `tests/` 下的集成测试使用，
/// 且集成测试只能看到库的公开 API。
#[derive(Debug, Default)]
pub struct FakeRandom {
    counter: AtomicU64,
}

impl FakeRandom {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RandomSource for FakeRandom {
    fn seed(&self) -> u64 {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        mix(0x9E37_79B9_7F4A_7C15 ^ n)
    }
}

/// splitmix64 的混合步骤：把低位规律性明显的输入打散成均匀分布。
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn system_random_does_not_repeat_itself() {
        // 纳秒分辨率下连续调用应当互不相同；这里放宽到"绝大多数不同"，
        // 避免在粗粒度时钟上偶发失败
        let seeds: HashSet<u64> = (0..100).map(|_| SystemRandom.seed()).collect();
        assert!(
            seeds.len() > 90,
            "SystemRandom 产生的种子重复过多: {} 个不同值",
            seeds.len()
        );
    }

    #[test]
    fn fake_random_is_deterministic() {
        let a: Vec<u64> = (0..5).map(|_| FakeRandom::new().seed()).collect();
        let b: Vec<u64> = (0..5).map(|_| FakeRandom::new().seed()).collect();
        assert_eq!(a, b, "同一序列必须可复现");
    }

    #[test]
    fn fake_random_advances_between_calls() {
        let r = FakeRandom::new();
        let first: Vec<u64> = (0..5).map(|_| r.seed()).collect();
        let unique: HashSet<u64> = first.iter().copied().collect();
        assert_eq!(unique.len(), first.len(), "连续调用必须给出不同种子");
    }

    #[test]
    fn mix_spreads_sequential_inputs() {
        // 未混合前 0 与 1 只差一位；混合后应当完全发散
        let a = mix(0);
        let b = mix(1);
        assert_ne!(a, b);
        assert!(
            (a ^ b).count_ones() > 16,
            "混合后仍高度相关: {a:#x} vs {b:#x}"
        );
    }
}
