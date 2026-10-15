//! IPC 契约测试。
//!
//! 前端 `src/ipc/types.ts` 中的类型是**手写**的（见 architecture.md Q-1）。
//! 手写契约最大的风险是字段名漂移：Rust 侧改了 `#[serde(rename_all)]`
//! 或字段名，TS 侧不会报错，只会在运行时静默变成 `undefined`。
//!
//! 本文件把这些 JSON 形状钉死。**任何一侧改动导致这里失败，都说明
//! 前端的类型定义需要同步更新。**

use aurora_music_lib::app::library_service::LibraryStats;
use aurora_music_lib::domain::model::{LibraryRoot, Track};
use serde_json::Value;

fn keys(value: &Value) -> Vec<String> {
    let mut k: Vec<String> = value
        .as_object()
        .expect("应为 JSON 对象")
        .keys()
        .cloned()
        .collect();
    k.sort();
    k
}

#[test]
fn track_serializes_with_camel_case_field_names() {
    let track = Track {
        id: 1,
        path: "/music/a.mp3".into(),
        title: "T".into(),
        artist: None,
        album: None,
        album_artist: None,
        album_key: None,
        track_no: None,
        disc_no: None,
        year: None,
        duration_ms: Some(1000),
        sample_rate: Some(44100),
        channels: Some(2),
        bitrate: Some(320),
        codec: Some("MP3".into()),
        container: Some("MPEG".into()),
        file_size: 100,
        file_mtime: 200,
        added_at: 300,
        updated_at: 400,
    };

    let json: Value = serde_json::to_value(&track).expect("序列化 Track");
    let actual = keys(&json);

    // 这份列表必须与 src/ipc/types.ts 的 Track 接口一致。
    // 顺序为字节序排序（`keys()` 的结果），因此 addedAt 在 album 之前。
    let expected = [
        "addedAt",
        "album",
        "albumArtist",
        "albumKey",
        "artist",
        "bitrate",
        "channels",
        "codec",
        "container",
        "discNo",
        "durationMs",
        "fileMtime",
        "fileSize",
        "id",
        "path",
        "sampleRate",
        "title",
        "trackNo",
        "updatedAt",
        "year",
    ];

    assert_eq!(
        actual, expected,
        "\nTrack 的 JSON 字段与 src/ipc/types.ts 不一致。\n\
         若你刚刚修改了 Rust 结构体，请同步更新前端类型定义。"
    );
}

#[test]
fn library_root_serializes_with_camel_case_field_names() {
    let root = LibraryRoot {
        id: 1,
        path: "/music".into(),
        enabled: true,
        last_scan_at: Some(123),
        created_at: 456,
    };

    let json: Value = serde_json::to_value(&root).expect("序列化 LibraryRoot");
    assert_eq!(
        keys(&json),
        ["createdAt", "enabled", "id", "lastScanAt", "path"],
        "\nLibraryRoot 的 JSON 字段与 src/ipc/types.ts 不一致"
    );
}

#[test]
fn library_stats_serializes_with_camel_case_field_names() {
    let stats = LibraryStats {
        root_count: 2,
        track_count: 42,
    };
    let json: Value = serde_json::to_value(&stats).expect("序列化 LibraryStats");
    assert_eq!(
        keys(&json),
        ["rootCount", "trackCount"],
        "\nLibraryStats 的 JSON 字段与 src/ipc/types.ts 不一致"
    );
}

#[test]
fn error_payload_shape_matches_frontend_contract() {
    use aurora_music_lib::domain::{AppError, ErrorPayload, LibraryError};

    let payload: ErrorPayload = AppError::Library(LibraryError::RootNotFound).into();
    let json: Value = serde_json::to_value(&payload).expect("序列化 ErrorPayload");

    assert_eq!(
        keys(&json),
        ["code", "message", "retryable"],
        "\nErrorPayload 的 JSON 字段与 src/ipc/types.ts 不一致"
    );
    assert_eq!(json["code"], "LIBRARY_ROOT_NOT_FOUND");
    assert_eq!(json["retryable"], false);
    assert!(
        json["message"].as_str().is_some_and(|m| !m.is_empty()),
        "必须提供面向用户的消息"
    );
}

#[test]
fn scan_summary_shape_is_stable() {
    // ScanSummary 无法直接构造（字段私有度不同），因此用真实扫描产出
    use aurora_music_lib::app::library_service;
    use aurora_music_lib::infra::db;
    use std::path::PathBuf;

    let dir = tempfile::TempDir::new().unwrap();
    let pool = db::open(&dir.path().join("t.db")).unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");

    let root = library_service::add_root(&pool, fixtures.to_str().unwrap()).unwrap();
    let summary = library_service::scan_root(&pool, root.id).unwrap();

    let json: Value = serde_json::to_value(&summary).expect("序列化 ScanSummary");
    assert_eq!(
        keys(&json),
        [
            "added",
            "durationMs",
            "errors",
            "failed",
            "missing",
            "rootId",
            "seen",
            "skipped",
            "updated",
        ],
        "\nScanSummary 的 JSON 字段与 src/ipc/types.ts 不一致"
    );

    // 错误条目也必须是 { name, reason }
    let errors = json["errors"].as_array().expect("errors 应为数组");
    assert!(!errors.is_empty(), "夹具中应有失败文件");
    assert_eq!(keys(&errors[0]), ["name", "reason"]);
}

#[test]
fn playback_event_shape_is_stable() {
    use aurora_music_lib::app::PlaybackEvent;

    // internally-tagged enum：serde 的 rename_all 只作用于变体名，
    // 变体内部字段是否也被重命名必须实测确认。
    let cases = [
        (
            PlaybackEvent::TrackStarted { track_id: 7 },
            vec!["kind", "trackId"],
        ),
        (
            PlaybackEvent::TrackEnded { track_id: 7 },
            vec!["kind", "trackId"],
        ),
        (
            PlaybackEvent::TrackFailed {
                track_id: 7,
                code: "PLAYBACK_DECODE_FAILED".into(),
                message: "无法播放".into(),
            },
            vec!["code", "kind", "message", "trackId"],
        ),
        (
            PlaybackEvent::QueueFinished,
            vec!["kind"],
        ),
    ];

    for (event, expected) in cases {
        let json: Value = serde_json::to_value(&event).expect("序列化 PlaybackEvent");
        assert_eq!(
            keys(&json),
            expected,
            "\nPlaybackEvent 的 JSON 字段与 src/ipc/types.ts 不一致: {json}"
        );
    }

    // 变体标签必须是 camelCase
    let started = serde_json::to_value(PlaybackEvent::TrackStarted { track_id: 1 }).unwrap();
    assert_eq!(started["kind"], "trackStarted");
    let ended = serde_json::to_value(PlaybackEvent::TrackEnded { track_id: 1 }).unwrap();
    assert_eq!(ended["kind"], "trackEnded");
    let failed = serde_json::to_value(PlaybackEvent::TrackFailed {
        track_id: 1,
        code: "X".into(),
        message: "y".into(),
    })
    .unwrap();
    assert_eq!(failed["kind"], "trackFailed");
    let finished = serde_json::to_value(PlaybackEvent::QueueFinished).unwrap();
    assert_eq!(finished["kind"], "queueFinished");
}
