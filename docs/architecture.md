# Aurora Music — 架构设计

> 文档版本：0.1.0 · 状态：待评审 · 最后更新：2026-10-15
>
> 需求编号引用见 `requirements.md`。里程碑见 `roadmap.md`。测试策略见 `testing.md`。

---

## 1. 架构总览

### 1.1 分层

```
┌──────────────────────────────────────────────────────────────────────────┐
│  表现层  React (UI only)                                                  │
│  视图 / 组件 / 状态容器                                                   │
│  ⛔ 禁止：音频 API、SQL、直接 fetch、file:// 访问                          │
└───────────────────────────────┬──────────────────────────────────────────┘
                                │  ① Tauri IPC（invoke / events）
                                │     唯一通道，经 ipc/ 层封装
┌───────────────────────────────▼──────────────────────────────────────────┐
│  命令层  commands/                                                        │
│  薄适配器：入参校验 → 调用服务 → DTO 映射 → 错误序列化                     │
│  ⛔ 禁止：业务逻辑、直接触碰 DB/文件/网络                                  │
└───────────────────────────────┬──────────────────────────────────────────┘
                                │  ② 普通 Rust 函数调用
┌───────────────────────────────▼──────────────────────────────────────────┐
│  应用层  app/  (Services)                                                 │
│  LibraryService · PlaybackService · QueueService · SearchService          │
│  ArtworkService · LyricsService · SourceRegistry                          │
│  职责：编排、事务边界、并发控制、事件发布                                  │
└──────────────┬────────────────────────────────────┬──────────────────────┘
               │  ③ 依赖的是 trait（端口），不是具体类型
┌──────────────▼───────────────┐   ┌────────────────▼─────────────────────┐
│  领域层  domain/             │   │  基础设施层  infra/                   │
│  · 数据模型 (model.rs)       │◄──┤  实现 domain 定义的 ports：            │
│  · 错误类型 (error.rs)       │   │   db/ · scan/ · audio/ · artwork/     │
│  · 队列逻辑 (queue.rs)       │   │   lyrics/ · sources/ · desktop/       │
│  · ResolvedStream (stream.rs)│   │   logging.rs                          │
│  · 端口即 trait (ports.rs)   │   │                                       │
│  零 I/O，纯逻辑，可完整单测   │   │  外部世界只在这里被触碰                │
└──────────────────────────────┘   └───────────────────────────────────────┘
```

### 1.2 依赖规则（编译期可检查）

| 规则 | 说明 |
|---|---|
| R1 | `domain/` 不依赖任何其他层，也不依赖 `tauri`、`rusqlite`、`rodio`、`reqwest` |
| R2 | `app/` 只依赖 `domain/` 的 trait，不依赖 `infra/` 的具体类型 |
| R3 | `infra/` 依赖 `domain/`，实现其 trait |
| R4 | `commands/` 依赖 `app/` 与 `domain/`，不依赖 `infra/` |
| R5 | 唯一的组装点（依赖注入）在 `lib.rs` 的 `setup()` |
| R6 | `domain/` 中不允许出现 `unwrap()` / `expect()`（测试除外） |

> R2 + R5 的组合使得：把 `RodioAudioEngine` 换成 `NullAudioEngine` 只改一处装配代码，这是整个测试策略的前提。

### 1.3 对应设计原则

| 原则 | 结构性保证 |
|---|---|
| P1 React 不操作音频设备 | 前端无音频 API；`AudioOutput` trait 仅存在于 Rust |
| P2 明确可测试的命令接口 | 全部 I/O 经 `commands/`；DB 不对前端暴露；服务层可脱离 Tauri 单测 |
| P3 来源与引擎解耦 | 二者只通过 `ResolvedStream` 值对象通信，互不知晓对方存在 |
| P4 禁止未验证 URL 进入引擎 | `ResolvedStream` 字段私有 + 唯一校验构造器；引擎签名只接受该类型 |
| P5 用户数据持久化 | SQLite 为唯一真相源；队列/进度/歌单均落库 |
| P6 先 MVP | 里程碑门禁，见 `roadmap.md` |

---

## 2. 模块职责

对照 `requirements.md` 要求的 11 个模块：

### 2.1 音乐资料库（Library）

- **位置：** `app/library_service.rs` + `infra/db/repo/tracks.rs`
- **职责：** 曲目 CRUD、按专辑/艺术家/文件夹聚合查询、库统计
- **不负责：** 文件系统遍历（属 scan）、标签解析（属 scan）
- **对外：** `list_tracks` `get_track` `list_albums` `list_artists` `list_album_tracks` `library_stats` `remove_tracks`

**专辑聚合的关键决策：** 专辑身份不能只用专辑名（"Greatest Hits" 会撞车）。采用复合键：

```
album_key = normalize(album_artist) ⊕ '\u{1f}' ⊕ normalize(album)
```

其中 `normalize` = 大小写折叠 + 去除首尾空白 + Unicode NFKC。若 `album_artist` 为空则回退到 `artist`。`album_key` 作为生成列持久化，避免每次查询重算。

### 2.2 本地文件扫描（Scanning）

- **位置：** `infra/scan/`
- **职责：** 遍历目录、扩展名过滤、增量判定、标签提取、变更检测、进度上报
- **运行于：** 独立 tokio 任务，通过 `CancellationToken` 可取消
- **关键约束：**
  - 默认**不跟随符号链接**（防循环）
  - 单文件失败 → 记入错误列表，继续扫描
  - **对音乐目录只读**，绝不写入（NG-4）
  - 进度事件节流至 ≤ 5 Hz

**扫描流程：**

```
ScanRequest { root_ids, mode }
   │
   ├─ 1. 加载根目录 → 规范化路径（canonicalize，解析 ..）
   ├─ 2. walkdir 遍历，按扩展名白名单过滤
   │       └─ 对每个文件：stat(size, mtime)
   ├─ 3. 与 DB 中 (path, file_size, file_mtime) 比对
   │       ├─ 完全一致        → 跳过（增量扫描的核心）
   │       └─ 新增 / 有变化   → 进入第 4 步
   ├─ 4. 并行探测（有界线程池，N = CPU 核数）
   │       ├─ lofty 读标签
   │       ├─ 计算 album_key
   │       └─ 不在此处解码音频（代价过高）
   ├─ 5. 批量 upsert（单事务，每 500 条提交一次）
   ├─ 6. 标记本次未出现且属于该根的曲目为「缺失」
   │       └─ 不删除文件，仅置 tracks.missing_since
   └─ 7. 写入 scan_sessions 统计
```

**为什么第 4 步不解码音频：** 首版资料库不需要精确时长以外的音频信息。时长可由 lofty 从标签或帧头获得，无需完整解码。全量解码 10 万文件不可接受（NFR-1.3）。

### 2.3 音频播放引擎（Audio Engine）

- **位置：** `infra/audio/rodio_engine.rs`（实现）+ `infra/audio/null_engine.rs`（测试替身）
- **职责：** 解码、输出、暂停/恢复/跳转/音量、位置上报、曲目结束通知
- **线程模型：** cpal 输出流独占一个线程；引擎对象通过消息通道接收命令

**核心 trait（定义于 `domain/ports.rs`）：**

```rust
/// 播放引擎端口。实现必须是非阻塞的：所有方法立即返回，
/// 实际工作经内部通道交给音频线程。
pub trait AudioOutput: Send + Sync {
    fn load(&self, stream: ResolvedStream) -> Result<(), PlaybackError>;
    fn play(&self) -> Result<(), PlaybackError>;
    fn pause(&self) -> Result<(), PlaybackError>;
    fn stop(&self) -> Result<(), PlaybackError>;
    fn seek(&self, pos: Duration) -> Result<(), PlaybackError>;
    fn set_volume(&self, v: f32) -> Result<(), PlaybackError>;

    /// 从引擎启动起算的单调时间轴位置；无曲目时为 None。
    fn position(&self) -> Option<Duration>;
    fn state(&self) -> TransportState;

    /// 引擎事件出口。PlaybackService 在此消费 TrackEnded / DecodeError / DeviceLost。
    fn take_events(&self) -> Vec<EngineEvent>;
}

pub enum TransportState { Idle, Loading, Ready, Playing, Paused, Stopped, Error }
pub enum EngineEvent {
    TrackEnded,
    DecodeError { message: String },
    DeviceLost { message: String },
    PositionTick { pos: Duration },
}
```

**曲目结束检测（本模块最易踩坑处）：**

⚠️ **实测修正：** rodio 0.23 的 API 与 0.21 及更早版本**完全不同**。
`OutputStream` / `Sink` 已不存在，取而代之的是：

| 旧 API（≤0.21） | rodio 0.23 实际 API |
|---|---|
| `OutputStream::try_default()` | `DeviceSinkBuilder::open_default_sink() -> MixerDeviceSink` |
| `Sink::try_new(&handle)` | `Player::connect_new(mixer)` 或 `rodio::play(mixer, src)` |
| `sink.get_pos()` | `Player::get_pos() -> Duration` ✅ 仍然存在 |
| `sink.empty()` / `len()` / `skip_one()` | 同名保留在 `Player` 上 |

**这带来一个好消息：** `Player::get_pos()` 直接给出播放位置，
因此**不需要**靠自定义 `Source` 数采样帧来推算位置。

曲目结束仍可用 `Player::empty()` 判定，但它无法区分"播完"与"尚未开始"。
对于需要精确边界的场景（无缝播放、精确历史结算），仍建议用自定义
`Source` 包装器投递结束事件：

```rust
/// 计数采样帧，耗尽时向事件通道投递 TrackEnded，然后返回 None。
struct EndingDetector<S> { inner: S, tx: Sender<EngineEvent>, frames_emitted: u64 }
impl<S: Source> Iterator for EndingDetector<S> { /* ... */ }
```

`Source` trait 在 0.23 中仍是 `Iterator<Item = Sample>`，因此该包装方式依然有效。

> **另据实测：** `DecoderBuilder::with_gapless(true)` 与 `.with_byte_len(len)` 均已存在。
> 后者对 MP3 / Vorbis 的**可靠 seek 与时长计算是必需的**，构建解码器时必须传入字节长度。

### 2.4 播放队列（Queue）

- **位置：** `domain/queue.rs`（纯逻辑）+ `app/queue_service.rs`（编排）+ `infra/db/repo/queue.rs`（持久化）
- **职责：** 队列增删改序、当前索引、播放模式、随机置换
- **真相源：** Rust 侧 `PlayQueue`，DB 为持久化副本，前端为只读视图

```rust
pub struct PlayQueue {
    items: Vec<QueueItem>,
    current: Option<usize>,
    repeat: RepeatMode,          // Off | All | One
    shuffle: Option<ShuffleState>,
}

pub struct QueueItem {
    pub track_id: TrackId,
    pub origin: QueueOrigin,     // User | Album(AlbumKey) | Playlist(PlaylistId)
    pub added_at: SystemTime,
}

pub struct ShuffleState {
    /// 播放顺序：order[i] 表示第 i 个播放的是 items 的哪个下标
    order: Vec<usize>,
}
```

**随机播放的正确实现（FR-3.5）：**

朴素的"每次随机选一首"会让"上一首"得到不同结果，用户会立刻察觉。正确做法是生成一次性置换并保存：

```
开启随机：
  order = [0..n] 经 Fisher-Yates 洗牌
  调整使当前曲目位于 order 首位
  → 持久化 order
下一首：在 order 中找到 current 的位置，取下一个
上一首：取前一个（不需要任何随机数）
向队列追加：order.push(items.len() - 1)（新曲排在最后）
```

`order` 持久化后，重启、切歌、跳转都不会破坏顺序。

### 2.5 搜索（Search）

- **位置：** `app/search_service.rs` + `infra/db/repo/search.rs`
- **实现：** SQLite FTS5 外部内容表 + 触发器同步
- **职责：** 跨字段检索、前缀优先排序、结果分组

```sql
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, album_artist,
    content='tracks', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
```

- `remove_diacritics 2` 满足 FR-4.3（变音符号不敏感）
- CJK 场景下 `unicode61` 不会切分中日韩词，因此**对 CJK 查询额外走 `LIKE` 前缀匹配**作为补充路径（FR-4.4）
- 排序策略：前缀命中 > 词首命中 > 包含命中，同档按 `play_count` 降序

> 备选方案：若实测 FTS5 在 CJK 上表现不可接受，改用 `simple` 分词器或自建 bigram 索引。此决策推迟到 M3 实测后。

### 2.6 收藏与歌单（Favorites & Playlists）

- **位置：** `app/playlist_service.rs` + `infra/db/repo/playlists.rs`
- **收藏的建模决策：** 使用**独立的 `favorites` 表**，而非保留歌单。
  - 理由：语义不同（全局唯一集合 vs 用户可创建的多个列表）
  - 避免"用户删除了收藏歌单"这一尴尬状态
  - 查询更简单，无需 `WHERE kind='favorites'` 过滤

### 2.7 最近播放（History）

- **位置：** `app/history_service.rs` + `infra/db/repo/history.rs`
- **职责：** 记录播放事件、去重视图、播放计数
- **写入时机：** 曲目开始播放时插入一条 `play_history`，播放结束或切歌时更新 `ms_played` 与 `completed`
- **去重视图：** `recently_played` 以 `track_id` 分组取 `MAX(played_at)`

### 2.8 封面缓存（Artwork）

- **位置：** `infra/artwork/`
- **存储布局：**

```
$XDG_CACHE_HOME/aurora-music/artwork/
  └── <hash[0..2]>/<hash[2..4]>/<hash>_<size>.webp
      hash = blake3(源图片字节)
```

- **写入目标永远是缓存目录**，绝不写音乐目录（FR-7.5，NG-4）
- **提取来源优先级：** 内嵌封面 → 同目录 `cover.*` / `folder.*` / `album.*` → 生成占位图
- **尺寸档位：** 64 / 256 / 640 px（WebP 有损 quality=85）
- **淘汰：** 按目录总字节预算做 LRU，`artwork.last_accessed_at` 驱动
- **投递方式：** 注册 Tauri 自定义 URI 协议（`register_asynchronous_uri_scheme_protocol`），前端以 `<img src="...">` 引用

> 为什么不用 base64 走 IPC：一张 640px 封面 base64 后约 300 KB，虚拟列表滚动时会产生大量大字符串序列化，直接违反 NFR-1.7。自定义协议让 WebKit 用自己的图片管线与 HTTP 缓存处理。
>
> ⚠️ 自定义协议在不同平台的 URL 形态不同（Linux 上为 `scheme://host/path`）。**实现时必须实测确认**，不要假定。

### 2.9 歌词（Lyrics）

- **位置：** `infra/lyrics/`
- **获取顺序：** 同名 `.lrc` → 内嵌标签（USLT / `©lyr`）→ 在线（M5 之后，且须经来源校验）→ 无
- **编码处理（CJK 必需，FR-8.4）：** LRC 文件常为 GBK / Shift-JIS / Big5。流程：
  1. 先验 BOM
  2. 无 BOM 则试 UTF-8 严格解码
  3. 失败则用 `chardetng` 猜测，再用 `encoding_rs` 解码
- **解析产物：** `Vec<LyricLine { at: Duration, text: String }>`；纯文本降级为单段
- **同步：** 前端以播放位置二分查找当前行；偏移微调存于 DB

### 2.10 在线来源适配器（Online Sources）

- **位置：** `infra/sources/`（实现）+ `domain/ports.rs`（trait）
- **注册表：** `SourceRegistry` 持有 `Vec<Arc<dyn MusicSource>>`，`local` 为内置且不可移除

```rust
#[async_trait]
pub trait MusicSource: Send + Sync {
    fn id(&self) -> SourceId;
    fn display_name(&self) -> &str;
    fn capabilities(&self) -> SourceCapabilities;   // 是否支持搜索/歌词/封面

    async fn search(&self, query: &SourceQuery) -> Result<Vec<TrackCandidate>, SourceError>;

    /// 解析为可播放地址。实现方**不得**返回未校验的裸 URL；
    /// 必须构造 ResolvedStream，由构造函数完成全部校验。
    async fn resolve(&self, candidate: &TrackCandidate)
        -> Result<ResolvedStream, SourceError>;
}
```

**P4 的类型级强制（本设计的核心安全机制）：**

```rust
/// 已通过全部校验的可播放流。字段私有——外部无法直接构造。
pub struct ResolvedStream {
    url: Url,                            // 私有
    mime: AudioMime,                     // 私有，白名单枚举
    content_length: Option<u64>,         // 私有
    expires_at: Option<SystemTime>,      // 私有
    source_id: SourceId,                 // 私有
}

impl ResolvedStream {
    /// 唯一构造入口，执行全部校验。校验失败 ⇒ 无法得到该类型的值。
    pub fn validate(
        raw: &str,
        source: &dyn MusicSource,
        policy: &StreamPolicy,
    ) -> Result<Self, StreamRejection> {
        // 1. 可解析为绝对 URL
        // 2. scheme 必须为 https（localhost 除外，仅开发模式）
        // 3. host 必须属于该来源声明的白名单
        // 4. 禁止 userinfo（user:pass@）
        // 5. 端口必须在允许集合内
        // 6. 不得为 file:// / data: / javascript: / blob:
        // 7. mime 必须在 AudioMime 白名单内
        // 8. 体积不超过上限
        // ...
    }

    pub fn url(&self) -> &Url { &self.url }
}
```

`AudioOutput::load()` 的参数类型是 `ResolvedStream`，不是 `String`。因此**编译器保证**未经验证的地址无法进入音频引擎——这比运行时检查强得多（P4）。

**重定向处理：** 适配器需自行跟随重定向（`reqwest` 设置 `redirect::Policy::none()`），逐跳重新执行上述校验，并限制跳数 ≤ 5。这样"初次校验合法但重定向到恶意地址"的绕过路径被关闭。

### 2.11 SQLite 数据访问层（DAL）

- **位置：** `infra/db/`
- **选型：** `rusqlite` + `bundled` feature + `r2d2` 连接池
- **决策依据：**

| 方案 | 评估 |
|---|---|
| `rusqlite` + bundled | ✅ **采用**。SQLite 编译进二进制，无系统依赖；FTS5 默认可用；同步 API 语义简单；测试可用 `:memory:` |
| `sqlx` | ❌ 编译期查询校验需要构建时提供 `DATABASE_URL` 或 `cargo sqlx prepare`，引入构建脆弱性；对本地文件库收益有限 |
| `tauri-plugin-sql` | ❌ **明确拒绝**。它把 SQL 执行能力暴露给前端，直接违反 P2 与 NFR-4.1 |

- **连接池：** `r2d2_sqlite`，池大小 4（读多写少，本地库不需要更大）
- **PRAGMA（连接建立时统一设置）：**

```sql
PRAGMA journal_mode = WAL;        -- 读写并发（NFR-2.1）
PRAGMA synchronous  = NORMAL;     -- WAL 下的合理折中
PRAGMA foreign_keys = ON;         -- 级联删除生效
PRAGMA busy_timeout = 5000;
PRAGMA temp_store   = MEMORY;
```

- **迁移：** `user_version` 驱动的顺序迁移，每个迁移在单事务内执行（NFR-2.3）
- **阻塞问题：** rusqlite 是同步的。**所有 DB 调用必须经 `spawn_blocking` 或专用线程池**，禁止在异步运行时线程上直接执行（NFR-1.7）。

### 2.12 错误处理

见 §6。

### 2.13 日志（Logging）

见 §7。

---

## 3. 数据模型

### 3.1 完整 DDL（v1）

```sql
-- ─────────────────────────────────────────────────────────────
-- 资料库根目录
-- ─────────────────────────────────────────────────────────────
CREATE TABLE library_roots (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL UNIQUE,      -- 规范化的绝对路径
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_scan_at INTEGER,
    created_at   INTEGER NOT NULL
);

-- ─────────────────────────────────────────────────────────────
-- 曲目
-- ─────────────────────────────────────────────────────────────
CREATE TABLE tracks (
    id             INTEGER PRIMARY KEY,
    source_id      TEXT    NOT NULL DEFAULT 'local',   -- 'local' | 'online:<adapter>'
    external_id    TEXT,                                -- 在线来源曲目 id；本地为 NULL

    path           TEXT,                                -- 本地绝对路径；在线为 NULL
    root_id        INTEGER REFERENCES library_roots(id) ON DELETE SET NULL,

    -- 标签元数据
    title          TEXT    NOT NULL,
    artist         TEXT,
    album          TEXT,
    album_artist   TEXT,
    album_key      TEXT,                                -- 见 §2.1
    track_no       INTEGER,
    disc_no        INTEGER,
    year           INTEGER,
    genre          TEXT,

    -- 技术元数据
    duration_ms    INTEGER,
    sample_rate    INTEGER,
    channels       INTEGER,
    bitrate        INTEGER,
    codec          TEXT,
    container      TEXT,

    -- 变更检测（增量扫描）
    file_size      INTEGER,
    file_mtime     INTEGER,

    -- 状态
    has_embedded_art INTEGER NOT NULL DEFAULT 0,
    missing_since  INTEGER,                             -- 非 NULL 表示文件已消失
    play_count     INTEGER NOT NULL DEFAULT 0,          -- 反规范化，由 history 维护
    added_at       INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,

    CHECK (source_id <> 'local' OR path IS NOT NULL),
    CHECK (source_id =  'local' OR external_id IS NOT NULL)
);

CREATE UNIQUE INDEX idx_tracks_path     ON tracks(path)               WHERE path IS NOT NULL;
CREATE UNIQUE INDEX idx_tracks_external ON tracks(source_id, external_id) WHERE external_id IS NOT NULL;
CREATE INDEX idx_tracks_album_key  ON tracks(album_key);
CREATE INDEX idx_tracks_artist     ON tracks(artist);
CREATE INDEX idx_tracks_root       ON tracks(root_id);
CREATE INDEX idx_tracks_missing    ON tracks(missing_since) WHERE missing_since IS NOT NULL;

-- ─────────────────────────────────────────────────────────────
-- 扫描会话（FR-1.8，也为诊断提供依据）
-- ─────────────────────────────────────────────────────────────
CREATE TABLE scan_sessions (
    id             INTEGER PRIMARY KEY,
    root_id        INTEGER NOT NULL REFERENCES library_roots(id) ON DELETE CASCADE,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    status         TEXT    NOT NULL DEFAULT 'running',
                   -- 'running' | 'completed' | 'cancelled' | 'failed'
    files_seen     INTEGER NOT NULL DEFAULT 0,
    files_added    INTEGER NOT NULL DEFAULT 0,
    files_updated  INTEGER NOT NULL DEFAULT 0,
    files_skipped  INTEGER NOT NULL DEFAULT 0,
    files_failed   INTEGER NOT NULL DEFAULT 0,
    error_summary  TEXT
);
CREATE INDEX idx_scan_sessions_root ON scan_sessions(root_id, started_at DESC);

-- ─────────────────────────────────────────────────────────────
-- 歌单
-- ─────────────────────────────────────────────────────────────
CREATE TABLE playlists (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    description TEXT,
    cover_hash  TEXT,                       -- 自定义封面（可选）
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_playlists_name ON playlists(name);

CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks(id)    ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position)
);
CREATE INDEX idx_playlist_items_track ON playlist_items(track_id);
-- 说明：同一曲目允许在一张歌单中重复出现；position 用稀疏整数（步长 1024），
--       重排时只更新被移动项，避免整表重写。

-- ─────────────────────────────────────────────────────────────
-- 收藏（§2.6 决策：独立表）
-- ─────────────────────────────────────────────────────────────
CREATE TABLE favorites (
    track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL
);
CREATE INDEX idx_favorites_added ON favorites(added_at DESC);

-- ─────────────────────────────────────────────────────────────
-- 播放历史
-- ─────────────────────────────────────────────────────────────
CREATE TABLE play_history (
    id           INTEGER PRIMARY KEY,
    track_id     INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    started_at   INTEGER NOT NULL,
    ms_played    INTEGER NOT NULL DEFAULT 0,
    completed    INTEGER NOT NULL DEFAULT 0,
    origin       TEXT                                 -- 'queue' | 'album' | 'playlist'
);
CREATE INDEX idx_history_track ON play_history(track_id, started_at DESC);
CREATE INDEX idx_history_time  ON play_history(started_at DESC);

-- 去重视图（FR-6.2）
CREATE VIEW recently_played AS
SELECT track_id, MAX(started_at) AS last_played_at, COUNT(*) AS times
FROM play_history
GROUP BY track_id;

-- ─────────────────────────────────────────────────────────────
-- 队列持久化（FR-3.6）
-- ─────────────────────────────────────────────────────────────
CREATE TABLE queue_items (
    position   INTEGER PRIMARY KEY,
    track_id   INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    origin     TEXT    NOT NULL DEFAULT 'user',
    origin_ref TEXT,                                  -- album_key / playlist_id
    added_at   INTEGER NOT NULL
);

-- 单行状态表：随机顺序、循环模式、断点位置
CREATE TABLE playback_state (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    current_item   INTEGER,                -- queue_items.position
    position_ms    INTEGER NOT NULL DEFAULT 0,
    volume         REAL    NOT NULL DEFAULT 0.8,
    repeat_mode    TEXT    NOT NULL DEFAULT 'off',   -- 'off' | 'all' | 'one'
    shuffle_order  TEXT,                   -- JSON 数组；NULL 表示未开启随机
    updated_at     INTEGER NOT NULL
);

-- ─────────────────────────────────────────────────────────────
-- 封面缓存元数据
-- ─────────────────────────────────────────────────────────────
CREATE TABLE artwork (
    id               INTEGER PRIMARY KEY,
    hash             TEXT    NOT NULL UNIQUE,       -- blake3(源图片字节)
    kind             TEXT    NOT NULL,              -- 'embedded' | 'file' | 'downloaded' | 'generated'
    mime             TEXT    NOT NULL,
    width            INTEGER,
    height           INTEGER,
    byte_size        INTEGER NOT NULL,
    created_at       INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL
);
CREATE INDEX idx_artwork_lru ON artwork(last_accessed_at);

-- 曲目/专辑 → 封面
CREATE TABLE artwork_refs (
    id         INTEGER PRIMARY KEY,
    artwork_id INTEGER NOT NULL REFERENCES artwork(id) ON DELETE CASCADE,
    track_id   INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    album_key  TEXT,
    CHECK (track_id IS NOT NULL OR album_key IS NOT NULL)
);
CREATE INDEX idx_artwork_refs_track ON artwork_refs(track_id);
CREATE INDEX idx_artwork_refs_album ON artwork_refs(album_key);

-- ─────────────────────────────────────────────────────────────
-- 歌词
-- ─────────────────────────────────────────────────────────────
CREATE TABLE lyrics (
    id         INTEGER PRIMARY KEY,
    track_id   INTEGER NOT NULL UNIQUE REFERENCES tracks(id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL,          -- 'synced' | 'plain' | 'none'
    source     TEXT    NOT NULL,          -- 'lrc_file' | 'embedded' | 'online'
    content    TEXT    NOT NULL,
    offset_ms  INTEGER NOT NULL DEFAULT 0,
    encoding   TEXT,                      -- 原始编码，供诊断
    fetched_at INTEGER NOT NULL
);

-- ─────────────────────────────────────────────────────────────
-- 搜索索引（外部内容表 + 触发器同步）
-- ─────────────────────────────────────────────────────────────
CREATE VIRTUAL TABLE tracks_fts USING fts5(
    title, artist, album, album_artist,
    content='tracks', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album, album_artist)
    VALUES (new.id, new.title, new.artist, new.album, new.album_artist);
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, album_artist)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.album_artist);
END;
CREATE TRIGGER tracks_au AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album, album_artist)
    VALUES ('delete', old.id, old.title, old.artist, old.album, old.album_artist);
    INSERT INTO tracks_fts(rowid, title, artist, album, album_artist)
    VALUES (new.id, new.title, new.artist, new.album, new.album_artist);
END;

-- ─────────────────────────────────────────────────────────────
-- 来源配置
-- ─────────────────────────────────────────────────────────────
CREATE TABLE source_configs (
    source_id    TEXT PRIMARY KEY,
    enabled      INTEGER NOT NULL DEFAULT 0,     -- FR-9.2：默认关闭
    config_json  TEXT NOT NULL DEFAULT '{}',     -- 非敏感配置
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
-- ⚠️ 凭据不入此表。凭据存于系统密钥环，此表仅存非敏感配置。
```

### 3.2 数据库位置

```
$XDG_DATA_HOME/aurora-music/aurora.db        # 数据库
$XDG_CACHE_HOME/aurora-music/artwork/        # 封面缓存
$XDG_STATE_HOME/aurora-music/logs/           # 日志
```

均通过 `directories` crate 解析，不硬编码 `~/.config` 等路径。

---

## 4. 命令接口（Tauri IPC）

### 4.1 约定

- 命名：`<域>_<动作>`，snake_case
- 所有命令返回 `Result<T, AppError>`；`AppError` 序列化见 §6.3
- 命令体内**不写业务逻辑**，仅做参数校验与调用转接
- 前端只能通过 `src/ipc/client.ts` 调用；该文件是全项目唯一出现 `invoke()` 的位置

### 4.2 命令清单（按里程碑）

```rust
// ── 资料库 ─────────────────────────────────────────────────
library_add_root(path: String) -> LibraryRoot
library_remove_root(root_id: i64) -> ()
library_list_roots() -> Vec<LibraryRoot>
library_scan_start(root_ids: Vec<i64>, mode: ScanMode) -> ScanId
library_scan_cancel(scan_id: i64) -> ()
library_scan_status(scan_id: i64) -> ScanStatus
library_list_tracks(query: TrackQuery) -> Paged<TrackDto>
library_get_track(track_id: i64) -> TrackDto
library_list_albums(sort: AlbumSort) -> Vec<AlbumDto>
library_list_artists() -> Vec<ArtistDto>
library_stats() -> LibraryStats
library_remove_tracks(track_ids: Vec<i64>, delete_files: bool) -> ()
    // ⚠️ delete_files 默认 false，且需前端二次确认

// ── 播放 ───────────────────────────────────────────────────
playback_play_track(track_id: i64, context: Option<PlayContext>) -> ()
playback_toggle() -> ()
playback_stop() -> ()
playback_seek(position_ms: i64) -> ()
playback_set_volume(volume: f32) -> ()
playback_get_state() -> PlaybackStateDto

// ── 队列 ───────────────────────────────────────────────────
queue_get() -> QueueDto
queue_append(track_ids: Vec<i64>) -> ()
queue_play_next(track_ids: Vec<i64>) -> ()
queue_remove(position: i64) -> ()
queue_reorder(from: i64, to: i64) -> ()
queue_jump_to(position: i64) -> ()
queue_clear() -> ()
queue_set_repeat(mode: RepeatMode) -> ()
queue_set_shuffle(enabled: bool) -> ()

// ── 歌单 / 收藏 ────────────────────────────────────────────
playlist_create(name: String) -> PlaylistDto
playlist_rename(id: i64, name: String) -> ()
playlist_delete(id: i64) -> ()
playlist_add_tracks(id: i64, track_ids: Vec<i64>) -> ()
playlist_remove_item(id: i64, position: i64) -> ()
playlist_reorder(id: i64, from: i64, to: i64) -> ()
playlist_list() -> Vec<PlaylistDto>
playlist_get_tracks(id: i64) -> Vec<TrackDto>
favorites_toggle(track_id: i64) -> bool
favorites_list() -> Vec<TrackDto>

// ── 历史 ───────────────────────────────────────────────────
history_recent(limit: i64) -> Vec<TrackDto>
history_most_played(limit: i64) -> Vec<TrackDto>
history_clear() -> ()

// ── 搜索 ───────────────────────────────────────────────────
search_query(q: String, limit: i64) -> SearchResults

// ── 歌词 ───────────────────────────────────────────────────
lyrics_get(track_id: i64) -> Option<LyricsDto>
lyrics_set_offset(track_id: i64, offset_ms: i64) -> ()

// ── 来源 ───────────────────────────────────────────────────
source_list() -> Vec<SourceDto>
source_set_enabled(source_id: String, enabled: bool) -> ()
source_search(source_id: String, q: String) -> Vec<TrackCandidateDto>
source_enqueue(candidate: TrackCandidateDto) -> ()

// ── 诊断 ───────────────────────────────────────────────────
diagnostics_recent_errors(limit: i64) -> Vec<LogEntryDto>
diagnostics_paths() -> AppPathsDto
diagnostics_log_from_frontend(level: String, message: String) -> ()
```

### 4.3 事件（Rust → 前端）

| 事件 | 载荷 | 节流 |
|---|---|---|
| `playback:state` | 完整播放状态 | 状态变更时立即，否则 ≤ 2 Hz |
| `playback:position` | `{ position_ms, duration_ms }` | ≤ 4 Hz |
| `queue:changed` | 队列快照或增量 | 变更时 |
| `scan:progress` | 统计计数 | ≤ 5 Hz |
| `scan:finished` | 会话摘要 | 一次 |
| `library:changed` | 变更类型与影响范围 | ≤ 2 Hz |
| `source:error` | 来源标识 + 错误码 | 即时 |

> **位置更新策略：** Rust 以 4 Hz 推送权威位置；前端用 `requestAnimationFrame` 在两次推送之间按本地时间插值渲染。这样进度条平滑（NFR-1.6）而 IPC 负载极低。每次推送都会重新对齐，防止漂移累积。

---

## 5. 模块调用关系

### 5.1 静态依赖

```
commands/* ──► app/* ──► domain/* (trait)
                 ▲            ▲
                 │            │ implements
            infra/* ──────────┘
                 │
                 ▼ 外部世界：文件系统 · SQLite · 声卡 · 网络 · D-Bus
```

### 5.2 运行时关键路径

**路径 A：播放一首本地曲目**

```
[UI] 点击曲目
  │ invoke playback_play_track(track_id, context)
  ▼
[commands/playback]  校验 id > 0
  ▼
[PlaybackService]
  ├─► QueueService::replace_with_context(track_ids, start_at)   ─► DB 事务写入 queue_items
  ├─► LibraryRepo::get_track(id)                                ─► 取路径
  ├─► LocalSource::resolve(track)                               ─► ResolvedStream::validate()
  ├─► AudioOutput::load(stream)                                 ─► 音频线程
  ├─► AudioOutput::play()
  ├─► HistoryRepo::record_start(id)                             ─► DB 写入
  └─► emit("playback:state")
  ▼
[UI] 状态更新，进度条开始插值动画
```

**路径 B：曲目自然结束**

```
[音频线程] EndingDetector 耗尽 → EngineEvent::TrackEnded
  ▼
[PlaybackService] 轮询 take_events()（100 ms 周期）
  ├─► HistoryRepo::record_end(id, ms_played, completed=true)
  ├─► QueueService::advance(repeat_mode)   ← 返回 Option<TrackId>
  │     ├─ Some(next) → 回到路径 A 的 load 步骤
  │     └─ None       → AudioOutput::stop()，emit state=Stopped
  └─► emit("playback:state")
```

**路径 C：扫描**

```
[UI] invoke library_scan_start(root_ids, mode)
  ▼
[LibraryService::scan]
  ├─► 创建 scan_sessions 行（status='running'）
  ├─► 生成 CancellationToken
  ├─► 启动 tokio 任务：
  │     ├─ walkdir 遍历
  │     ├─ 批量比对 → 探测 → upsert（每 500 条一事务）
  │     ├─ 每 200 ms：emit("scan:progress", counts)
  │     └─ 结束：更新会话 → emit("scan:finished")
  └─► 立即返回 ScanId（不等待）
```

**路径 D：在线来源曲目入队（P4 的完整体现）**

```
[UI] 搜索在线来源 → 点击某候选
  │ invoke source_enqueue(candidate)
  ▼
[SourceRegistry::resolve(source_id, candidate)]
  │
  ├─► <Adapter>::search/resolve  ──► 得到**裸 URL 字符串**
  │        ⚠️ 此刻它还只是一段不可信的文本
  │
  ├─► ResolvedStream::validate(&raw_url, adapter, &policy)
  │        ├─ scheme == https            ✔
  │        ├─ host ∈ adapter 白名单       ✔
  │        ├─ 无 userinfo / 端口合法       ✔
  │        ├─ mime ∈ AudioMime 白名单      ✔
  │        └─ 体积 ≤ 上限                  ✔
  │        └─ 失败 ⇒ Err(StreamRejection) ⇒ 前端显示可读原因，流程终止
  │
  ├─► tracks 表插入 source_id='online:xxx' 行
  ├─► queue_items 追加该 track_id
  └─► 返回 track_id
  ▼
[UI] 队列出现该曲目（与本地曲目完全同构）
```

`ResolvedStream` 的字段私有性保证：**开发者无法在忘记校验的情况下把 URL 送进引擎**——不校验就没有值可传。

---

## 6. 错误处理

### 6.1 分层错误类型

```rust
// domain/error.rs —— 领域层，不含任何 I/O 库类型

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("根目录不存在")]
    RootNotFound { path: PathBuf },
    #[error("扫描已取消")]
    ScanCancelled,
    #[error("无法读取目录")]
    WalkFailed { path: PathBuf, source: String },
    #[error("元数据解析失败")]
    MetadataFailed { path: PathBuf, reason: String },
    #[error("曲目不存在")]
    TrackNotFound { id: i64 },
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("没有可用的音频输出设备")]
    NoOutputDevice,
    #[error("无法解码该文件")]
    DecodeFailed { codec: Option<String>, reason: String },
    #[error("格式不受支持")]
    UnsupportedFormat { codec: String },
    #[error("跳转位置超出范围")]
    SeekOutOfRange { requested_ms: i64, duration_ms: i64 },
    #[error("音频设备连接中断")]
    DeviceLost { reason: String },
    #[error("队列为空")]
    EmptyQueue,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("来源未启用")]
    NotConfigured { source_id: String },
    #[error("流地址校验未通过")]
    StreamRejected(#[from] StreamRejection),
    #[error("网络请求失败")]
    Network { status: Option<u16>, reason: String },
    #[error("需要重新登录")]
    AuthRequired,
    #[error("请求过于频繁")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("来源不支持该操作")]
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("数据库迁移失败")]
    Migration { from: u32, to: u32, reason: String },
    #[error("数据库结构版本高于本程序")]
    SchemaTooNew { db_version: u32, app_version: u32 },
    #[error("数据库操作失败")]
    Query { context: &'static str, reason: String },
    #[error("数据库文件损坏")]
    Corrupt,
}

#[derive(Debug, thiserror::Error)]
pub enum StreamRejection {
    #[error("地址无法解析")]           Unparseable,
    #[error("协议不被允许")]           DisallowedScheme { scheme: String },
    #[error("主机不在白名单内")]       DisallowedHost { host: String },
    #[error("地址包含凭据")]           ContainsCredentials,
    #[error("端口不被允许")]           DisallowedPort { port: u16 },
    #[error("内容类型不是音频")]       NotAudio { mime: String },
    #[error("文件超出体积上限")]       TooLarge { size: u64, limit: u64 },
    #[error("重定向次数过多")]         TooManyRedirects { count: u32 },
    #[error("地址已过期")]             Expired,
}
```

### 6.2 顶层聚合

```rust
// domain/error.rs
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)] Library(#[from] LibraryError),
    #[error(transparent)] Playback(#[from] PlaybackError),
    #[error(transparent)] Source(#[from] SourceError),
    #[error(transparent)] Storage(#[from] StorageError),
    #[error("参数无效")]   InvalidArgument { field: &'static str, reason: String },
    #[error("内部错误")]   Internal { id: String },   // id 用于关联日志，不含细节
}
```

### 6.3 序列化契约

```rust
impl AppError {
    pub fn code(&self) -> &'static str { /* 稳定字符串，见下表 */ }
    pub fn retryable(&self) -> bool { /* ... */ }
}

#[derive(Serialize)]
pub struct ErrorPayload {
    pub code: String,            // 机器可读，前端据此分支
    pub message: String,         // 面向用户，已脱敏
    pub retryable: bool,
    pub correlation_id: String,  // 与日志中的同一条记录关联
}
```

前端消费：

```ts
type AppError = {
  code: string;
  message: string;
  retryable: boolean;
  correlationId: string;
};
```

**错误码表（稳定，不随版本重命名）：**

| code | 来源 | retryable |
|---|---|---|
| `LIBRARY_ROOT_NOT_FOUND` | `RootNotFound` | ✗ |
| `LIBRARY_SCAN_CANCELLED` | `ScanCancelled` | ✓ |
| `LIBRARY_TRACK_NOT_FOUND` | `TrackNotFound` | ✗ |
| `PLAYBACK_NO_OUTPUT_DEVICE` | `NoOutputDevice` | ✓ |
| `PLAYBACK_DECODE_FAILED` | `DecodeFailed` | ✗ |
| `PLAYBACK_UNSUPPORTED_FORMAT` | `UnsupportedFormat` | ✗ |
| `PLAYBACK_SEEK_OUT_OF_RANGE` | `SeekOutOfRange` | ✗ |
| `PLAYBACK_DEVICE_LOST` | `DeviceLost` | ✓ |
| `SOURCE_NOT_CONFIGURED` | `NotConfigured` | ✗ |
| `SOURCE_STREAM_REJECTED` | `StreamRejected` | ✗ |
| `SOURCE_NETWORK` | `Network` | ✓ |
| `SOURCE_AUTH_REQUIRED` | `AuthRequired` | ✗ |
| `SOURCE_RATE_LIMITED` | `RateLimited` | ✓ |
| `STORAGE_MIGRATION_FAILED` | `Migration` | ✗ |
| `STORAGE_SCHEMA_TOO_NEW` | `SchemaTooNew` | ✗ |
| `STORAGE_QUERY_FAILED` | `Query` | ✓ |
| `INTERNAL` | `Internal` | ✓ |

### 6.4 硬性规则

1. `commands/`、`app/`、`infra/` 中**禁止 `unwrap()` / `expect()` / `panic!()`**，`domain/` 亦然。
   - 例外：`main.rs` 启动期不可恢复的初始化失败；加 `#[expect]` 注释说明理由。
   - CI 中以 clippy lint 强制（见 `testing.md`）。
2. **用户可见消息脱敏：** 不得包含绝对路径、URL 的查询串、令牌、用户名。
   - `LibraryError::WalkFailed` 的 `path` 只写日志，不进入 `message`。
3. **不吞错：** `let _ = ...` 需要显式注释说明为何可忽略。
4. **单个组件故障不扩散：** 扫描单文件失败、解码单曲失败均记录后继续（FR-2.6、FR-1.10）。

---

## 7. 日志与可观测性

### 7.1 方案

| 组件 | 版本 | 用途 |
|---|---|---|
| `tracing` | 0.1.44 | 结构化埋点 |
| `tracing-subscriber` | 0.3.23 | 组合 EnvFilter + 双输出 |
| `tracing-appender` | 0.2.5 | 按日滚动的文件写入 |

**双输出：** 终端（开发者，彩色，`debug`）+ 文件（用户，`info`，无颜色）。
级别由 `RUST_LOG` 覆盖，默认 `aurora_music=info,warn`。

**文件位置：** `$XDG_STATE_HOME/aurora-music/logs/aurora.log.<date>`，保留 7 天。

### 7.2 埋点规范

```rust
// 用 span 表达一次操作的生命周期
let span = tracing::info_span!("scan.root", root_id, path_hash = %hash_path(&path));
let _g = span.enter();
tracing::info!(files_seen, files_added, elapsed_ms, "scan finished");

// 曲目相关操作用 track_id 作为结构化字段（非字符串拼接）
tracing::debug!(track_id, codec, "probing track");
```

**必须有日志的关键事件：**

| 事件 | 级别 |
|---|---|
| 应用启动 / 关闭（含版本、数据库 schema 版本） | info |
| 扫描开始 / 结束（含统计） | info |
| 播放开始 / 结束 | debug |
| 解码失败 / 设备丢失 | warn |
| 来源请求失败 | warn |
| 流地址校验拒绝 | warn（含拒绝原因，不含完整 URL） |
| 迁移执行 | info |
| 未捕获的前端异常 | error |

### 7.3 脱敏规则（NFR-4.5、FR-10.4）

在线媒体地址常带签名查询串（`?token=...&sig=...`）。日志中：

```rust
fn redact_url(u: &Url) -> String {
    // 保留 scheme + host + path 前缀，丢弃全部 query 与 fragment
    format!("{}://{}{}", u.scheme(), u.host_str().unwrap_or("?"), u.path())
}
```

路径同理：日志中的文件路径只保留哈希或最后一段，便于关联而不过度暴露。

### 7.4 前端日志汇聚

```ts
// ipc/client.ts 中统一包装
window.addEventListener('error', (e) => invoke('diagnostics_log_from_frontend', {...}));
window.addEventListener('unhandledrejection', (e) => invoke('diagnostics_log_from_frontend', {...}));
```

`diagnostics_recent_errors` 从内存环形缓冲区（容量 200）读取，供 UI 诊断面板（FR-10.5）展示。

---

## 8. 前端结构

```
src/
├── main.tsx
├── App.tsx                  # 布局骨架
├── ipc/                     # ⚠️ 唯一调用 invoke() 的地方
│   ├── client.ts            # 封装 invoke + 错误归一化
│   ├── events.ts            # 事件订阅
│   └── types.ts             # 与 Rust DTO 对齐的类型
├── state/
│   ├── player.ts            # 播放状态 + 位置插值
│   ├── library.ts
│   ├── queue.ts
│   └── ui.ts
├── views/
│   ├── Library/  Album/  Artist/  Playlist/
│   ├── Search/   NowPlaying/  Settings/
├── components/
│   ├── Sidebar/  PlayerBar/  QueuePanel/
│   ├── Artwork/  Lyrics/     VirtualList/
└── styles/
    └── theme.css            # 深色主题 token
```

### 8.1 状态管理边界

| 状态 | 归属 | 理由 |
|---|---|---|
| 播放状态、位置、音量 | **Rust 为真相源**，前端为镜像 | 多入口（UI / 媒体键 / MPRIS）需一致 |
| 队列内容与顺序 | **Rust 为真相源** | 同上 |
| 资料库数据 | Rust 为真相源，前端缓存 | 数据量大 |
| 当前视图、滚动位置、面板开关 | **前端持有** | 纯 UI，无需持久化到 DB |
| 搜索输入框内容 | 前端持有（防抖后发 IPC） | 高频变更 |

### 8.2 位置插值（NFR-1.6）

```ts
// 收到 playback:position 时记录锚点
let anchor = { posMs, durationMs, at: performance.now() };

// requestAnimationFrame 中渲染
function render() {
  const elapsed = performance.now() - anchor.at;
  const shown = Math.min(anchor.posMs + elapsed, anchor.durationMs);
  progressBar.style.transform = `scaleX(${shown / anchor.durationMs})`;
  requestAnimationFrame(render);
}
```

### 8.3 深色主题

- 使用 CSS 自定义属性定义 token（颜色、间距、圆角、层级），集中在 `theme.css`
- 遵循 KDE 的 `prefers-color-scheme`；M6 可读取 Plasma 配色方案
- 大列表使用虚拟滚动，配合固定尺寸占位避免布局抖动

---

## 9. 技术选型与版本矩阵

> 全部版本于 **2026-10-15** 通过 `index.crates.io` 与 `registry.npmjs.org` 实测确认，非记忆值。

### 9.1 Rust 依赖

| 用途 | crate | 采用版本 | MSRV | 备注 |
|---|---|---|---|---|
| 应用框架 | `tauri` | **2.12.4** | — | ⚠️ 禁止 `3.0.0-alpha.5` |
| 构建脚本 | `tauri-build` | **2.7.3** | — | ⚠️ 同上 |
| 音频输出+解码 | `rodio` | **0.23.0** | 1.87 | 特性 `symphonia-all` |
| 音频设备 | `cpal` | 0.18.2 | 1.85 | 由 rodio 传递引入 |
| 标签元数据 | `lofty` | **0.25.4** | — | 独立于解码链 |
| 数据库 | `rusqlite` | **0.40.2** | — | 特性 `bundled` |
| 连接池 | `r2d2` / `r2d2_sqlite` | 0.8.10 / 0.35.0 | — | |
| 异步运行时 | `tokio` | **1.53.3** | — | 特性 `rt-multi-thread`, `sync`, `time`, `fs` |
| 取消令牌 | `tokio-util` | 0.7.20 | — | `CancellationToken` |
| 目录遍历 | `walkdir` | **2.5.0** | — | |
| 文件监听 | `notify` | **8.2.0** | — | ⚠️ 不要用 `9.0.0-rc.5` |
| HTTP 客户端 | `reqwest` | **0.13.5** | — | 特性 `rustls-tls`, `json`, `stream` |
| 图片处理 | `image` | **0.25.10** | — | WebP 编码 |
| 错误 | `thiserror` | **2.0.21** | — | `anyhow` 仅用于 `main.rs` |
| 日志 | `tracing` / `tracing-subscriber` / `tracing-appender` | 0.1.44 / 0.3.23 / 0.2.5 | — | |
| 序列化 | `serde` / `serde_json` | 1.0.229 / 1.0.154 | — | |
| 哈希 | `blake3` | **1.8.7** | — | 封面内容寻址 |
| 异步 trait | `async-trait` | 0.1.92 | — | |
| URL 解析 | `url` | **2.5.8** | — | 安全校验的基础 |
| 编码检测 | `chardetng` / `encoding_rs` | 1.0.0 / 0.8.42 | — | LRC 编码 |
| 路径 | `camino` | 1.2.6 | — | UTF-8 路径 |
| 目录解析 | `directories` | **6.0.0** | — | XDG |
| 锁 | `parking_lot` | 0.12.5 | — | |
| 缓存 | `moka` | 0.12.16 | — | 内存 LRU |
| MPRIS | `mpris-server` | **0.10.0** | — | KDE 集成 |
| 类型导出 | `specta` / `tauri-specta` | 1.0.5 / 1.0.2 | — | ⚠️ 2.0 为 rc，见 Q-5 |

**⚠️ 依赖陷阱（必须显式钉版本）：**

```toml
# crates.io 上这些 crate 的"最新版本"是 3.0.0-alpha 预发布版。
# 直接 cargo add tauri-plugin-sql 会拉入 alpha，导致 API 不兼容。
tauri-plugin-dialog = "=2.8.3"
tauri-plugin-fs     = "=2.7.1"
tauri-plugin-opener = "=2.7.2"
tauri-plugin-log    = "=2.10.1"
```

**⚠️ symphonia 版本分裂问题：**

| 事实 | 后果 |
|---|---|
| `rodio 0.23.0` 内部依赖 `symphonia ^0.5.5` | 传递引入 symphonia **0.5.x** |
| 独立 crate `symphonia` 最新为 **0.6.1** | 二者 semver 不兼容 |
| 若同时直接依赖 `symphonia 0.6` | 依赖树中出现**两份 symphonia**，二进制膨胀、行为不一致 |

**决策：不直接依赖 `symphonia`。** 解码能力通过 `rodio` 的 `symphonia-all` 特性获得；元数据交给 `lofty`。这样依赖树中只有一份 symphonia。

> 若将来确需 symphonia 原生 API（如精细探测），必须先确认 rodio 已升级到 0.6，或接受双版本代价——届时重新评估。

**`rodio` 特性配置（✅ 已实测验证）：**

```toml
rodio = { version = "0.23.0", default-features = false, features = [
    "playback",        # ⚠️ 必需：输出设备与 Player 全部由它门控
    "symphonia-all",   # MP3/FLAC/AAC/OGG/Opus/WAV 等全格式解码
    "dither",          # 音量变化时的抖动，减少量化失真
] }
```

> ⚠️ **实测踩坑记录：** `playback = ["dep:cpal"]` 是独立 feature，`symphonia-all`
> **只带解码器不带输出**。漏掉 `playback` 会让 `DeviceSinkBuilder`、`MixerDeviceSink`、
> `Player`、`PlayError` 全部消失，编译报 `unresolved import`。
> 该结论来自本机实际编译，已由 `Cargo.toml` 固化。

### 9.2 前端依赖

| 包 | 项目声明 | 最新稳定 | 决策 |
|---|---|---|---|
| `react` / `react-dom` | `^19.1.0` | 19.3.1 | 保持 |
| `@tauri-apps/api` | `^2` | 2.12.4 | 保持 |
| `@tauri-apps/cli` | `^2` | 2.12.3 | 保持 |
| `vite` | `^8.0.16` | 8.3.4 | 保持 |
| `typescript` | `~6.0.3` | **7.0.2** | ⚠️ **保持 6.x**。TS 7 是大版本跃迁，不在 MVP 引入风险 |
| `@vitejs/plugin-react` | `^6.0.2` | 6.1.2 | 保持 |

**需要新增（M1–M3）：**

| 包 | 用途 | 里程碑 |
|---|---|---|
| `@tauri-apps/plugin-dialog` | 选择音乐目录 | M1 |
| `zustand` | 轻量状态容器 | M1 |
| `@tanstack/react-virtual` | 大列表虚拟滚动 | M1 |
| `react-router-dom` | 视图路由（或自建轻量路由） | M1 |
| `vitest` + `@testing-library/react` | 前端测试 | M1 |
| `eslint` + `@typescript-eslint` | 代码规范 | M0 |

### 9.3 系统依赖

**已实测满足**（无需任何安装操作）：

`gtk3 3.24.52` · `webkit2gtk-4.1 2.54.1` · `libsoup3 3.8.0` · `glib2 2.90.1` · `librsvg 2.63.2` · `libxkbcommon 1.13.2` · `alsa-lib 1.2.16.1`（含头文件）· `pipewire 1.6.9` + `pipewire-audio`（ALSA 桥接插件与配置齐备）· `gcc 16.2.1` · `make` · `pkgconf` · `patchelf` · `fakeroot`

**结论：Tauri 2 构建所需的系统依赖 100% 满足，无需 `pacman -S` 任何东西。**

> 注：`gst-plugins-*` 解码器缺失**与本项目无关**——本设计使用 rodio+Symphonia 在 Rust 侧解码，不经过 WebKitGTK 的 GStreamer 管线。这正是选择该方案而非 HTML5 `<audio>` 的收益之一。

---

## 10. 风险与未决问题

### 10.1 风险

| 编号 | 风险 | 影响 | 缓解 |
|---|---|---|---|
| RK-1 | `rodio` 的曲目结束检测不精确 | 续播有毫秒级间隙 | M1 用 `EndingDetector` 包装验证；gapless 推迟到 M4 |
| RK-2 | FTS5 对 CJK 分词不佳 | 中文搜索质量差 | M3 实测；备选 bigram 索引或 `LIKE` 补充路径 |
| RK-3 | rusqlite 阻塞异步运行时 | UI 卡顿（违反 NFR-1.7） | 强制 `spawn_blocking`；CI 中加入审查项 |
| RK-4 | 自定义 URI 协议在各平台 URL 形态不同 | 封面加载失败 | 实现时实测确认，不假定格式 |
| RK-5 | 依赖树中混入 Tauri 3 alpha | 编译失败 / API 断裂 | 全部 tauri 相关依赖显式钉 `=2.x.y` |
| RK-6 | NVIDIA + WebKitGTK DMABUF 渲染问题 | 界面白屏 | 已知问题；如复现，记录 `WEBKIT_DISABLE_DMABUF_RENDERER` 为已知规避手段，不作为默认配置 |
| RK-7 | 在线来源的合规性与接口稳定性 | 法律风险 / 适配器失效 | 每个适配器须附合规依据；接口变更时优雅降级而非崩溃 |
| RK-8 | 10 万曲目扫描的元数据探测耗时 | 首次体验差 | 有界并行 + 增量扫描；M1 末用 1 万文件实测 |
| RK-9 | 路径含空格（当前项目在 `Aurora Music/` 下） | 部分 C 构建脚本异常 | 建议迁移到无空格路径（见下） |

> **RK-9 补充：** 当前项目位于 `/home/arch/Downloads/Aurora Music/aurora-music`，父目录含空格。Rust/npm 本身可处理，但为避免后续引入带 C 构建脚本的 crate 时踩坑，建议迁至 `~/dev/aurora-music`。**这需要你确认后再执行**，我不擅自移动。

### 10.2 实施期实测发现（2026-10-15）

以下结论来自实际编码与测试，**修正了设计阶段的若干假设**：

| # | 发现 | 影响 |
|---|---|---|
| F-1 | rodio 0.23 的 `playback` feature 必须显式启用，`symphonia-all` 只带解码器 | 已在 `Cargo.toml` 固化 |
| F-2 | rodio 0.23 用 `DeviceSinkBuilder`/`MixerDeviceSink`/`Player` 取代了 `OutputStream`/`Sink` | 见 §2.3 修正 |
| F-3 | `Player::get_pos()` 直接可用，无需自建采样计数式位置追踪 | 简化了引擎设计 |
| F-4 | **扫描阶段（lofty）比播放阶段（rodio）对损坏文件更严格** | 见下表 |
| F-5 | lofty 的 `Accessor` 用 `disk()` 而非 `disc()`；年份在 `date()` 返回的 `Timestamp.year` 里 | 已在 `probe.rs` 固化 |
| F-6 | lofty 的 `FileParseError` 只暴露 `is_unknown_format()`，无法区分 I/O 与解析失败 | 改为显式开文件区分 |
| F-7 | **Tauri 2 提供 `tauri::test::MockRuntime`**，可对命令层做真实 IPC 测试 | 见 `testing.md` |
| F-8 | Tauri 2 命令参数默认转 camelCase（`ArgumentCase::Camel`） | 前端传 `rootId` 对应 Rust `root_id` |

**F-4 详述 —— 损坏文件的两层行为差异：**

| 文件类型 | 扫描阶段（lofty，只读头部） | 播放阶段（rodio，需解码音频帧） |
|---|---|---|
| 纯文本改名 `.mp3` | ❌ 拒绝 | ❌ 拒绝 |
| 截断 50% 的 FLAC | ❌ **拒绝** | ⚠️ 接受，但在迭代中**静默提前结束** |

**为什么播放阶段必须额外防御：** `impl Iterator for Decoder` 的 `Item = Sample`
（不是 `Result<Sample, _>`），因此**解码错误不会以 Result 形式暴露**，
迭代器只是安静地返回 `None`。

**因此播放引擎必须实施：** 曲目结束时比对「实际播出位置」与「声明时长」，
若显著短于声明值（例如 < 90%），判定为损坏并上报 `PLAYBACK_DECODE_FAILED`
后跳到下一首（FR-2.6）。扫描阶段的严格校验降低了此类情况的发生概率，
但**不能替代**这一运行时检查——损坏发生在音频帧中间而非文件尾部时，
头部仍然是合法的。

### 10.3 播放引擎实施发现（2026-10-15）

| # | 发现 | 影响 |
|---|---|---|
| F-9 | `Player` 与 `MixerDeviceSink` 在 Linux/ALSA 上**均为 `Send + Sync`** | 不需要"专用音频线程 + 命令通道"的复杂设计，引擎可直接放进 Tauri 托管状态。已在 `src-tauri/tests/` 与运行期线程表验证 |
| F-10 | `Player::get_pos()` 精确且结束后保持终值；`append()` 会重置位置计数 | 位置上报与"曲目结束"判定都可靠；无需自建采样计数 |
| F-11 | 损坏文件有**两种**模式：`Decoder::new()` 拒绝 vs 接受后提前结束 | 引擎必须同时处理加载期失败与运行期截断（见 F-4） |
| F-12 | **曲目自然结束后 `state()` 仍返回 `Playing`** | 真实缺陷，由 `tests/real_playback.rs` 在真实声卡上发现。根因：结束只置 `end_reported` 而未清 `current`。修复方式：`state()` 与 `take_events()` 共用幂等的 `refresh()`，使状态判定与调用顺序无关 |
| F-13 | `Player::empty()` 在 `append()` 之后、真正出声之前会短暂为 `true` | 直接用它判定"播完"会误报。必须用「曾观察到非空 **或** 位置 > 0」作为前置条件 |

**F-12 的教训：** 无声替身的测试全绿，因为服务层的自动续播立刻覆盖了这个状态窗口。**只有真实硬件测试才暴露了它。** 这印证了 `testing.md` 中"关键路径必须有真实验证"这一条的必要性。

**F-13 的教训：** rodio 的 `empty()` 语义比名字暗示的宽泛。所有基于它的判定都必须先排除"尚未开始"的状态。

### 10.4 未决问题

| 编号 | 问题 | 需要谁决定 | 时限 |
|---|---|---|---|
| Q-1 | 是否采用 `specta`/`tauri-specta` 自动生成 TS 类型，还是手写并靠测试保证一致 | 你 | M0 开始前 |
| Q-2 | 首个在线来源适配器选哪个 | 你 | M4 结束前 |
| Q-3 | 是否在 M0 迁出含空格路径 | 你 | M0 开始前 |
| Q-4 | 是否引入 `notify` 文件监听（M3 还是延后） | 你 | M3 |
| Q-5 | 封面自定义 URI 协议 vs 直接暴露缓存目录给 `asset:` 协议 | 实现时实测 | M3 |

---

## 11. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| 0.1.0 | 2026-10-15 | 初稿；全部依赖版本经 crates.io / npm 实测确认 |
