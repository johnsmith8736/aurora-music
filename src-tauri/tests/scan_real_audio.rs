//! 针对**真实音频文件**的扫描集成测试。
//!
//! 与单元测试的区别：这里不使用构造的假数据，而是对 ffmpeg 生成的真实
//! MP3 / FLAC / WAV 文件执行完整链路（遍历 → 元数据提取 → 入库 → 查询），
//! 验证提取出的时长、采样率、标签确实来自文件本身。
//!
//! 夹具由 `tests/fixtures/gen-fixtures.sh` 生成并已提交到仓库。
//!
//! # 关于损坏文件的实测结论
//!
//! 扫描阶段（`lofty`，只读容器头部）与播放阶段（`rodio`/`symphonia`，
//! 必须解码音频帧）对损坏文件的宽容度**不同**：
//!
//! | 文件 | 扫描阶段 | 播放阶段 |
//! |---|---|---|
//! | 纯文本改名 `.mp3` | ❌ 拒绝 | ❌ 拒绝 |
//! | 截断 50% 的 FLAC | ❌ **拒绝** | ⚠️ 接受但提前结束 |
//!
//! 也就是说扫描阶段比预想的更严格 —— 截断文件根本进不了资料库。
//! 这降低了播放阶段遇到"静默提前结束"的概率，但不能完全消除：
//! 损坏发生在音频帧中间而非文件尾部时，头部仍然是合法的。

use std::path::PathBuf;

use aurora_music_lib::app::library_service;
use aurora_music_lib::infra::db;

/// 夹具目录的固定清单 —— 增删夹具时必须同步更新此处。
///
/// | 文件 | 扫描结果 |
/// |---|---|
/// | `tagged.mp3` | ✅ 入库（完整 ID3 标签） |
/// | `tagged.flac` | ✅ 入库（完整标签） |
/// | `07 - Untagged Track.wav` | ✅ 入库（无标签，标题回退文件名） |
/// | `decodes_short.flac` | ✅ 入库（容器合法，但音频数据被截断） |
/// | `truncated.flac` | ❌ 拒绝（截断导致容器结构损坏） |
/// | `not_audio.mp3` | ❌ 拒绝（纯文本伪装） |
/// | `notes.txt` | ⏭ 忽略（非音频扩展名） |
const FIXTURE_TOTAL_FILES: usize = 7;
const FIXTURE_AUDIO_CANDIDATES: i64 = 6;
const EXPECTED_ADDED: usize = 4;
const EXPECTED_FAILED: i64 = 2;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio")
}

struct Ctx {
    _dir: tempfile::TempDir,
    pool: db::DbPool,
}

fn setup() -> Ctx {
    let dir = tempfile::TempDir::new().expect("临时目录");
    let pool = db::open(&dir.path().join("test.db")).expect("测试数据库");
    Ctx { _dir: dir, pool }
}

fn scanned_library() -> (Ctx, Vec<aurora_music_lib::domain::model::Track>) {
    let ctx = setup();
    let fixtures = fixtures_dir();
    assert!(
        fixtures.is_dir(),
        "夹具目录不存在，请先运行 tests/fixtures/gen-fixtures.sh: {fixtures:?}"
    );
    let root = library_service::add_root(&ctx.pool, fixtures.to_str().unwrap()).unwrap();
    library_service::scan_root(&ctx.pool, root.id).unwrap();
    let tracks = library_service::list_tracks(&ctx.pool, None, 100).unwrap();
    (ctx, tracks)
}

/// 夹具清单守卫。
///
/// 增删夹具文件时此测试会失败，提醒同步更新上方的期望常量与文档表格 ——
/// 避免"夹具变了但断言没变"造成的假绿。
#[test]
fn fixture_inventory_matches_documented_expectations() {
    let dir = fixtures_dir();
    let files: Vec<String> = std::fs::read_dir(&dir)
        .expect("读取夹具目录")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        files.len(),
        FIXTURE_TOTAL_FILES,
        "夹具目录文件数与文档不符。当前内容: {files:?}"
    );

    let audio = files
        .iter()
        .filter(|f| {
            let ext = std::path::Path::new(f)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            ["mp3", "flac", "wav", "ogg", "m4a", "aac"].contains(&ext.as_str())
        })
        .count() as i64;

    assert_eq!(
        audio, FIXTURE_AUDIO_CANDIDATES,
        "音频候选文件数与文档不符。当前内容: {files:?}"
    );
}

#[test]
fn scans_real_audio_files_and_extracts_real_metadata() {
    let (_ctx, tracks) = scanned_library();
    assert_eq!(tracks.len(), EXPECTED_ADDED);

    let by_name = |needle: &str| {
        tracks
            .iter()
            .find(|t| t.path.ends_with(needle))
            .unwrap_or_else(|| panic!("未找到 {needle}"))
    };

    // ── MP3：标签来自真实的 ID3 帧 ──────────────────────────────
    let mp3 = by_name("tagged.mp3");
    assert_eq!(mp3.title, "Fixture MP3");
    assert_eq!(mp3.artist.as_deref(), Some("Fixture Artist"));
    assert_eq!(mp3.album.as_deref(), Some("Fixture Album"));
    assert_eq!(mp3.album_artist.as_deref(), Some("Fixture Album Artist"));
    assert_eq!(mp3.track_no, Some(3));
    assert_eq!(mp3.year, Some(2026));
    assert_eq!(mp3.codec.as_deref(), Some("MP3"));
    assert_eq!(mp3.sample_rate, Some(8000), "采样率来自真实文件");
    assert_eq!(mp3.channels, Some(1), "夹具是单声道");

    // MP3 有时长不确定性：编码器会插入延迟帧与 padding，
    // 且 MP3 帧粒度为 1152 采样，因此**不应**断言精确等于 1000 ms。
    let mp3_ms = mp3.duration_ms.expect("MP3 应能读出时长");
    assert!(
        (900..1400).contains(&mp3_ms),
        "MP3 时长应在 1 秒附近（含编码 padding），实际 {mp3_ms} ms"
    );

    // ── FLAC：无损容器，时长精确 ────────────────────────────────
    let flac = by_name("tagged.flac");
    assert_eq!(flac.title, "Fixture FLAC");
    assert_eq!(flac.artist.as_deref(), Some("Fixture Artist"));
    assert_eq!(flac.year, Some(2026));
    assert_eq!(flac.codec.as_deref(), Some("FLAC"));
    assert_eq!(
        flac.duration_ms,
        Some(1000),
        "FLAC 是无损容器，1 秒音频应精确报 1000 ms"
    );

    // ── WAV：无标签 → 必须回退到文件名，且不得伪造标签 ──────────
    let wav = by_name("Untagged Track.wav");
    assert_eq!(
        wav.title, "Untagged Track",
        "无标签文件应回退到文件名，并去掉 '07 - ' 轨道号前缀"
    );
    assert_eq!(wav.artist, None, "无标签时不得伪造艺术家");
    assert_eq!(wav.album, None);
    assert_eq!(wav.album_key, None, "无专辑信息不应生成聚合键");
    assert_eq!(wav.codec.as_deref(), Some("PCM"));
    assert_eq!(wav.duration_ms, Some(1000), "WAV 时长应精确");

    // ── 专辑聚合键 ──────────────────────────────────────────────
    assert!(mp3.album_key.is_some());
    assert!(flac.album_key.is_some());

    // ── 路径必须是绝对且真实存在的 ──────────────────────────────
    for t in &tracks {
        assert!(t.path.starts_with('/'), "入库路径应为绝对路径: {}", t.path);
        assert!(
            std::path::Path::new(&t.path).exists(),
            "入库的路径必须真实存在: {}",
            t.path
        );
    }
}

#[test]
fn scan_statistics_match_real_file_contents() {
    let ctx = setup();
    let fixtures = fixtures_dir();
    let root = library_service::add_root(&ctx.pool, fixtures.to_str().unwrap()).unwrap();
    let summary = library_service::scan_root(&ctx.pool, root.id).unwrap();

    // 目录里有 6 个文件，其中 notes.txt 不是音频扩展名，被完全忽略
    assert_eq!(summary.seen, FIXTURE_AUDIO_CANDIDATES, "音频候选数应与夹具清单一致");
    assert_eq!(summary.added as usize, EXPECTED_ADDED);
    assert_eq!(summary.failed, EXPECTED_FAILED);
    assert_eq!(summary.updated, 0);
    assert_eq!(summary.skipped, 0);
    assert!(summary.duration_ms >= 0);
}

#[test]
fn corrupted_files_are_rejected_at_scan_time() {
    let ctx = setup();
    let fixtures = fixtures_dir();
    let root = library_service::add_root(&ctx.pool, fixtures.to_str().unwrap()).unwrap();
    let summary = library_service::scan_root(&ctx.pool, root.id).unwrap();

    let tracks = library_service::list_tracks(&ctx.pool, None, 100).unwrap();

    // 纯文本改名 .mp3 → 拒绝
    assert!(
        !tracks.iter().any(|t| t.path.ends_with("not_audio.mp3")),
        "伪装成 .mp3 的文本文件不得进入资料库"
    );

    // 截断 50% 的 FLAC → **扫描阶段即被拒绝**
    // （lofty 会校验容器结构，比 rodio 的解码器更严格）
    assert!(
        !tracks.iter().any(|t| t.path.ends_with("truncated.flac")),
        "截断的 FLAC 应在扫描阶段被拒绝，而不是带着错误的时长入库"
    );

    // 两个失败都必须有明确的、不含路径的错误说明
    assert_eq!(summary.errors.len() as i64, EXPECTED_FAILED);
    for e in &summary.errors {
        assert!(!e.name.contains('/'), "错误记录不应暴露绝对路径: {}", e.name);
        assert!(!e.reason.is_empty(), "必须给出可读的失败原因");
    }
}

#[test]
fn rescanning_unchanged_library_skips_everything() {
    let ctx = setup();
    let fixtures = fixtures_dir();
    let root = library_service::add_root(&ctx.pool, fixtures.to_str().unwrap()).unwrap();

    let first = library_service::scan_root(&ctx.pool, root.id).unwrap();
    assert_eq!(first.added as usize, EXPECTED_ADDED);

    // 第二次扫描：文件大小与修改时间都未变 → 全部跳过
    let second = library_service::scan_root(&ctx.pool, root.id).unwrap();
    assert_eq!(
        second.skipped as usize, EXPECTED_ADDED,
        "增量扫描应跳过全部未变文件"
    );
    assert_eq!(second.added, 0, "不应重复入库");
    assert_eq!(second.updated, 0);

    assert_eq!(
        library_service::list_tracks(&ctx.pool, None, 100)
            .unwrap()
            .len(),
        EXPECTED_ADDED,
        "重复扫描不得产生重复行"
    );
}

#[test]
fn modifying_a_file_triggers_update_not_duplicate() {
    let ctx = setup();
    let fixtures = fixtures_dir();
    let root = library_service::add_root(&ctx.pool, fixtures.to_str().unwrap()).unwrap();
    library_service::scan_root(&ctx.pool, root.id).unwrap();

    // 把夹具复制到独立目录，避免污染已提交的夹具
    let work = tempfile::TempDir::new().unwrap();
    let target = work.path().join("tagged.mp3");
    std::fs::copy(fixtures.join("tagged.mp3"), &target).unwrap();

    let work_root = library_service::add_root(&ctx.pool, work.path().to_str().unwrap()).unwrap();
    let first = library_service::scan_root(&ctx.pool, work_root.id).unwrap();
    assert_eq!(first.added, 1);

    // 追加字节 → 大小与 mtime 改变
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.extend_from_slice(b"\0\0\0\0");
    std::fs::write(&target, &bytes).unwrap();

    let second = library_service::scan_root(&ctx.pool, work_root.id).unwrap();
    assert_eq!(second.updated, 1, "变更的文件应被更新");
    assert_eq!(second.added, 0, "不应产生新行");
    assert_eq!(
        library_service::list_tracks(&ctx.pool, None, 100)
            .unwrap()
            .iter()
            .filter(|t| t.path == target.to_str().unwrap())
            .count(),
        1,
        "同一路径只能有一行"
    );
}

#[test]
fn search_finds_real_tracks_by_tag_content() {
    let (ctx, _) = scanned_library();

    // 按标签标题搜索
    let hits = library_service::list_tracks(&ctx.pool, Some("Fixture"), 100).unwrap();
    assert_eq!(hits.len(), 2, "应命中 tagged.mp3 与 tagged.flac");

    // 按艺术家搜索
    let by_artist = library_service::list_tracks(&ctx.pool, Some("Artist"), 100).unwrap();
    assert!(by_artist.len() >= 2);

    // 按文件名回退出的标题搜索 —— 证明回退结果也进了索引
    let fallback = library_service::list_tracks(&ctx.pool, Some("Untagged"), 100).unwrap();
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0].title, "Untagged Track");

    // 无结果时返回空而不报错
    assert!(library_service::list_tracks(&ctx.pool, Some("zzzzznotfound"), 100)
        .unwrap()
        .is_empty());
}

#[test]
fn nonexistent_directory_reports_error_not_panic() {
    let ctx = setup();
    let err = library_service::add_root(&ctx.pool, "/definitely/not/a/real/path");
    assert!(err.is_err(), "不存在的目录必须报错");

    let payload: aurora_music_lib::domain::ErrorPayload = err.unwrap_err().into();
    assert_eq!(payload.code, "LIBRARY_ROOT_NOT_FOUND");
    assert!(
        !payload.message.contains("/definitely"),
        "面向用户的消息不得包含路径: {}",
        payload.message
    );
}

#[test]
fn empty_directory_reports_no_audio_files() {
    let ctx = setup();
    let empty = tempfile::TempDir::new().unwrap();
    let root = library_service::add_root(&ctx.pool, empty.path().to_str().unwrap()).unwrap();

    let err = library_service::scan_root(&ctx.pool, root.id).unwrap_err();
    let payload: aurora_music_lib::domain::ErrorPayload = err.into();
    assert_eq!(payload.code, "LIBRARY_NO_AUDIO_FILES");
}
