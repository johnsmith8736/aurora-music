//! 本地文件扫描。
//!
//! 职责：遍历目录、按扩展名白名单过滤、提取元数据、判定增量。
//!
//! 设计约束（docs/architecture.md §2.2）：
//! - **对音乐目录只读**，绝不写入、移动或删除用户文件
//! - 单文件失败不中断整体扫描
//! - 不跟随符号链接（防循环）
//! - 不在扫描阶段解码音频（代价过高），元数据来自容器头部

pub mod probe;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::domain::model::{is_audio_extension, TrackMetadata};
use crate::domain::LibraryError;
use crate::infra::db::repo::ScanStats;

/// 一个待写入的曲目。
#[derive(Debug, Clone)]
pub struct ScannedTrack {
    pub meta: TrackMetadata,
    /// 相对资料库是新增（true）还是更新（false）。
    pub is_new: bool,
}

/// 单个文件的失败记录。`name` 只保留文件名，不暴露绝对路径。
#[derive(Debug, Clone)]
pub struct FileError {
    pub name: String,
    pub reason: String,
}

/// 一次扫描的完整结果。
#[derive(Debug, Default)]
pub struct ScanReport {
    pub tracks: Vec<ScannedTrack>,
    /// 库中存在但本次未在磁盘上找到的路径。
    pub missing: Vec<String>,
    pub stats: ScanStats,
    pub errors: Vec<FileError>,
}

/// 已知状态：(大小, 修改时间毫秒)。
pub type KnownStates = HashMap<String, (i64, i64)>;

/// 遍历根目录并产出扫描结果。
///
/// `known` 来自数据库，用于增量判定：大小与修改时间都未变的文件直接跳过。
pub fn scan_directory(root: &Path, known: &KnownStates) -> Result<ScanReport, LibraryError> {
    if !root.is_dir() {
        return Err(LibraryError::RootNotFound);
    }

    let mut report = ScanReport::default();
    let mut seen: Vec<String> = Vec::new();

    // depth == 0 是用户显式选择的根目录，**必须跳过隐藏判定**：
    // 用户完全可能把音乐放在 ~/.music 这样的隐藏目录里。
    // 隐藏过滤只对根目录以下的层级生效。
    let walker = WalkDir::new(root)
        .follow_links(false) // 防符号链接循环
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.path()));

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // 单个目录不可读不应中断整体扫描（FR-1.10）
                report.errors.push(FileError {
                    name: e
                        .path()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "<未知条目>".into()),
                    reason: "无法读取目录条目".into(),
                });
                report.stats.failed += 1;
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        // 扩展名白名单
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_audio_extension(ext) {
            continue;
        }

        report.stats.seen += 1;

        let Some(path_str) = path.to_str().map(str::to_owned) else {
            // Linux 文件名可以是任意字节，非 UTF-8 路径无法存入 TEXT 列
            report.errors.push(FileError {
                name: path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                reason: "文件名不是有效的 UTF-8，已跳过".into(),
            });
            report.stats.failed += 1;
            continue;
        };
        seen.push(path_str.clone());

        // 读取文件系统属性
        let fs_meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => {
                report.errors.push(FileError {
                    name: display_name(path),
                    reason: "无法读取文件属性".into(),
                });
                report.stats.failed += 1;
                continue;
            }
        };

        let size = fs_meta.len() as i64;
        let mtime = mtime_millis(&fs_meta);

        // 增量判定：大小与修改时间都未变 → 跳过
        if let Some(&(known_size, known_mtime)) = known.get(&path_str)
            && known_size == size
            && known_mtime == mtime
        {
            report.stats.skipped += 1;
            continue;
        }

        match probe::probe_file(path, size, mtime) {
            Ok(meta) => {
                let is_new = !known.contains_key(&path_str);
                if is_new {
                    report.stats.added += 1;
                } else {
                    report.stats.updated += 1;
                }
                report.tracks.push(ScannedTrack { meta, is_new });
            }
            Err(e) => {
                report.errors.push(FileError {
                    name: display_name(path),
                    reason: e.to_string(),
                });
                report.stats.failed += 1;
            }
        }
    }

    // 找出库中已不存在于磁盘的条目
    for known_path in known.keys() {
        if !seen.contains(known_path) {
            report.missing.push(known_path.clone());
        }
    }

    Ok(report)
}

/// 取文件名用于展示（不暴露绝对路径）。
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<未知>".into())
}

/// 忽略隐藏文件与目录（以 `.` 开头）。
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with('.'))
}

/// 文件修改时间（Unix 毫秒）。
pub fn mtime_millis(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 把路径规范化为绝对路径字符串。
pub fn canonicalize(path: &Path) -> Result<PathBuf, LibraryError> {
    std::fs::canonicalize(path).map_err(|_| LibraryError::RootNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).expect("写入测试文件");
        p
    }

    #[test]
    fn ignores_non_audio_and_hidden_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        touch(root, "notes.txt", b"not audio");
        touch(root, "cover.jpg", b"fake image");
        fs::write(root.join(".hidden.mp3"), b"x").unwrap();

        // 这些会 probe 失败（内容不是音频），但必须被"看见"
        touch(root, "real.mp3", b"not really audio either");

        let report = scan_directory(root, &KnownStates::new()).unwrap();

        // 只有 .mp3 进入扫描；.txt/.jpg/.hidden.mp3 被忽略
        assert_eq!(report.stats.seen, 1, "只应看到 1 个音频候选文件");
        assert_eq!(report.stats.failed, 1, "伪装的 mp3 应记为失败");
        assert_eq!(report.errors.len(), 1);
        // 错误信息不含绝对路径
        assert!(
            !report.errors[0].name.contains('/'),
            "错误记录不应暴露绝对路径: {}",
            report.errors[0].name
        );
    }

    #[test]
    fn incremental_scan_skips_unchanged_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let p = touch(root, "a.mp3", b"0123456789");

        let fs_meta = fs::metadata(&p).unwrap();
        let mut known = KnownStates::new();
        known.insert(
            p.to_str().unwrap().to_string(),
            (fs_meta.len() as i64, mtime_millis(&fs_meta)),
        );

        let report = scan_directory(root, &known).unwrap();
        assert_eq!(report.stats.seen, 1);
        assert_eq!(report.stats.skipped, 1, "未变的文件必须被跳过");
        assert!(report.tracks.is_empty());
    }

    #[test]
    fn detects_missing_files_without_deleting_them() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        let mut known = KnownStates::new();
        let gone = root.join("gone.mp3");
        known.insert(gone.to_str().unwrap().to_string(), (10, 1));

        let report = scan_directory(root, &known).unwrap();
        assert_eq!(report.missing.len(), 1);
        assert!(report.missing[0].ends_with("gone.mp3"));
    }

    #[test]
    fn nonexistent_root_is_reported_not_panicking() {
        let err = scan_directory(Path::new("/definitely/not/here"), &KnownStates::new());
        assert!(matches!(err, Err(LibraryError::RootNotFound)));
    }

    /// 回归测试：根目录本身以 `.` 开头时，不得被隐藏判定剪掉整棵树。
    ///
    /// 此前的 bug：`filter_entry` 对 depth 0 也生效，导致用户把音乐放在
    /// `~/.music` 时扫描结果为空。tempfile 创建的目录恰好以 `.tmp` 开头，
    /// 因此这个场景在测试里会自然复现。
    #[test]
    fn hidden_root_directory_is_still_scanned() {
        let parent = tempfile::TempDir::new().unwrap();
        let hidden_root = parent.path().join(".hidden-music");
        fs::create_dir(&hidden_root).unwrap();
        touch(&hidden_root, "track.mp3", b"0123456789");

        assert!(
            hidden_root.file_name().unwrap().to_str().unwrap().starts_with('.'),
            "测试前提：根目录名必须以 . 开头"
        );

        let report = scan_directory(&hidden_root, &KnownStates::new()).unwrap();
        assert_eq!(
            report.stats.seen, 1,
            "隐藏在点目录下的音频文件必须被扫描到"
        );
    }

    /// 根目录**内部**的点目录仍应被跳过（例如 .git、.cache）。
    #[test]
    fn hidden_subdirectories_are_skipped() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let sub = root.join(".git");
        fs::create_dir(&sub).unwrap();
        touch(&sub, "junk.mp3", b"0123456789");
        touch(root, "real.mp3", b"0123456789");

        let report = scan_directory(root, &KnownStates::new()).unwrap();
        assert_eq!(report.stats.seen, 1, "根目录内的点目录应被跳过");
        assert_eq!(report.errors.len(), 1, "real.mp3 内容不是音频，记为失败");
    }
}
