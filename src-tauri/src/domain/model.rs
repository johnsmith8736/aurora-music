//! 领域数据模型。
//!
//! 这些类型不依赖任何 I/O 库（无 rusqlite / lofty / tauri 类型），
//! 因此可以脱离数据库与文件系统进行单元测试。

use serde::{Deserialize, Serialize};

/// 曲目主键。
pub type TrackId = i64;
/// 资料库根目录主键。
pub type RootId = i64;

/// 应用当前支持的音频扩展名白名单（小写，不含点）。
///
/// 白名单而非黑名单：避免误把封面、歌词、说明文件当成音频。
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "ogg", "oga", "m4a", "aac"];

/// 判断路径扩展名是否在音频白名单内。
pub fn is_audio_extension(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTENSIONS.contains(&lower.as_str())
}

/// 资料库根目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRoot {
    pub id: RootId,
    /// 规范化的绝对路径。
    pub path: String,
    pub enabled: bool,
    pub last_scan_at: Option<i64>,
    pub created_at: i64,
}

/// 曲目的完整元数据（尚未入库，或从库中读出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: TrackId,
    pub path: String,

    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub album_key: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,

    pub duration_ms: Option<i64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bitrate: Option<u32>,
    pub codec: Option<String>,
    pub container: Option<String>,

    pub file_size: i64,
    pub file_mtime: i64,
    pub added_at: i64,
    pub updated_at: i64,
}

/// 文件系统探测得到的原始元数据 —— 入库前的中间形态。
///
/// 与 `Track` 的区别：没有数据库分配的 `id` / `added_at` / `updated_at`。
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMetadata {
    pub path: String,
    pub file_size: i64,
    pub file_mtime: i64,

    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub album_key: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<i32>,

    pub duration_ms: Option<i64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bitrate: Option<u32>,
    pub codec: Option<String>,
    pub container: Option<String>,
}

/// 计算专辑聚合键。
///
/// 只使用专辑名会让所有 "Greatest Hits" 撞在一起，因此必须以
/// 专辑艺术家 + 专辑名共同构成身份。无专辑艺术家时回退到艺术家。
///
/// 返回 `None` 表示既无专辑名也无艺术家，无法构成专辑身份。
///
/// 注意：当前仅做大小写折叠与空白规范化。完整的 Unicode NFKC 规范化
/// 推迟到需要处理变音符号时再引入 `unicode-normalization`。
pub fn compute_album_key(
    album: Option<&str>,
    album_artist: Option<&str>,
    artist: Option<&str>,
) -> Option<String> {
    let album = album.map(str::trim).filter(|s| !s.is_empty())?;
    let owner = album_artist
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| artist.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");

    Some(format!(
        "{}\u{1f}{}",
        owner.to_lowercase(),
        album.to_lowercase()
    ))
}

/// 从文件名推断标题（标签缺失时的回退，FR-1.5）。
///
/// 会去掉常见的 "01 - " / "01. " 轨道号前缀。
pub fn title_from_filename(filename: &str) -> String {
    let stem = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename);

    // 去掉形如 "01 - "、"01. "、"01_" 的轨道号前缀
    let trimmed = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let trimmed = trimmed.trim_start_matches([' ', '-', '.', '_']);

    if trimmed.is_empty() {
        stem.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_extension_whitelist() {
        assert!(is_audio_extension("mp3"));
        assert!(is_audio_extension("MP3"));
        assert!(is_audio_extension("Flac"));
        assert!(is_audio_extension("wav"));
        assert!(!is_audio_extension("txt"));
        assert!(!is_audio_extension("jpg"));
        assert!(!is_audio_extension(""));
    }

    #[test]
    fn album_key_distinguishes_same_album_name_by_different_artists() {
        // 两张都叫 "Greatest Hits" 的专辑必须区分开
        let a = compute_album_key(Some("Greatest Hits"), Some("Queen"), None);
        let b = compute_album_key(Some("Greatest Hits"), Some("ABBA"), None);
        assert_ne!(a, b);
    }

    #[test]
    fn album_key_is_case_insensitive() {
        assert_eq!(
            compute_album_key(Some("Abbey Road"), Some("The Beatles"), None),
            compute_album_key(Some("abbey road"), Some("the beatles"), None)
        );
    }

    #[test]
    fn album_key_prefers_album_artist_over_artist() {
        // 专辑艺术家存在时必须优先使用它
        let with_aa = compute_album_key(Some("X"), Some("AA"), Some("A"));
        let with_a = compute_album_key(Some("X"), None, Some("A"));
        assert_ne!(with_aa, with_a, "专辑艺术家应优先于艺术家");

        // 无专辑艺术家时才回退到艺术家
        let fallback = compute_album_key(Some("X"), None, Some("AA"));
        assert_eq!(with_aa, fallback, "无专辑艺术家时应回退到艺术家");
    }

    #[test]
    fn album_key_none_without_album() {
        assert_eq!(compute_album_key(None, Some("A"), Some("A")), None);
        assert_eq!(compute_album_key(Some("   "), Some("A"), None), None);
    }

    #[test]
    fn title_from_filename_strips_track_number() {
        assert_eq!(title_from_filename("01 - Song.mp3"), "Song");
        assert_eq!(title_from_filename("02. Another.flac"), "Another");
        assert_eq!(title_from_filename("03_Song.wav"), "Song");
        assert_eq!(title_from_filename("Plain Title.mp3"), "Plain Title");
        // 全数字文件名不应被削成空串
        assert_eq!(title_from_filename("01.mp3"), "01");
    }
}
