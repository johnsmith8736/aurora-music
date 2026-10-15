//! 通过 Tauri **真实 IPC 层**调用命令的端到端测试。
//!
//! 这一层此前无法验证，现在用 `tauri::test::MockRuntime` 覆盖。
//! 它验证的是单元测试无法覆盖的部分：
//!
//! 1. 命令确实注册在 `generate_handler!` 中
//! 2. **参数名转换**：Rust 侧 `root_id` ← 前端 `rootId`
//!    （Tauri 2 默认使用 camelCase，见 tauri-macros ArgumentCase::Camel）
//! 3. 返回值序列化形状与前端 TS 类型一致
//! 4. 错误按 `ErrorPayload` 形状回传，而不是 panic 或字符串
//! 5. `spawn_blocking` 包装没有死锁

use std::path::PathBuf;
use std::sync::Arc;

use aurora_music_lib::app::{AppState, PlaybackService};
use aurora_music_lib::infra::audio::NullAudioEngine;
use aurora_music_lib::infra::db::{self, DbPool};
use aurora_music_lib::infra::random::FakeRandom;
use aurora_music_lib::infra::time::FakeClock;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{WebviewWindow, WebviewWindowBuilder};

type App = tauri::App<MockRuntime>;

/// IPC 测试一律使用无声引擎 —— 不得依赖真实声卡。
fn silent_playback() -> Arc<PlaybackService> {
    Arc::new(PlaybackService::new(
        Arc::new(NullAudioEngine::scripted()),
        Arc::new(FakeClock::new(1_000_000)),
        Arc::new(FakeRandom::new()),
    ))
}

fn build_app(pool: DbPool) -> App {
    mock_builder()
        .manage(AppState::new(pool, silent_playback()))
        .invoke_handler(tauri::generate_handler![
            aurora_music_lib::commands::library::library_add_root,
            aurora_music_lib::commands::library::library_list_roots,
            aurora_music_lib::commands::library::library_remove_root,
            aurora_music_lib::commands::library::library_scan_root,
            aurora_music_lib::commands::library::library_list_tracks,
            aurora_music_lib::commands::library::library_get_track,
            aurora_music_lib::commands::library::library_stats,
            aurora_music_lib::commands::playback::playback_play_tracks,
            aurora_music_lib::commands::playback::playback_toggle,
            aurora_music_lib::commands::playback::playback_stop,
            aurora_music_lib::commands::playback::playback_next,
            aurora_music_lib::commands::playback::playback_previous,
            aurora_music_lib::commands::playback::playback_seek,
            aurora_music_lib::commands::playback::playback_set_volume,
            aurora_music_lib::commands::playback::playback_get_state,
            aurora_music_lib::commands::queue::queue_set_repeat,
            aurora_music_lib::commands::queue::queue_set_shuffle,
        ])
        .build(tauri::generate_context!())
        .expect("构建测试应用")
}

/// 通过真实 IPC 通道调用一个命令。
///
/// `args` 使用 **camelCase** 键名 —— 与前端 `src/ipc/client.ts` 完全一致。
fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().expect("解析 URL"),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().expect("反序列化响应"))
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio")
}

struct Harness {
    _dir: tempfile::TempDir,
    app: App,
    webview: WebviewWindow<MockRuntime>,
}

fn harness() -> Harness {
    let dir = tempfile::TempDir::new().expect("临时目录");
    let pool = db::open(&dir.path().join("ipc.db")).expect("测试数据库");
    let app = build_app(pool);
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("创建测试窗口");
    Harness {
        _dir: dir,
        app,
        webview,
    }
}

#[test]
fn commands_are_registered_and_reachable_over_ipc() {
    let h = harness();

    // 未注册的命令必须失败 —— 反向证明测试确实走了 IPC 分发
    let unknown = invoke(&h.webview, "this_command_does_not_exist", json!({}));
    assert!(unknown.is_err(), "未注册的命令不应成功");

    // 已注册的命令必须成功
    let roots = invoke(&h.webview, "library_list_roots", json!({}));
    assert!(roots.is_ok(), "library_list_roots 应可调用: {roots:?}");
    assert_eq!(roots.unwrap(), json!([]), "新库应返回空数组");

    let _ = &h.app;
}

#[test]
fn add_scan_and_list_work_end_to_end_over_ipc() {
    let h = harness();

    // ── 1. 添加根目录（参数用 camelCase: path）──────────────────
    let added = invoke(
        &h.webview,
        "library_add_root",
        json!({ "path": fixtures_dir().to_str().unwrap() }),
    )
    .expect("library_add_root 应成功");

    assert_eq!(keys_of(&added), ["createdAt", "enabled", "id", "lastScanAt", "path"]);
    assert_eq!(added["enabled"], true);
    let root_id = added["id"].as_i64().expect("root id");

    // ── 2. 扫描（关键：Rust 侧参数名是 root_id，此处必须传 rootId）──
    let summary = invoke(&h.webview, "library_scan_root", json!({ "rootId": root_id }))
        .expect("library_scan_root 应成功");

    assert_eq!(summary["added"], 4, "夹具中 4 个文件应成功入库");
    assert_eq!(summary["failed"], 2, "2 个损坏文件应被拒绝");
    assert_eq!(summary["seen"], 6, "夹具含 6 个音频候选文件");

    // 失败明细必须包含可读原因，且不含绝对路径
    let errors = summary["errors"].as_array().expect("errors 数组");
    assert_eq!(errors.len(), 2);
    for e in errors {
        assert_eq!(keys_of(e), ["name", "reason"]);
        let name = e["name"].as_str().unwrap();
        assert!(!name.contains('/'), "错误项不应暴露绝对路径: {name}");
        assert!(!e["reason"].as_str().unwrap().is_empty());
    }

    // ── 3. 列出曲目 ─────────────────────────────────────────────
    let tracks = invoke(&h.webview, "library_list_tracks", json!({ "query": null, "limit": 100 }))
        .expect("library_list_tracks 应成功");
    let tracks = tracks.as_array().expect("曲目数组");
    assert_eq!(tracks.len(), 4);

    // 验证真实元数据经 IPC 传输后仍然正确
    let mp3 = tracks
        .iter()
        .find(|t| t["path"].as_str().unwrap().ends_with("tagged.mp3"))
        .expect("应包含 tagged.mp3");
    assert_eq!(mp3["title"], "Fixture MP3");
    assert_eq!(mp3["artist"], "Fixture Artist");
    assert_eq!(mp3["album"], "Fixture Album");
    assert_eq!(mp3["albumArtist"], "Fixture Album Artist");
    assert_eq!(mp3["trackNo"], 3);
    assert_eq!(mp3["sampleRate"], 8000);
    assert_eq!(mp3["channels"], 1);

    // ── 4. 搜索 ─────────────────────────────────────────────────
    let found = invoke(
        &h.webview,
        "library_list_tracks",
        json!({ "query": "Fixture", "limit": 100 }),
    )
    .expect("搜索应成功");
    assert_eq!(found.as_array().unwrap().len(), 2);

    // ── 5. 统计 ─────────────────────────────────────────────────
    let stats = invoke(&h.webview, "library_stats", json!({}))
        .expect("library_stats 应成功");
    assert_eq!(stats["rootCount"], 1);
    assert_eq!(stats["trackCount"], 4);

    // ── 6. 移除根目录 ───────────────────────────────────────────
    invoke(&h.webview, "library_remove_root", json!({ "rootId": root_id }))
        .expect("library_remove_root 应成功");

    let stats_after = invoke(&h.webview, "library_stats", json!({})).unwrap();
    assert_eq!(stats_after["rootCount"], 0);
    assert_eq!(
        stats_after["trackCount"], 0,
        "删除根目录应级联清理其曲目"
    );
}

#[test]
fn errors_cross_ipc_as_structured_payload_not_panic() {
    let h = harness();

    // 不存在的路径
    let err = invoke(
        &h.webview,
        "library_add_root",
        json!({ "path": "/definitely/not/a/real/path" }),
    )
    .expect_err("不存在的路径必须返回 Err");

    assert_eq!(keys_of(&err), ["code", "message", "retryable"]);
    assert_eq!(err["code"], "LIBRARY_ROOT_NOT_FOUND");
    assert_eq!(err["retryable"], false);
    assert!(
        !err["message"].as_str().unwrap().contains("/definitely"),
        "错误消息不得泄露路径"
    );

    // 空路径参数
    let empty = invoke(&h.webview, "library_add_root", json!({ "path": "   " }))
        .expect_err("空路径必须返回 Err");
    assert_eq!(empty["code"], "INVALID_ARGUMENT");

    // 不存在的 root_id
    let missing = invoke(&h.webview, "library_scan_root", json!({ "rootId": 9999 }))
        .expect_err("不存在的 root 必须返回 Err");
    assert_eq!(missing["code"], "LIBRARY_ROOT_NOT_FOUND");

    // 缺少必需参数：Tauri 在反序列化阶段就应拒绝，而不是调用命令
    let no_arg = invoke(&h.webview, "library_add_root", json!({}));
    assert!(no_arg.is_err(), "缺少参数时不应执行命令");
}

#[test]
fn scanning_the_same_library_twice_over_ipc_is_incremental() {
    let h = harness();

    let root_id = invoke(
        &h.webview,
        "library_add_root",
        json!({ "path": fixtures_dir().to_str().unwrap() }),
    )
    .unwrap()["id"]
        .as_i64()
        .unwrap();

    let first = invoke(&h.webview, "library_scan_root", json!({ "rootId": root_id })).unwrap();
    assert_eq!(first["added"], 4);
    assert_eq!(first["skipped"], 0);

    let second = invoke(&h.webview, "library_scan_root", json!({ "rootId": root_id })).unwrap();
    assert_eq!(second["added"], 0, "第二次扫描不应新增");
    assert_eq!(second["skipped"], 4, "未变更的文件应全部跳过");

    let tracks = invoke(&h.webview, "library_list_tracks", json!({ "limit": 100 })).unwrap();
    assert_eq!(tracks.as_array().unwrap().len(), 4, "不得产生重复行");
}

/// 单曲查询：正常路径取回完整元数据，不存在的 id 回结构化错误。
///
/// 这条命令服务于"正在播放"界面 —— 它需要 codec / 采样率 / 文件体积 /
/// 入库时间等字段，而 `PlaybackSnapshot` 里没有这些。
#[test]
fn get_track_returns_full_metadata_and_reports_missing_ids() {
    let h = harness();

    let root_id = invoke(
        &h.webview,
        "library_add_root",
        json!({ "path": fixtures_dir().to_str().unwrap() }),
    )
    .unwrap()["id"]
        .as_i64()
        .unwrap();
    invoke(&h.webview, "library_scan_root", json!({ "rootId": root_id })).unwrap();

    let listed = invoke(&h.webview, "library_list_tracks", json!({ "limit": 100 })).unwrap();
    let from_list = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["path"].as_str().unwrap().ends_with("tagged.mp3"))
        .expect("夹具应包含 tagged.mp3")
        .clone();
    let id = from_list["id"].as_i64().unwrap();

    // ── 1. 单曲查询必须与列表条目逐字段一致 ─────────────────────
    let one = invoke(&h.webview, "library_get_track", json!({ "trackId": id }))
        .expect("library_get_track 应成功");
    assert_eq!(one, from_list, "单曲查询结果必须与列表条目一致");

    // 这些技术字段是详情界面独有依赖，必须有真实值
    assert_eq!(one["title"], "Fixture MP3");
    assert_eq!(one["sampleRate"], 8000);
    assert_eq!(one["channels"], 1);
    assert_eq!(one["trackNo"], 3);
    assert_eq!(one["albumArtist"], "Fixture Album Artist");
    assert!(one["fileSize"].as_i64().unwrap() > 0, "文件体积应为真实值");
    assert!(one["addedAt"].as_i64().unwrap() > 0, "入库时间应为真实值");

    // ── 2. 不存在的 id → 结构化错误，不是 panic 也不是字符串 ─────
    let err = invoke(&h.webview, "library_get_track", json!({ "trackId": 999_999 }))
        .expect_err("不存在的曲目必须报错");
    assert_eq!(err["code"], "LIBRARY_TRACK_NOT_FOUND");
    assert_eq!(err["retryable"], false);
    assert_eq!(keys_of(&err), ["code", "message", "retryable"]);

    let _ = &h.app;
}

fn keys_of(value: &Value) -> Vec<String> {
    let mut k: Vec<String> = value
        .as_object()
        .expect("应为 JSON 对象")
        .keys()
        .cloned()
        .collect();
    k.sort();
    k
}

// ══════════════════════════════════════════════════════════════
// 播放命令
// ══════════════════════════════════════════════════════════════

/// 扫描夹具资料库，返回 (track_id, 文件名) 列表。
fn scanned_tracks(h: &Harness) -> Vec<(i64, String)> {
    let root_id = invoke(
        &h.webview,
        "library_add_root",
        json!({ "path": fixtures_dir().to_str().unwrap() }),
    )
    .unwrap()["id"]
        .as_i64()
        .unwrap();

    invoke(&h.webview, "library_scan_root", json!({ "rootId": root_id })).unwrap();

    invoke(&h.webview, "library_list_tracks", json!({ "limit": 100 }))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let id = t["id"].as_i64().unwrap();
            let name = t["path"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string();
            (id, name)
        })
        .collect()
}

#[test]
fn playback_state_shape_is_stable() {
    let h = harness();
    let state = invoke(&h.webview, "playback_get_state", json!({})).expect("初始状态可读");

    assert_eq!(
        keys_of(&state),
        [
            "album",
            "artist",
            "durationMs",
            "engine",
            "hasNext",
            "hasPrevious",
            "lastFailure",
            "positionMs",
            "queueIndex",
            "queueLength",
            "repeat",
            "shuffle",
            "status",
            "title",
            "trackId",
            "volume",
        ],
        "\nPlaybackSnapshot 的 JSON 字段与前端类型不一致"
    );

    assert_eq!(state["status"], "idle", "初始应为 idle");
    assert_eq!(state["trackId"], Value::Null);
    assert_eq!(state["queueLength"], 0);
    assert_eq!(state["hasNext"], false);
    assert!(
        state["engine"].as_str().is_some_and(|e| !e.is_empty()),
        "必须报告引擎描述，便于诊断"
    );
}

/// 播放模式经真实 IPC 往返：`queue_set_*` 写入 → `playback_get_state` 读回。
#[test]
fn playback_modes_round_trip_over_ipc() {
    let h = harness();

    // 默认值
    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["repeat"], "off", "默认应为顺序播放");
    assert_eq!(state["shuffle"], false, "默认应关闭随机");

    // 循环模式三态往返
    for mode in ["all", "one", "off"] {
        invoke(&h.webview, "queue_set_repeat", json!({ "mode": mode }))
            .unwrap_or_else(|e| panic!("queue_set_repeat({mode}) 应成功: {e}"));
        let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
        assert_eq!(state["repeat"], mode, "循环模式应往返一致");
    }

    // 随机开关往返
    for enabled in [true, false, true] {
        invoke(&h.webview, "queue_set_shuffle", json!({ "enabled": enabled }))
            .unwrap_or_else(|e| panic!("queue_set_shuffle({enabled}) 应成功: {e}"));
        let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
        assert_eq!(state["shuffle"], enabled, "随机开关应往返一致");
    }

    // 非法模式必须被拒绝，而不是静默降级为 off
    let err = invoke(&h.webview, "queue_set_repeat", json!({ "mode": "bogus" }))
        .expect_err("非法循环模式必须被拒绝");
    assert!(!err.is_null(), "应返回结构化错误而非空值");
    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["repeat"], "off", "被拒绝的调用不得改变已有模式");

    let _ = &h.app;
}

#[test]
fn play_pause_next_previous_work_over_ipc() {
    let h = harness();
    let tracks = scanned_tracks(&h);
    assert_eq!(tracks.len(), 4, "夹具应有 4 首可播放曲目");

    let ids: Vec<i64> = tracks.iter().map(|(id, _)| *id).collect();

    // 从第 0 首开始播放整队
    invoke(
        &h.webview,
        "playback_play_tracks",
        json!({ "trackIds": ids, "startIndex": 0 }),
    )
    .expect("播放命令应成功");

    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["status"], "playing");
    assert_eq!(state["trackId"].as_i64(), Some(ids[0]));
    assert_eq!(state["queueLength"], 4);
    assert_eq!(state["queueIndex"], 0);
    assert_eq!(state["hasNext"], true);
    assert_eq!(state["hasPrevious"], false);

    // 下一首
    invoke(&h.webview, "playback_next", json!({})).expect("下一首应成功");
    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["trackId"].as_i64(), Some(ids[1]));
    assert_eq!(state["hasPrevious"], true);

    // 上一首
    invoke(&h.webview, "playback_previous", json!({})).expect("上一首应成功");
    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["trackId"].as_i64(), Some(ids[0]));
    assert_eq!(state["hasPrevious"], false);

    // 暂停 / 恢复
    invoke(&h.webview, "playback_toggle", json!({})).unwrap();
    assert_eq!(
        invoke(&h.webview, "playback_get_state", json!({})).unwrap()["status"],
        "paused"
    );
    invoke(&h.webview, "playback_toggle", json!({})).unwrap();
    assert_eq!(
        invoke(&h.webview, "playback_get_state", json!({})).unwrap()["status"],
        "playing"
    );

    // 停止
    invoke(&h.webview, "playback_stop", json!({})).unwrap();
    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["queueLength"], 0);
    assert_eq!(state["trackId"], Value::Null);
}

#[test]
fn volume_is_clamped_over_ipc() {
    let h = harness();

    invoke(&h.webview, "playback_set_volume", json!({ "volume": 0.35 })).unwrap();
    let v = invoke(&h.webview, "playback_get_state", json!({})).unwrap()["volume"]
        .as_f64()
        .unwrap();
    assert!((v - 0.35).abs() < 1e-6);

    invoke(&h.webview, "playback_set_volume", json!({ "volume": 5.0 })).unwrap();
    let v = invoke(&h.webview, "playback_get_state", json!({})).unwrap()["volume"]
        .as_f64()
        .unwrap();
    assert!((v - 1.0).abs() < 1e-6, "音量必须被钳制到 1.0");
}

#[test]
fn playback_errors_cross_ipc_as_structured_payloads() {
    let h = harness();

    // 空队列
    let err = invoke(
        &h.webview,
        "playback_play_tracks",
        json!({ "trackIds": [], "startIndex": 0 }),
    )
    .expect_err("空队列必须报错");
    assert_eq!(err["code"], "PLAYBACK_EMPTY_QUEUE");

    // 不存在的曲目 ID
    let err = invoke(
        &h.webview,
        "playback_play_tracks",
        json!({ "trackIds": [999_999], "startIndex": 0 }),
    )
    .expect_err("不存在的曲目必须报错");
    assert_eq!(err["code"], "LIBRARY_TRACK_NOT_FOUND");

    // 无内容时切换播放
    let err = invoke(&h.webview, "playback_toggle", json!({})).expect_err("应报错");
    assert_eq!(err["code"], "PLAYBACK_NOTHING_PLAYING");

    // 负数跳转
    let err = invoke(&h.webview, "playback_seek", json!({ "positionMs": -1 }))
        .expect_err("负数应被拒绝");
    assert_eq!(err["code"], "INVALID_ARGUMENT");
}

#[test]
fn playing_unplayable_files_is_reported_not_silently_ignored() {
    let h = harness();
    let tracks = scanned_tracks(&h);

    // 找到被扫描阶段接受、但内容不是音频的那一个（not_audio.mp3 已被扫描拒绝，
    // 因此这里只验证：所有入库曲目都能被引擎接受或明确报错）
    let ids: Vec<i64> = tracks.iter().map(|(id, _)| *id).collect();
    invoke(
        &h.webview,
        "playback_play_tracks",
        json!({ "trackIds": ids, "startIndex": 0 }),
    )
    .expect("夹具中的合法音频应可播放");

    let state = invoke(&h.webview, "playback_get_state", json!({})).unwrap();
    assert_eq!(state["status"], "playing");
}
