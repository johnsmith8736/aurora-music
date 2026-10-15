# 接口设计

> 状态：设计阶段 · 最后更新：2026-10-15
> 证据标记约定见 [`README.md`](README.md) §0。
> 类型定义：[`src/services/music-sources/types.ts`](../../src/services/music-sources/types.ts)

---

## 1. 设计前提

### 1.1 实现位置：Rust，不是 TypeScript

设计稿要求产出 TypeScript 的 `MusicSource` / `MusicSourceRegistry` 接口。
**现有架构不允许在 TS 侧实现它们。**

| 约束 | 原文 | 出处 |
|---|---|---|
| 前端不得直接发起任意网络请求 | NFR-4.2 | `docs/requirements.md` |
| 前端访问后端的唯一入口 | "全项目只有本文件允许出现 `invoke()`" | `src/ipc/client.ts:2` |

**【项目内已验证】** 已检查：`invoke(` 仅出现在 `src/ipc/client.ts`，
边界完好。若在 TS 侧实现带 HTTP 的来源适配器，将同时破坏这两条约束。

**处置（记录，不擅自重构）：**

| 层 | 内容 |
|---|---|
| **Rust `domain/ports.rs`** | 真实接口定义（trait），实现位于 `infra/sources/` |
| **TS `src/services/music-sources/types.ts`** | 契约镜像 + 前端投影类型 + 类型级安全约束 |
| **TS 不实现** | 任何发起网络请求的适配器 |

这样既满足"产出 TS 接口定义"的交付要求，又不违反架构约束。

### 1.2 与现有 `ResolvedStream` 设计的关系

`docs/architecture.md` §2.10 已经设计了 **P4 的类型级强制**：
`ResolvedStream` 字段私有 + 唯一校验构造器，`AudioOutput::load()` 只接受该类型。

**本设计沿用同一思路，不引入第二套机制。** TS 侧的
`ResolvedPlayback` 用 `unique symbol` 品牌复现同样的约束，
使前端也无法凭空构造播放凭证。

| 概念 | Rust | TS 投影 |
|---|---|---|
| 唯一校验入口 | `ResolvedStream::validate()` | `createResolvedPlayback()` |
| 不可凭空构造 | 字段私有 | `unique symbol` 品牌 |
| 引擎只接受该类型 | `load(stream: ResolvedStream)` | 见 §5 |

---

## 2. 统一领域模型

### 2.1 分层

```
Track（元数据）          ← 只描述"这是什么"
    │
    │  resolvePlayback()  ← 唯一能跨越这条边界的操作
    ▼
ResolvedPlayback（凭证）  ← 携带 PlaybackCapability
    │
    ▼
播放引擎
```

**核心不变量：`Track` 没有 `capability` 字段。**
因此把 `Track` 直接交给播放器在类型上不成立 —— 这不是靠约定，是靠类型系统。

### 2.2 为什么不合并本地与远程的 Track

现有 `src/ipc/types.ts` 的 `Track` 与新的来源 `Track` **刻意不合并**：

| | 本地库 `Track`（现有） | 来源 `Track`（新增） |
|---|---|---|
| 标识 | `id: number`（数据库主键） | `sourceId` + `sourceTrackId` |
| 路径 | `path: string`（必有） | **无路径概念** |
| 文件属性 | `fileSize` / `fileMtime` / `codec` | 无 |
| 可播性 | 天然可播 | **不天然可播** |

合并会破坏 §2.1 的安全属性：本地曲目的"天然可播"与远程曲目的
"需要授权"会被混为一谈。

**【推断】** 未来若需要统一的 UI 展示模型，应在**视图层**做投影，
而不是合并领域类型。

---

## 3. 来源接口（Rust 侧，权威定义）

> 本节是设计的**权威来源**。TS 文件是它的镜像。

### 3.1 端口 trait

```rust
// domain/ports.rs（新增，本阶段不创建）

#[async_trait]
pub trait MusicSource: Send + Sync {
    fn id(&self) -> SourceId;
    fn display_name(&self) -> &str;
    fn capabilities(&self) -> SourceCapabilities;

    /// 搜索曲目元数据。
    ///
    /// **搜索结果是元数据，不是播放授权。** 实现返回的 `SearchHit`
    /// 可携带 `PlaybackCapability::Unsupported`。
    async fn search(
        &self,
        query: &SearchQuery,
        cancel: &CancellationToken,
    ) -> Result<SearchResult, MusicSourceError>;

    /// 解析播放能力 —— **唯一**能产出 `ResolvedStream` 的入口。
    ///
    /// 实现职责：
    /// 1. 取得候选 URL
    /// 2. 调用 `ResolvedStream::validate()` 完成全部校验
    /// 3. 重定向时逐跳重新校验
    /// 4. 不可播放时返回 `Ok(Unsupported)`，**不是** `Err`
    async fn resolve_playback(
        &self,
        track: &SourceTrackId,
        cancel: &CancellationToken,
    ) -> Result<ResolvedStream, MusicSourceError>;

    fn auth_status(&self) -> SourceAuthStatus;
    async fn authenticate(&self, cancel: &CancellationToken)
        -> Result<SourceAuthStatus, MusicSourceError>;
    async fn sign_out(&self) -> Result<(), MusicSourceError>;
}
```

### 3.2 能力是四个独立维度

| 能力 | 方法 | 可否缺失 |
|---|---|---|
| 搜索 | `search()` | ✅ 可以（只做元数据的目录服务） |
| 播放解析 | `resolve_playback()` | ✅ 可以（返回 `Unsupported`） |
| 认证 | `auth_status()` / `authenticate()` | ✅ 可以（公开来源不需认证） |
| 播放权限 | 由 `ResolvedStream` 表达 | — 可能是 `Unsupported` |

**来源可以支持搜索但不支持播放。** 接口不强制两者同时具备 ——
这是"不要把所有来源的播放能力强行统一为直接 URL"的结构性体现。

### 3.3 取消与超时

```rust
pub struct SearchQuery {
    pub text: String,
    pub limit: usize,
    pub offset: Option<usize>,
}
```

**取消通过 `CancellationToken` 参数传递，不放在 `SearchQuery` 里。**
理由：取消是控制流，不是查询的一部分；且 `CancellationToken` 需要
跨来源共享（禁用来源时立即取消其全部在途请求）。

超时由调用方（`SourceRegistry`）统一施加，适配器不自行设置 ——
避免每个适配器各定一套超时策略。见 `security-and-errors.md` §4。

---

## 4. 播放能力：判别联合

### 4.1 为什么不是"一个有 url 字段的结构体"

```rust
// ❌ 被否决的设计
pub struct PlaybackSource {
    pub url: String,          // 本地文件怎么办？
    pub expires_at: Option<..>,  // 本地文件没有过期概念
}
```

问题：
1. 强制所有来源都有 URL —— 但某些平台只能在自家客户端播放
2. 本地文件与远程流被迫共用一套字段，语义含糊
3. "不可播放"无处表达，只能靠 `url` 为空字符串这种脆弱约定

### 4.2 采用的设计

```rust
pub enum PlaybackCapability {
    /// 本地文件。路径来自资料库，已规范化并校验存在性。
    LocalFile { path: PathBuf },

    /// 来源解析出的直接音频流。
    DirectUrl {
        url: Url,
        mime_type: Option<String>,
        /// `None` 表示来源未声明过期 —— **不代表永久有效**。
        expires_at: Option<SystemTime>,
        content_length: Option<u64>,
    },

    /// 移交外部播放器。保留此分支以避免"必须有 URL"的错误假设。
    ExternalPlayer { uri: String, provider: String },

    /// 有元数据但不可播放。**一等公民，不是错误。**
    Unsupported { reason: PlaybackUnavailableReason, message: String },
}
```

**`Unsupported` 是 `Ok` 而非 `Err`：**
"这首歌存在但你的账号不能播"是**正常业务状态**，不是异常。
把它做成错误会导致 UI 到处处理异常分支。

### 4.3 过期时间的三态

| 值 | 含义 | 实现方义务 |
|---|---|---|
| `Some(t)` | 来源声明 `t` 后失效 | 使用前检查 |
| `None` | 来源**未声明**过期 | **不得**当作永久有效；长时间播放或恢复播放前应重新解析 |

**【推断】** 多数流媒体服务返回带签名的临时链接，有效期从数分钟到数小时不等。
具体策略取决于来源，**不能统一假设**。

**持久化规则：** `DirectUrl` 分支的 URL **不得**默认写入数据库。
队列持久化时只存 `sourceId` + `sourceTrackId`，播放前重新解析。
理由：签名 URL 既是凭据也是时效性内容，落库会产生"重启后播放失败"
且难以诊断的故障。

---

## 5. 类型级安全：两道防线

### 5.1 防线一：元数据无法充当凭证

```
Track          { sourceId, sourceTrackId, title, ... }   ← 无 capability
ResolvedPlayback { ..., capability: PlaybackCapability }  ← 有 capability
```

TS 的结构化类型系统保证：把 `Track` 传给需要 `ResolvedPlayback` 的位置会编译失败。

### 5.2 防线二：凭证无法凭空构造

```ts
declare const PlaybackGrantBrand: unique symbol;

export interface ResolvedPlayback {
  readonly [PlaybackGrantBrand]: true;
  // ...
}
```

**`unique symbol` 品牌使对象字面量无法满足该类型** —— 只有
`createResolvedPlayback()`（或显式断言）能产出。

Rust 侧对应的是 `ResolvedStream` 的**私有字段**：不经过
`validate()` 就没有值可传。

> **【推断】** 两处约束同构，但强度不同：Rust 的私有字段是真正的封装，
> TS 的品牌是编译期约定（运行时可通过 `as` 绕过）。
> 因此**权威校验在 Rust 侧**，TS 品牌只是让前端尽早发现自己写错了。

### 5.3 播放引擎的签名

```rust
// infra/audio/rodio_engine.rs 的目标形态（本阶段不修改）
fn load_and_play(&self, stream: ResolvedStream) -> Result<(), PlaybackError>;
```

`ResolvedStream` 取代现有的 `AudioSource { path: PathBuf }`（见 §7）。

---

## 6. 注册表与生命周期

### 6.1 不变量

实现必须保证以下四条，测试见 `testing-plan.md` §5：

| # | 不变量 | 对应需求 |
|---|---|---|
| I-1 | `"local"` 来源**始终存在且不可移除**，`enabled` 恒为 `true` | — |
| I-2 | 所有在线来源**默认 `enabled === false`** | FR-9.2 |
| I-3 | 禁用/移除/注销任何在线来源，**不影响本地来源能力** | FR-9.2 推论 |
| I-4 | 未通过核实的来源以 `placeholder: true` 注册，**不返回伪造结果** | 本模块安全要求 |

### 6.2 生命周期

```
注册（进程启动）
  └─ 全部在线来源 enabled = false
        │
用户显式启用 ──► setEnabled(id, true)
        │              └─ 检查认证状态
        │                    ├─ 未配置 → 引导配置
        │                    └─ 已配置 → 标记 ready
        │
        ├─► search() ──► 适配器 ──► 结果（可能含 Unsupported）
        │
        ├─► resolvePlayback() ──► ResolvedStream::validate()
        │                              └─ 失败 → StreamRejected
        │
禁用 ──► setEnabled(id, false)
        └─ 立即取消该来源全部在途请求（CancellationToken）
        └─ 队列中的该来源曲目标记为不可播放，但不从队列移除
```

**禁用时队列中的曲目怎么办？**
**【推断】** 不应静默移除（用户会以为播放器丢了东西）。
应标记为不可播放，播放到该曲时按 FR-2.6 跳过并给出提示。

### 6.3 跨来源搜索的失败隔离

```rust
pub struct MultiSourceSearchResult {
    pub results: Vec<SearchResult>,
    /// 失败的来源。**空数组表示全部成功。**
    pub failures: Vec<MusicSourceError>,
    /// 因被禁用而跳过的来源。
    pub skipped: Vec<SourceId>,
}
```

**单个来源失败不得使整体失败。** 这也是 I-3 的延伸：
一个在线来源挂了，用户的本地音乐搜索必须照常工作。

---

## 7. 与现有播放链路的集成点

### 7.1 当前状态

**【项目内已验证】**

| 类型 | 位置 | 当前形状 |
|---|---|---|
| `AudioSource` | `domain/ports.rs:22` | `{ track_id: TrackId, path: PathBuf, declared_duration: Option<Duration> }` |
| `QueueEntry` | `domain/queue.rs:21` | `{ track_id, title, artist, album, path: PathBuf, duration_ms }` |

**两者都用 `PathBuf` 表达音源 → 只能表示本地文件。**

### 7.2 扩展方案（**本阶段不实施**）

| 步骤 | 内容 | 影响面 |
|---|---|---|
| E-1 | `AudioSource` 的 `path` 改为 `ResolvedStream` | `domain/ports.rs`、两个引擎实现、`playback_service.rs` |
| E-2 | `QueueEntry` 增加来源字段，`path` 改为可选 | `domain/queue.rs`、`library_service::load_queue_entries`、前端 `QueueEntry` |
| E-3 | `track_id: i64` 改为可表达来源的复合标识 | 贯穿队列、历史、播放服务 |
| E-4 | 数据库 schema 迁移：`tracks.source_id` / `external_id` | **v1 schema 已预留这两列**（`infra/db/schema.rs`），需确认约束 |

> ⚠️ **E-1 ~ E-3 是对现有文件的修改，属本阶段明确禁止的范围。**
> 此处只记录方案，不实施。

### 7.3 兼容性自评

| 现有组件 | 是否需要改动 | 说明 |
|---|---|---|
| `domain/model.rs` 的 `Track` | ⚠️ 可能需要 | 已有 `source_id` 概念（schema 中），但 Rust 结构体未暴露 |
| `infra/scan/*` | ❌ 不需要 | 本地扫描与来源无关 |
| `infra/db/*` | ⚠️ 需新增迁移 | 创建 `source_configs` 表；schema v1 中已设计但未创建 |
| `infra/audio/*` | ⚠️ 需要（E-1） | 引擎签名从 `AudioSource` 改为 `ResolvedStream` |
| `app/playback_service.rs` | ⚠️ 需要（E-1/E-2） | 队列与加载路径 |
| `commands/*` | ➕ 新增 | 来源相关命令 |
| 前端 `ipc/*` | ➕ 新增 | 来源相关 API 与类型 |
| 前端 UI | ➕ 新增 | 来源设置页、搜索来源筛选 |

**结论：** 现有本地功能**不需要**改动的部分占多数（扫描、数据库主体、
音频引擎内部实现），但**播放链路的类型签名需要扩展**。
这是不可避免的——本地文件与远程流终究要被同一个引擎播放。

---

## 8. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| 0.1.0 | 2026-10-15 | 初稿；记录 C-1 ~ C-4 四处架构冲突，未擅自重构 |
