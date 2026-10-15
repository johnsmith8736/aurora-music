//! 播放队列 —— 纯逻辑，零 I/O，可完整单测。
//!
//! # 本阶段范围
//!
//! 只实现顺序播放所需的最小能力：整队替换、前进、后退、跳转。
//! **随机播放、循环模式、持久化、拖拽重排属于下一个独立功能**
//! （roadmap M2），此处刻意不实现，避免半成品的随机逻辑污染设计。
//!
//! # 语义约定
//!
//! - 队列**不环绕**：`advance()` 在末曲返回 `None`，`retreat()` 在首曲返回 `None`。
//!   环绕与否由循环模式决定，而循环模式属于 M2。
//! - 任何越界输入都返回 `None` 且**不修改队列状态**。

use std::path::PathBuf;

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

/// 有序播放队列。
#[derive(Debug, Default, Clone)]
pub struct PlayQueue {
    items: Vec<QueueEntry>,
    current: Option<usize>,
}

impl PlayQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用新列表整体替换队列，并把当前项设为 `start`。
    ///
    /// 越界或空列表时返回 `None`，且**不修改**已有队列。
    pub fn replace(&mut self, items: Vec<QueueEntry>, start: usize) -> Option<&QueueEntry> {
        if items.is_empty() || start >= items.len() {
            return None;
        }
        self.items = items;
        self.current = Some(start);
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

    /// 前进一首。已在末曲则返回 `None`（不环绕）。
    pub fn advance(&mut self) -> Option<&QueueEntry> {
        let next = self.current.map(|i| i + 1).filter(|i| *i < self.items.len())?;
        self.current = Some(next);
        self.items.get(next)
    }

    /// 后退一首。已在首曲则返回 `None`。
    pub fn retreat(&mut self) -> Option<&QueueEntry> {
        let prev = self.current.filter(|i| *i > 0).map(|i| i - 1)?;
        self.current = Some(prev);
        self.items.get(prev)
    }

    /// 跳到指定下标。越界返回 `None` 且不改变状态。
    pub fn jump(&mut self, index: usize) -> Option<&QueueEntry> {
        if index >= self.items.len() {
            return None;
        }
        self.current = Some(index);
        self.items.get(index)
    }

    pub fn has_next(&self) -> bool {
        self.current.is_some_and(|i| i + 1 < self.items.len())
    }

    pub fn has_previous(&self) -> bool {
        self.current.is_some_and(|i| i > 0)
    }

    /// 清空队列。
    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
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

    /// 属性测试：任意操作序列后，队列不变量必须成立。
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

        // 确定性伪随机序列，避免引入 rand 依赖
        let mut acc: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..500 {
            acc = acc.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ops[(acc >> 33) as usize % ops.len()](&mut q);

            // 不变量 1：current 若存在，必须落在合法下标内
            if let Some(i) = q.current_index() {
                assert!(i < q.len(), "current={i} 越出 len={}", q.len());
                assert!(q.current().is_some(), "current 存在则必须能取到条目");
            }
            // 不变量 2：has_next / has_previous 与下标一致
            if let Some(i) = q.current_index() {
                assert_eq!(q.has_next(), i + 1 < q.len());
                assert_eq!(q.has_previous(), i > 0);
            }
        }
    }
}
