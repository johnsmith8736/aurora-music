//! 播放队列 —— 纯逻辑，零 I/O，可完整单测。
//!
//! # 播放顺序
//!
//! 实际播放顺序由 `order` 决定。未开启随机时它是恒等置换 `0..n`，
//! 因此**未开启随机的代码路径行为与引入随机之前完全一致**。
//!
//! 开启随机时生成一份**固定置换**并保持不变 —— 这是 FR-3.5 的要求：
//! "上一首"只是沿 `order` 后退一位，不需要任何随机数，
//! 因此语义正确、顺序稳定，不会每次点上一首跳到不同的曲目。
//!
//! # 环绕
//!
//! **环绕由循环模式决定**：`RepeatMode::All` 时 `advance()` 在末曲回到首曲、
//! `retreat()` 在首曲回到末曲；`Off` 与 `One` 都不环绕。
//!
//! `RepeatMode::One` **不影响** `advance()` / `retreat()` ——
//! 单曲循环只影响"自然结束后的自动续播"，手动点"下一首"在任何模式下
//! 都应前进。这条划分由 `PlaybackService` 负责落实。
//!
//! # 其它
//!
//! - 任何越界输入都返回 `None` 且**不修改队列状态**。
//! - `current` 始终是 `items` 的下标（不是 `order` 中的位置）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::model::TrackId;

/// 队列中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub track_id: TrackId,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub path: PathBuf,
    pub duration_ms: Option<i64>,
}

/// 循环模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    /// 顺序播放：队列播完即停。
    #[default]
    Off,
    /// 列表循环：末曲之后回到首曲。
    All,
    /// 单曲循环：自然结束后重播同一首。
    One,
}

/// 固定种子的播放顺序置换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShuffleState {
    /// `order[i]` 表示第 i 个播放的是 `items` 的哪个下标。
    order: Vec<usize>,
    /// 生成本置换所用的种子。
    ///
    /// 队列被整体替换后，旧 `order` 的下标全部失效，必须按新长度重建。
    /// 记住种子使重建是确定性的：同样的队列长度 + 同样的种子 ⇒ 同样的顺序。
    seed: u64,
}

/// 有序播放队列。
#[derive(Debug, Default, Clone)]
pub struct PlayQueue {
    items: Vec<QueueEntry>,
    current: Option<usize>,
    repeat: RepeatMode,
    /// `None` 表示未开启随机，此时播放顺序为恒等置换。
    shuffle: Option<ShuffleState>,
}

impl PlayQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用新列表整体替换队列，并把当前项设为 `start`。
    ///
    /// 越界或空列表时返回 `None`，且**不修改**已有队列。
    ///
    /// 若随机已开启，会按新长度**重建置换**（旧下标已失效）；
    /// 随机的开关状态与用户偏好保持不变。
    pub fn replace(&mut self, items: Vec<QueueEntry>, start: usize) -> Option<&QueueEntry> {
        if items.is_empty() || start >= items.len() {
            return None;
        }
        self.items = items;
        self.current = Some(start);

        if let Some(previous) = self.shuffle.take() {
            let seed = previous.seed;
            self.shuffle = Some(ShuffleState {
                order: permutation(self.items.len(), start, seed),
                seed,
            });
        }

        self.items.get(start)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[QueueEntry] {
        &self.items
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.current.and_then(|i| self.items.get(i))
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn is_shuffled(&self) -> bool {
        self.shuffle.is_some()
    }

    /// 设置循环模式。只改模式，不动当前曲目。
    pub fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// 开启或关闭随机播放。
    ///
    /// 开启时用 `seed` 生成固定置换，并**旋转使当前曲目位于首位** ——
    /// 因此开启随机不会打断正在播放的曲目。
    /// 关闭时丢弃置换，恢复到 `items` 的原顺序；当前曲目同样不变。
    pub fn set_shuffle(&mut self, enabled: bool, seed: u64) {
        if !enabled {
            self.shuffle = None;
            return;
        }

        // 空队列时只记住开关与种子，置换等 `replace` 时再生成
        let order = match self.current {
            Some(cur) => permutation(self.items.len(), cur, seed),
            None => Vec::new(),
        };
        self.shuffle = Some(ShuffleState { order, seed });
    }

    /// 前进一首。是否在末曲环绕由 `RepeatMode` 决定。
    pub fn advance(&mut self) -> Option<&QueueEntry> {
        let pos = self.play_position()?;

        let next = if pos + 1 < self.order_len() {
            pos + 1
        } else if self.wraps() {
            0
        } else {
            return None;
        };

        let item = self.item_at(next)?;
        self.current = Some(item);
        self.items.get(item)
    }

    /// 后退一首。是否在首曲环绕由 `RepeatMode` 决定。
    pub fn retreat(&mut self) -> Option<&QueueEntry> {
        let pos = self.play_position()?;

        let prev = if pos > 0 {
            pos - 1
        } else if self.wraps() {
            self.order_len() - 1
        } else {
            return None;
        };

        let item = self.item_at(prev)?;
        self.current = Some(item);
        self.items.get(item)
    }

    /// 跳到指定下标（`items` 的下标，不是播放顺序中的位置）。
    /// 越界返回 `None` 且不改变状态。
    pub fn jump(&mut self, index: usize) -> Option<&QueueEntry> {
        if index >= self.items.len() {
            return None;
        }
        self.current = Some(index);
        self.items.get(index)
    }

    pub fn has_next(&self) -> bool {
        match self.play_position() {
            None => false,
            Some(pos) => pos + 1 < self.order_len() || self.wraps(),
        }
    }

    pub fn has_previous(&self) -> bool {
        match self.play_position() {
            None => false,
            Some(pos) => pos > 0 || self.wraps(),
        }
    }

    /// 清空队列。
    ///
    /// **保留循环模式与随机开关**——它们是用户偏好，
    /// 不该因为停止播放或队列播完而丢失。
    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
        if let Some(state) = self.shuffle.as_mut() {
            state.order.clear();
        }
    }

    // ── 内部：播放顺序 ──────────────────────────────────────────

    /// 当前曲目在播放顺序中的位置。未开启随机时即 `current` 本身。
    fn play_position(&self) -> Option<usize> {
        let cur = self.current?;
        match &self.shuffle {
            None => Some(cur),
            Some(state) => state.order.iter().position(|&i| i == cur),
        }
    }

    /// 播放顺序中第 `pos` 位对应的 `items` 下标。
    fn item_at(&self, pos: usize) -> Option<usize> {
        match &self.shuffle {
            None => (pos < self.items.len()).then_some(pos),
            Some(state) => state.order.get(pos).copied(),
        }
    }

    fn order_len(&self) -> usize {
        match &self.shuffle {
            None => self.items.len(),
            Some(state) => state.order.len(),
        }
    }

    /// 是否应当环绕：仅列表循环，且队列中存在不止一首曲目。
    ///
    /// `order_len() > 1` 是必要的——单曲队列环绕等于原地不动，
    /// 让 `has_next`/`has_previous` 返回 `true` 会点亮一个按了没反应的按钮。
    fn wraps(&self) -> bool {
        self.repeat == RepeatMode::All && self.order_len() > 1
    }
}

/// 生成 `0..n` 的 Fisher-Yates 置换，并旋转使 `first` 位于首位。
fn permutation(n: usize, first: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = SplitMix64::new(seed);

    // 从后往前，每次与 [0, i] 中的随机一位交换。
    // 用取模引入的偏差量级约 (i+1)/2^64，对播放列表洗牌完全无关紧要。
    for i in (1..n).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }

    // 旋转而不是重新洗牌：既让当前曲目排到首位，又保留其余部分的随机性
    if let Some(pos) = order.iter().position(|&x| x == first) {
        order.rotate_left(pos);
    }

    order
}

/// splitmix64：无依赖的确定性伪随机数发生器。
///
/// 洗牌不需要密码学强度，只需要分布均匀，这正是 splitmix64 的用途；
/// 且它无需引入 `rand`，与本项目既有的依赖取向一致
/// （另见 `domain/ports.rs` 的 `Clock` / `RandomSource` 端口设计）。
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: TrackId, title: &str) -> QueueEntry {
        QueueEntry {
            track_id: id,
            title: title.into(),
            artist: None,
            album: None,
            path: PathBuf::from(format!("/music/{id}.mp3")),
            duration_ms: Some(1000),
        }
    }

    fn queue_of(n: usize) -> PlayQueue {
        let items: Vec<_> = (0..n).map(|i| entry(i as i64, &format!("T{i}"))).collect();
        let mut q = PlayQueue::new();
        q.replace(items, 0);
        q
    }

    /// 确定性伪随机序列，避免引入 rand 依赖
    fn lcg(seed: u64) -> impl FnMut() -> u64 {
        let mut acc = seed;
        move || {
            acc = acc
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            acc >> 33
        }
    }

    #[test]
    fn replace_rejects_empty_and_out_of_range_without_mutating() {
        let mut q = queue_of(3);
        let before = q.clone();

        assert!(q.replace(vec![], 0).is_none());
        assert_eq!(q.items(), before.items(), "空列表不得清空已有队列");

        assert!(
            q.replace(vec![entry(9, "X")], 5).is_none(),
            "越界 start 必须被拒绝"
        );
        assert_eq!(q.items(), before.items(), "越界时不得修改队列");
    }

    #[test]
    fn advance_stops_at_last_item_without_wrapping() {
        let mut q = queue_of(3);
        assert_eq!(q.current().unwrap().title, "T0");

        assert_eq!(q.advance().unwrap().title, "T1");
        assert_eq!(q.advance().unwrap().title, "T2");
        assert!(q.advance().is_none(), "末曲之后不得环绕");
        assert_eq!(
            q.current().unwrap().title,
            "T2",
            "advance 返回 None 时不应改变当前项"
        );
    }

    #[test]
    fn retreat_stops_at_first_item() {
        let mut q = queue_of(3);
        q.jump(2);
        assert_eq!(q.retreat().unwrap().title, "T1");
        assert_eq!(q.retreat().unwrap().title, "T0");
        assert!(q.retreat().is_none(), "首曲之前不得环绕");
        assert_eq!(q.current().unwrap().title, "T0");
    }

    #[test]
    fn jump_rejects_out_of_range() {
        let mut q = queue_of(3);
        q.jump(1);
        assert!(q.jump(3).is_none());
        assert!(q.jump(999).is_none());
        assert_eq!(q.current().unwrap().title, "T1", "越界跳转不得改变当前项");
        assert_eq!(q.jump(2).unwrap().title, "T2");
    }

    #[test]
    fn next_and_previous_flags_track_boundaries() {
        let mut q = queue_of(3);
        assert!(!q.has_previous(), "首曲不应有上一首");
        assert!(q.has_next());

        q.jump(1);
        assert!(q.has_previous());
        assert!(q.has_next());

        q.jump(2);
        assert!(q.has_previous());
        assert!(!q.has_next(), "末曲不应有下一首");
    }

    #[test]
    fn single_item_queue_has_no_neighbours() {
        let mut q = queue_of(1);
        assert!(!q.has_next());
        assert!(!q.has_previous());
        assert!(q.advance().is_none());
        assert!(q.retreat().is_none());
        assert_eq!(q.current().unwrap().title, "T0");
    }

    #[test]
    fn empty_queue_reports_no_current() {
        let q = PlayQueue::new();
        assert!(q.is_empty());
        assert!(q.current().is_none());
        assert!(q.current_index().is_none());
        assert!(!q.has_next());
        assert!(!q.has_previous());
    }

    #[test]
    fn replace_from_middle_starts_at_requested_index() {
        let items: Vec<_> = (0..5).map(|i| entry(i, &format!("T{i}"))).collect();
        let mut q = PlayQueue::new();
        let cur = q.replace(items, 3).expect("合法 start");
        assert_eq!(cur.title, "T3");
        assert_eq!(q.current_index(), Some(3));
        assert_eq!(q.len(), 5);
    }

    // ══════════════════════════════════════════════════════════
    // 循环模式
    // ══════════════════════════════════════════════════════════

    #[test]
    fn repeat_all_wraps_forward_at_last_item() {
        let mut q = queue_of(3);
        q.set_repeat(RepeatMode::All);

        assert_eq!(q.advance().unwrap().title, "T1");
        assert_eq!(q.advance().unwrap().title, "T2");
        assert_eq!(q.advance().unwrap().title, "T0", "列表循环应在末曲回到首曲");
    }

    #[test]
    fn repeat_all_wraps_backward_at_first_item() {
        let mut q = queue_of(3);
        q.set_repeat(RepeatMode::All);

        assert_eq!(q.retreat().unwrap().title, "T2", "列表循环应在首曲回到末曲");
        assert_eq!(q.retreat().unwrap().title, "T1");
    }

    #[test]
    fn repeat_all_keeps_flags_true_in_both_directions() {
        let mut q = queue_of(3);
        q.set_repeat(RepeatMode::All);

        for i in 0..3 {
            q.jump(i);
            assert!(q.has_next(), "列表循环下任何位置都应有下一首");
            assert!(q.has_previous(), "列表循环下任何位置都应有上一首");
        }
    }

    #[test]
    fn repeat_one_behaves_like_off_for_advance_and_retreat() {
        // 单曲循环只影响自动续播，手动前进/后退不受影响
        let mut q = queue_of(3);
        q.set_repeat(RepeatMode::One);

        assert_eq!(q.advance().unwrap().title, "T1");
        assert_eq!(q.advance().unwrap().title, "T2");
        assert!(q.advance().is_none(), "单曲循环不得让 advance 环绕");
        assert!(!q.has_next(), "末曲在单曲循环下仍无下一首");
    }

    #[test]
    fn single_item_queue_does_not_wrap_even_with_repeat_all() {
        // 环绕等于原地不动：不应点亮一个按了没反应的按钮
        let mut q = queue_of(1);
        q.set_repeat(RepeatMode::All);

        assert!(!q.has_next());
        assert!(!q.has_previous());
        assert!(q.advance().is_none());
        assert!(q.retreat().is_none());
    }

    #[test]
    fn switching_repeat_mode_does_not_move_current() {
        let mut q = queue_of(4);
        q.jump(2);
        for mode in [RepeatMode::All, RepeatMode::One, RepeatMode::Off] {
            q.set_repeat(mode);
            assert_eq!(q.current().unwrap().title, "T2");
            assert_eq!(q.repeat(), mode);
        }
    }

    // ══════════════════════════════════════════════════════════
    // 随机播放
    // ══════════════════════════════════════════════════════════

    #[test]
    fn enabling_shuffle_keeps_current_track() {
        let mut q = queue_of(6);
        q.jump(3);
        q.set_shuffle(true, 42);

        assert!(q.is_shuffled());
        assert_eq!(q.current().unwrap().title, "T3", "开启随机不应打断当前曲目");
    }

    #[test]
    fn disabling_shuffle_keeps_current_track_and_restores_order() {
        let mut q = queue_of(6);
        q.jump(3);
        q.set_shuffle(true, 42);
        q.set_shuffle(false, 0);

        assert!(!q.is_shuffled());
        assert_eq!(q.current().unwrap().title, "T3", "关闭随机不应打断当前曲目");
        // 恢复原顺序：从 T3 前进应回到 T4
        assert_eq!(q.advance().unwrap().title, "T4");
    }

    #[test]
    fn shuffle_visits_every_track_exactly_once() {
        // roadmap.md 的退出标准：随机模式下连续"下一首"覆盖全部曲目且无重复
        for seed in [1u64, 7, 12345, 0xDEAD_BEEF] {
            let mut q = queue_of(10);
            q.set_shuffle(true, seed);

            let mut seen = vec![q.current().unwrap().track_id];
            while let Some(e) = q.advance() {
                seen.push(e.track_id);
            }

            assert_eq!(seen.len(), 10, "seed={seed} 应恰好访问 10 首");
            let mut sorted = seen.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), 10, "seed={seed} 不得重复访问同一首");
        }
    }

    #[test]
    fn shuffle_previous_walks_back_the_same_path() {
        // roadmap.md 的退出标准：开启随机后"上一首"能正确回退
        let mut q = queue_of(8);
        q.set_shuffle(true, 2026);

        let start = q.current().unwrap().track_id;
        let mut forward = vec![start];
        for _ in 0..4 {
            forward.push(q.advance().unwrap().track_id);
        }

        // 原路退回
        for expected in forward.iter().rev().skip(1) {
            assert_eq!(
                q.retreat().unwrap().track_id,
                *expected,
                "上一首必须沿同一置换回退，而不是重新随机"
            );
        }
        assert_eq!(q.current().unwrap().track_id, start);
    }

    #[test]
    fn shuffle_is_deterministic_for_a_given_seed() {
        let mut a = queue_of(7);
        a.set_shuffle(true, 999);
        let mut b = queue_of(7);
        b.set_shuffle(true, 999);

        let seq_a: Vec<_> = std::iter::from_fn(|| a.advance().map(|e| e.track_id)).collect();
        let seq_b: Vec<_> = std::iter::from_fn(|| b.advance().map(|e| e.track_id)).collect();
        assert_eq!(seq_a, seq_b, "同种子必须产生同顺序（FR-3.5 的可复现要求）");
    }

    #[test]
    fn different_seeds_generally_produce_different_orders() {
        let collect = |seed: u64| {
            let mut q = queue_of(12);
            q.set_shuffle(true, seed);
            std::iter::from_fn(|| q.advance().map(|e| e.track_id)).collect::<Vec<_>>()
        };
        assert_ne!(collect(1), collect(2), "不同种子应给出不同顺序");
    }

    #[test]
    fn replacing_the_queue_rebuilds_the_permutation() {
        // 关键回归：队列换掉后旧置换的下标会越界，必须按新长度重建
        let mut q = queue_of(3);
        q.set_shuffle(true, 5);

        let smaller: Vec<_> = (0..2).map(|i| entry(100 + i, &format!("N{i}"))).collect();
        q.replace(smaller, 1);

        assert!(q.is_shuffled(), "替换队列不应关闭随机开关");
        assert_eq!(q.current().unwrap().title, "N1");

        // 置换必须只含合法下标，且覆盖新队列
        let mut seen = vec![q.current().unwrap().track_id];
        while let Some(e) = q.advance() {
            seen.push(e.track_id);
        }
        assert_eq!(seen.len(), 2, "重建后的置换必须覆盖整个新队列");
    }

    #[test]
    fn clear_preserves_mode_preferences() {
        let mut q = queue_of(3);
        q.set_repeat(RepeatMode::All);
        q.set_shuffle(true, 1);
        q.clear();

        assert_eq!(q.len(), 0);
        assert!(q.current().is_none());
        assert_eq!(q.repeat(), RepeatMode::All, "停止播放不应丢失循环模式");
        assert!(q.is_shuffled(), "停止播放不应丢失随机开关");
    }

    /// 属性测试：任意操作序列后，队列不变量必须成立（顺序模式）。
    #[test]
    fn invariants_hold_under_arbitrary_operation_sequences() {
        let mut q = queue_of(4);
        let ops: [fn(&mut PlayQueue); 4] = [
            |q| {
                q.advance();
            },
            |q| {
                q.retreat();
            },
            |q| {
                q.jump(2);
            },
            |q| {
                q.jump(99);
            },
        ];

        let mut next = lcg(0x9E37_79B9_7F4A_7C15);
        for _ in 0..500 {
            ops[(next() as usize) % ops.len()](&mut q);

            // 不变量 1：current 若存在，必须落在合法下标内
            if let Some(i) = q.current_index() {
                assert!(i < q.len(), "current={i} 越出 len={}", q.len());
                assert!(q.current().is_some(), "current 存在则必须能取到条目");
            }
            // 不变量 2：顺序模式下 has_next / has_previous 与下标一致
            if let Some(i) = q.current_index() {
                assert_eq!(q.has_next(), i + 1 < q.len());
                assert_eq!(q.has_previous(), i > 0);
            }
        }
    }

    /// 属性测试：随机 + 列表循环下，不变量必须成立。
    ///
    /// 这里的 `has_next` / `has_previous` 不再与 `items` 下标挂钩，
    /// 而是恒为真（队列长度 > 1 时）—— 这正是置换生效的证据。
    #[test]
    fn invariants_hold_under_shuffle_and_repeat_all() {
        let mut q = queue_of(5);
        q.set_repeat(RepeatMode::All);
        q.set_shuffle(true, 0x1234_5678_9ABC_DEF0);

        let ops: [fn(&mut PlayQueue); 3] = [
            |q| {
                q.advance();
            },
            |q| {
                q.retreat();
            },
            |q| {
                q.jump(3);
            },
        ];

        let mut next = lcg(0x5DEE_CE66_D0F1_1234);
        for step in 0..500 {
            ops[(next() as usize) % ops.len()](&mut q);

            assert!(q.is_shuffled(), "置换不得中途消失");
            assert_eq!(q.len(), 5, "队列不得被操作改变长度");

            let cur = q.current().expect("随机模式下 current 必须始终有效");
            assert!(q.items().contains(cur), "current 必须指向队列中真实存在的条目");
            assert!(q.has_next(), "第 {step} 步：列表循环下应始终有下一首");
            assert!(q.has_previous(), "第 {step} 步：列表循环下应始终有上一首");
        }
    }
}
