//! 单文件元数据提取。
//!
//! 使用 `lofty` 读取容器头部与标签 —— **不解码音频**，因此速度极快。
//!
//! ⚠️ 已知局限：截断/部分损坏的文件其头部仍然完好，lofty 会读出
//! 声称的时长（例如被截掉一半的 FLAC 仍报告原始时长）。这类损坏
//! 只能在播放阶段通过"实际播出时长 vs 声明时长"比对才能发现，
//! 扫描阶段无法可靠识别。

use std::path::Path;

use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::prelude::Accessor;

use crate::domain::model::{compute_album_key, title_from_filename, TrackMetadata};

/// 元数据提取失败的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeError {
    /// 无法读取文件（权限、I/O）
    Io,
    /// 文件不是可识别的音频格式
    UnrecognizedFormat,
    /// 是音频但解析头部失败（损坏）
    Malformed(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Io => write!(f, "无法读取文件"),
            ProbeError::UnrecognizedFormat => write!(f, "不是可识别的音频格式"),
            ProbeError::Malformed(_) => write!(f, "音频文件已损坏或无法解析"),
        }
    }
}

/// 提取一个音频文件的元数据。
pub fn probe_file(path: &Path, file_size: i64, file_mtime: i64) -> Result<TrackMetadata, ProbeError> {
    // 空文件直接判定，避免进 lofty 产生误导性错误
    if file_size == 0 {
        return Err(ProbeError::Malformed("文件为空".into()));
    }

    // 显式打开文件以区分 I/O 失败与解析失败：
    // lofty 的 FileParseError 只暴露 `is_unknown_format()`，不区分 I/O。
    let mut file = std::fs::File::open(path).map_err(|_| ProbeError::Io)?;
    let tagged = lofty::read_from(&mut file).map_err(|e| {
        if e.is_unknown_format() {
            ProbeError::UnrecognizedFormat
        } else {
            ProbeError::Malformed(e.to_string())
        }
    })?;

    let props = tagged.properties();
    let file_type = tagged.file_type();

    // 优先主标签，回退到任意第一个标签
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title_tag = tag.and_then(|t| t.title()).map(|c| c.into_owned());
    let artist = tag.and_then(|t| t.artist()).map(|c| c.into_owned());
    let album = tag.and_then(|t| t.album()).map(|c| c.into_owned());
    let album_artist = tag
        .and_then(|t| {
            use lofty::tag::ItemKey;
            t.get_string(ItemKey::AlbumArtist).map(str::to_owned)
        })
        .filter(|s| !s.trim().is_empty());

    let track_no = tag.and_then(|t| t.track());
    // 注意：lofty 的 Accessor 用 `disk`（不是 disc），年份在 `date()` 返回的
    // Timestamp 里，而不是独立的 `year()`。
    let disc_no = tag.and_then(|t| t.disk());
    let year = tag.and_then(|t| t.date()).map(|ts| ts.year as i32);

    // 标题回退到文件名（FR-1.5）
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = title_tag
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| title_from_filename(&filename));

    let clean = |s: Option<String>| s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let artist = clean(artist);
    let album = clean(album);

    let duration_ms = {
        let ms = props.duration().as_millis() as i64;
        // 时长为 0 意味着头部没有有效信息，记为未知而非 0
        if ms > 0 {
            Some(ms)
        } else {
            None
        }
    };

    let album_key = compute_album_key(album.as_deref(), album_artist.as_deref(), artist.as_deref());

    let (codec, container) = describe_format(file_type);

    Ok(TrackMetadata {
        path: path.to_string_lossy().into_owned(),
        file_size,
        file_mtime,
        title,
        artist,
        album,
        album_artist,
        album_key,
        track_no,
        disc_no,
        year,
        duration_ms,
        sample_rate: props.sample_rate(),
        channels: props.channels().map(u16::from),
        bitrate: props.audio_bitrate().or_else(|| props.overall_bitrate()),
        codec: codec.map(str::to_owned),
        container: container.map(str::to_owned),
    })
}

/// 把 lofty 的 FileType 映射为 (编码, 容器) 展示名。
fn describe_format(ft: FileType) -> (Option<&'static str>, Option<&'static str>) {
    match ft {
        FileType::Mpeg => (Some("MP3"), Some("MPEG")),
        FileType::Flac => (Some("FLAC"), Some("FLAC")),
        FileType::Wav => (Some("PCM"), Some("WAV")),
        FileType::Aiff => (Some("PCM"), Some("AIFF")),
        FileType::Mp4 => (Some("AAC"), Some("MP4")),
        FileType::Aac => (Some("AAC"), Some("ADTS")),
        FileType::Vorbis => (Some("Vorbis"), Some("Ogg")),
        FileType::Opus => (Some("Opus"), Some("Ogg")),
        FileType::Speex => (Some("Speex"), Some("Ogg")),
        FileType::WavPack => (Some("WavPack"), Some("WV")),
        FileType::Ape => (Some("Monkey's Audio"), Some("APE")),
        FileType::Mpc => (Some("Musepack"), Some("MPC")),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_malformed() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("empty.mp3");
        std::fs::write(&p, b"").unwrap();
        let r = probe_file(&p, 0, 0);
        assert!(matches!(r, Err(ProbeError::Malformed(_))));
    }

    #[test]
    fn text_file_disguised_as_mp3_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("fake.mp3");
        std::fs::write(&p, b"this is not audio at all").unwrap();
        let size = std::fs::metadata(&p).unwrap().len() as i64;
        let r = probe_file(&p, size, 0);
        assert!(
            matches!(
                r,
                Err(ProbeError::UnrecognizedFormat) | Err(ProbeError::Malformed(_))
            ),
            "伪装文件必须被拒绝，实际: {r:?}"
        );
    }

    #[test]
    fn missing_file_reports_io_error() {
        let r = probe_file(Path::new("/nonexistent/x.mp3"), 100, 0);
        assert!(matches!(r, Err(ProbeError::Io)));
    }

    #[test]
    fn error_messages_do_not_leak_paths() {
        // 错误类型本身不携带路径，这是刻意的
        assert_eq!(ProbeError::Io.to_string(), "无法读取文件");
        assert_eq!(
            ProbeError::Malformed("/secret/path".into()).to_string(),
            "音频文件已损坏或无法解析"
        );
    }
}
