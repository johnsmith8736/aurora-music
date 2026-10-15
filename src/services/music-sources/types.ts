/**
 * 在线音乐来源模块 —— 接口契约与领域类型。
 *
 * # 这个文件是什么
 *
 * 它是**设计契约**，不是运行时实现。定义来源适配器必须满足的形状，
 * 以及前端能观察到的投影类型。
 *
 * # ⚠️ 为什么这里没有 HTTP 实现
 *
 * 现有架构有两条硬约束，使 TS 侧的适配器实现**不被允许**：
 *
 * | 约束 | 出处 |
 * |---|---|
 * | 前端不得直接发起任意网络请求（须经 Rust 侧白名单） | `docs/requirements.md` NFR-4.2 |
 * | 在线来源不得把未验证 URL 直接传入音频引擎 | `docs/architecture.md` P4 |
 *
 * 因此真正的 `MusicSource` **实现位于 Rust 侧**（`infra/sources/`），
 * 前端只能通过 IPC 观察来源状态。本文件承担三件事：
 *
 * 1. 记录契约（Rust trait 的 TS 镜像，供对照评审）
 * 2. 定义前端可观察的投影类型
 * 3. 用类型约束表达「元数据 ≠ 播放授权」这一安全属性
 *
 * 详细论证见 `docs/music-sources/interface-design.md`。
 *
 * # 与其他类型文件的关系
 *
 * - `src/ipc/types.ts` 是**本地资料库**的 IPC 投影（含 `path`/`fileSize` 等本地字段）
 * - 本文件是**来源无关**的元数据与播放能力模型
 *
 * 两者刻意不合并：本地曲目天然可播，远程曲目不天然可播，
 * 混为一谈会破坏下文的类型安全设计。
 *
 * @packageDocumentation
 */

// ══════════════════════════════════════════════════════════════
// 标识
// ══════════════════════════════════════════════════════════════

/**
 * 来源标识。内置的本地来源固定为 `"local"`。
 *
 * 形如 `"navidrome:550e8400"`、`"gdstudio:default"`。
 * 用字符串而非数字，便于日志阅读与调试。
 */
export type SourceId = string;

/** 来源内部的曲目标识。**对本地来源**即为资料库主键的字符串形式。 */
export type SourceTrackId = string;

/** 别名，仅供阅读时区分「资料库主键」与「来源内标识」。 */
export type LocalTrackId = number;

// ══════════════════════════════════════════════════════════════
// 元数据
// ══════════════════════════════════════════════════════════════

/**
 * 曲目元数据 —— **只描述"这是什么"，不表达"能不能播"**。
 *
 * 这是本模块最重要的类型约束。任何未经过 `MusicSource.resolvePlayback()`
 * 的 `Track` 都**不能**交给播放器：它没有 `PlaybackCapability` 字段，
 * 因此在类型上无法满足播放接口的要求。
 */
export interface Track {
  /** 来源标识。 */
  readonly sourceId: SourceId;
  /** 来源内的曲目标识。对本地来源为资料库主键的字符串形式。 */
  readonly sourceTrackId: SourceTrackId;

  readonly title: string;
  readonly artist: string | null;
  readonly album: string | null;
  readonly albumArtist: string | null;
  readonly trackNo: number | null;
  readonly discNo: number | null;
  readonly year: number | null;
  /** 时长（毫秒）。未知为 null，**不得**用 0 冒充未知。 */
  readonly durationMs: number | null;

  /** 封面引用。如何获取取决于来源能力，见 `SourceCapabilities.artwork`。 */
  readonly artworkRef: ArtworkRef | null;
}

/** 封面引用。不同来源获取封面的方式不同，因此用判别联合而非裸 URL。 */
export type ArtworkRef =
  | { readonly kind: "embedded" }
  | { readonly kind: "source-id"; readonly artworkId: string }
  | { readonly kind: "url"; readonly url: string; readonly expiresAt: number | null }
  | { readonly kind: "none" };

// ══════════════════════════════════════════════════════════════
// 播放能力（安全核心）
// ══════════════════════════════════════════════════════════════

/**
 * 播放能力 —— **与元数据严格分离**。
 *
 * 设计要点：
 *
 * 1. 不把所有来源强行统一为「直接 URL」。外部播放器移交、平台专用播放
 *    机制、以及"有元数据但不可播"都是合法形态。
 * 2. `direct-url` 分支携带 `expiresAt`：临时链接**默认不持久化**。
 * 3. `unsupported` 是一等公民 —— 搜索能返回它，播放不能消费它。
 */
export type PlaybackCapability =
  /** 本地文件。路径来自资料库，已规范化且经过存在性校验。 */
  | {
      readonly kind: "local-file";
      /** 绝对路径。 */
      readonly path: string;
    }
  /** 由来源解析出的直接音频流地址。 */
  | {
      readonly kind: "direct-url";
      readonly url: string;
      readonly mimeType: string | null;
      /**
       * 链接过期时间（Unix 毫秒）。`null` 表示来源未声明过期，
       * **不代表永久有效** —— 实现方仍须在使用前重新解析。
       */
      readonly expiresAt: number | null;
      /** 声明的内容长度（字节）。用于体积上限校验。 */
      readonly contentLength: number | null;
    }
  /**
   * 移交外部播放器。
   *
   * 保留此分支是为了避免"所有来源都必须能给出 URL"这一错误假设 ——
   * 某些平台只允许在自家客户端内播放。
   */
  | {
      readonly kind: "external-player";
      /** 交给外部播放器的 URI（可能是平台私有 scheme）。 */
      readonly uri: string;
      readonly provider: string;
    }
  /**
   * 有元数据但**不可播放**。
   *
   * 搜索结果为它时，UI 必须显示为不可播放而非报错。
   */
  | {
      readonly kind: "unsupported";
      readonly reason: PlaybackUnavailableReason;
      /** 面向用户的说明，已脱敏。 */
      readonly message: string;
    };

export type PlaybackUnavailableReason =
  | "not-entitled" // 账号无权播放该曲目
  | "region-restricted" // 地区限制
  | "source-disabled" // 来源被用户禁用
  | "not-authenticated" // 需要登录
  | "source-unsupported" // 该来源不提供播放解析
  | "unknown";

// ── 播放授权凭证 ─────────────────────────────────────────────

declare const PlaybackGrantBrand: unique symbol;

/**
 * 已解析的播放凭证 —— **唯一**可交给播放引擎的值。
 *
 * `PlaybackGrantBrand` 是 `unique symbol` 品牌：外部代码**无法**凭空
 * 构造出满足该类型的对象字面量，只有 `MusicSource.resolvePlayback()`
 * 的实现（或其显式断言）能产出。这样"忘记解析就播放"在类型层面
 * 就不可能发生 —— 与 Rust 侧 `ResolvedStream` 私有字段是同一思路。
 */
export interface ResolvedPlayback {
  readonly [PlaybackGrantBrand]: true;

  readonly sourceId: SourceId;
  readonly sourceTrackId: SourceTrackId;
  readonly capability: PlaybackCapability;
  /** 解析时刻（Unix 毫秒）。用于判断凭证是否已过期。 */
  readonly resolvedAt: number;
  /** 元数据副本，使播放界面无需再次查询来源。 */
  readonly track: Track;
}

/** 供来源适配器构造凭证。运行时不做校验 —— 约束来自类型系统。 */
export function createResolvedPlayback(
  fields: Omit<ResolvedPlayback, typeof PlaybackGrantBrand>,
): ResolvedPlayback {
  return { ...fields, [PlaybackGrantBrand]: true } as ResolvedPlayback;
}

/**
 * 凭证是否仍然可用。
 *
 * ⚠️ `expiresAt === null` 只表示"来源未声明过期"，**不表示永久有效**。
 * 实现方在长时间播放或恢复播放前仍应重新解析。
 */
export function isPlaybackExpired(
  playback: ResolvedPlayback,
  now: number = Date.now(),
): boolean {
  const cap = playback.capability;
  if (cap.kind !== "direct-url") return false;
  if (cap.expiresAt === null) return false;
  return now >= cap.expiresAt;
}

// ══════════════════════════════════════════════════════════════
// 搜索
// ══════════════════════════════════════════════════════════════

/** 搜索请求。 */
export interface SearchQuery {
  readonly text: string;
  readonly limit: number;
  /** 结果偏移，用于分页。来源不支持分页时可忽略。 */
  readonly offset?: number;
  readonly signal?: AbortSignal;
}

/**
 * 搜索命中项。
 *
 * **允许曲目存在但不可播放** —— `playback.kind === "unsupported"` 是
 * 合法且常见的状态（账号无权限、地区限制、来源不提供解析）。
 * UI 必须能表达这一状态，而不是把它当作错误。
 */
export interface SearchHit {
  readonly track: Track;
  /**
   * 搜索阶段已知的播放能力。
   *
   * 搜索结果**不是**播放授权：即使这里是 `direct-url`，也必须经
   * `resolvePlayback()` 重新解析后才能播放（链接可能已过期）。
   * 这里携带它只是为了让 UI 能提前把不可播项置灰。
   */
  readonly playback: PlaybackCapability;
}

/** 搜索结果。 */
export interface SearchResult {
  readonly sourceId: SourceId;
  readonly hits: readonly SearchHit[];
  /** 来源是否有更多结果（分页场景）。 */
  readonly hasMore: boolean;
  /** 本次搜索耗时（毫秒），用于诊断。 */
  readonly elapsedMs: number;
}

// ══════════════════════════════════════════════════════════════
// 队列
// ══════════════════════════════════════════════════════════════

/**
 * 队列项 —— 来源无关。
 *
 * 与现有 Rust `QueueEntry`（`domain/queue.rs`）的关系：
 * 现有类型的 `path: PathBuf` 只能表达本地文件，远程曲目无法表示。
 * 把队列扩展为跨来源是**独立的一步**，见
 * `docs/music-sources/interface-design.md` 的集成点一节。
 */
export interface QueueItem {
  readonly track: Track;
  /** 该队列项的来源上下文，用于"播放自哪个专辑/歌单"。 */
  readonly origin: QueueOrigin;
  readonly addedAt: number;
}

export type QueueOrigin =
  | { readonly kind: "local-library" }
  | { readonly kind: "source-album"; readonly sourceId: SourceId; readonly albumId: string }
  | { readonly kind: "source-playlist"; readonly sourceId: SourceId; readonly playlistId: string }
  | { readonly kind: "user" };

// ══════════════════════════════════════════════════════════════
// 来源能力与认证状态
// ══════════════════════════════════════════════════════════════

/**
 * 来源的静态能力声明。
 *
 * 能力是**声明**，不是保证：声明支持搜索的来源仍可能因网络问题失败。
 * 它的用途是让 UI 在调用前就知道该显示哪些入口。
 */
export interface SourceCapabilities {
  /** 是否支持搜索。false 时 UI 不应显示该来源的搜索入口。 */
  readonly search: boolean;
  /** 是否可以解析出可播放内容。 */
  readonly playbackResolve: boolean;
  /** 是否提供封面。 */
  readonly artwork: boolean;
  /** 是否提供歌词。 */
  readonly lyrics: boolean;
  /**
   * 是否需要认证才能使用。
   *
   * `true` 时未认证状态下 `search` 通常会失败，UI 应引导用户先配置。
   */
  readonly requiresAuth: boolean;
  /** 是否支持分页。 */
  readonly pagination: boolean;
  /**
   * 来源是否可能返回「有元数据但不可播放」的结果。
   *
   * 多数流媒体来源为 true。UI 据此决定是否普遍渲染置灰态。
   */
  readonly partialPlayback: boolean;
}

/** 来源的认证状态。 */
export type SourceAuthStatus =
  | { readonly kind: "not-required" }
  | { readonly kind: "unconfigured" }
  | { readonly kind: "configured"; readonly configuredAt: number }
  | { readonly kind: "authenticated"; readonly authenticatedAt: number; readonly expiresAt: number | null }
  | { readonly kind: "expired"; readonly expiredAt: number }
  | { readonly kind: "failed"; readonly message: string; readonly lastAttemptAt: number }
  /** 用户主动登出。与 `unconfigured` 区分，前者是选择，后者是初始状态。 */
  | { readonly kind: "signed-out" };

/** 来源在 UI 中呈现的完整状态。 */
export interface MusicSourceStatus {
  readonly id: SourceId;
  readonly displayName: string;
  /** 来源类型，用于选择图标与说明文案。 */
  readonly kind: "local" | "navidrome" | "gdstudio" | "unknown";
  readonly capabilities: SourceCapabilities;
  readonly auth: SourceAuthStatus;
  /**
   * 用户是否启用了该来源。
   *
   * ⚠️ 默认必须为 `false`（FR-9.2）。来源禁用**不得**影响本地音乐功能。
   */
  readonly enabled: boolean;
  /**
   * 来源是否可用于实际操作。
   *
   * = enabled && 认证状态满足 && 实现方自检通过。
   * UI 应以此决定是否允许发起搜索。
   */
  readonly ready: boolean;
  /** 来源实现是否为占位（尚无可用后端）。GD Studio 当前即为占位。 */
  readonly placeholder: boolean;
}

// ══════════════════════════════════════════════════════════════
// 错误
// ══════════════════════════════════════════════════════════════

/**
 * 来源错误。
 *
 * 与现有 `src/ipc/types.ts` 的 `ErrorPayload` 的关系：
 * `ErrorPayload` 是**跨 IPC 的通用错误信封**（code/message/retryable）。
 * 本类型是**来源模块内部**的细分错误，最终会被映射为 `ErrorPayload`
 * 的 code，不新增并行通道。
 */
export type MusicSourceError =
  | { readonly kind: "not-configured"; readonly sourceId: SourceId }
  | { readonly kind: "disabled"; readonly sourceId: SourceId }
  | { readonly kind: "auth-required"; readonly sourceId: SourceId }
  | { readonly kind: "auth-failed"; readonly sourceId: SourceId; readonly message: string }
  | { readonly kind: "network"; readonly sourceId: SourceId; readonly message: string }
  | { readonly kind: "timeout"; readonly sourceId: SourceId; readonly timeoutMs: number }
  | { readonly kind: "cancelled"; readonly sourceId: SourceId }
  | { readonly kind: "rate-limited"; readonly sourceId: SourceId; readonly retryAfterSecs: number | null }
  | { readonly kind: "not-found"; readonly sourceId: SourceId }
  | { readonly kind: "forbidden"; readonly sourceId: SourceId }
  | { readonly kind: "server-error"; readonly sourceId: SourceId; readonly status: number }
  | { readonly kind: "invalid-response"; readonly sourceId: SourceId; readonly detail: string }
  /** 解析出的播放地址未通过安全校验。 */
  | { readonly kind: "stream-rejected"; readonly sourceId: SourceId; readonly reason: StreamRejectionReason }
  /** 该来源尚不可用（如 GD Studio 文档未核实）。 */
  | { readonly kind: "not-implemented"; readonly sourceId: SourceId; readonly reason: string };

/** 播放地址被拒绝的原因。与 Rust 侧 `StreamRejection` 对齐。 */
export type StreamRejectionReason =
  | "unparseable"
  | "disallowed-scheme"
  | "disallowed-host"
  | "contains-credentials"
  | "disallowed-port"
  | "not-audio"
  | "too-large"
  | "too-many-redirects"
  | "expired";

// ══════════════════════════════════════════════════════════════
// 来源接口
// ══════════════════════════════════════════════════════════════

/**
 * 音乐来源适配器契约。
 *
 * # 实现位置
 *
 * **Rust 侧**（`infra/sources/`）。TS 实现被 NFR-4.2 禁止。
 * 本接口在此声明，用于对照评审与前端投影类型推导。
 *
 * # 能力分离
 *
 * 四个能力**互相独立**，实现方可以只支持其中一部分：
 *
 * | 能力 | 方法 | 说明 |
 * |---|---|---|
 * | 搜索 | `search()` | 只返回元数据，不构成播放授权 |
 * | 播放解析 | `resolvePlayback()` | 唯一能产出 `ResolvedPlayback` 的入口 |
 * | 认证 | `authStatus()` / `authenticate()` | 与搜索解耦；公共来源可不需认证 |
 * | 播放权限 | `PlaybackCapability` | 由解析结果表达，可能是 `unsupported` |
 *
 * 来源可以支持搜索但**不支持播放**（例如只提供元数据的目录服务）。
 * 接口不强制两者同时具备。
 */
export interface MusicSource {
  readonly id: SourceId;
  readonly displayName: string;
  readonly capabilities: SourceCapabilities;

  /**
   * 搜索曲目。
   *
   * 必须支持取消与超时。取消不得被当作错误上报给用户。
   */
  search(query: SearchQuery): Promise<SearchResult>;

  /**
   * 解析播放能力 —— **唯一**能产出 `ResolvedPlayback` 的入口。
   *
   * 实现方职责：
   * - 校验解析出的 URL（协议、主机白名单、无凭据、MIME、体积）
   * - 跟随重定向时**逐跳重新校验**
   * - 不返回裸 URL；给出 `unsupported` 而不是抛错来表示"不可播放"
   */
  resolvePlayback(
    sourceTrackId: SourceTrackId,
    signal?: AbortSignal,
  ): Promise<ResolvedPlayback>;

  /** 查询当前认证状态。不应发起网络请求。 */
  authStatus(): SourceAuthStatus;

  /**
   * 发起认证。
   *
   * 凭据由 Rust 侧密钥环读取，**不得**经由本接口的参数传入，
   * 也不得在返回值或日志中出现。
   */
  authenticate(signal?: AbortSignal): Promise<SourceAuthStatus>;

  /** 清除已保存的凭据。 */
  signOut(): Promise<void>;
}

// ══════════════════════════════════════════════════════════════
// 注册表
// ══════════════════════════════════════════════════════════════

/**
 * 来源注册表。
 *
 * # 不变量（实现必须保证）
 *
 * 1. `"local"` 来源**始终存在且不可移除**，`enabled` 恒为 `true`
 * 2. 所有在线来源**默认 `enabled === false`**（FR-9.2）
 * 3. 禁用/移除/注销任何在线来源，**不得**影响本地来源的能力
 * 4. 未通过文档核实与合规审查的来源，以 `placeholder: true` 注册，
 *    且**不得**返回任何伪造的可播放结果
 */
export interface MusicSourceRegistry {
  /** 列出全部来源（含禁用的）。 */
  list(): Promise<readonly MusicSourceStatus[]>;

  /** 取得单个来源的投影状态。不存在时返回 null，而非抛错。 */
  get(id: SourceId): Promise<MusicSourceStatus | null>;

  /** 启用/禁用来源。禁用应立即终止该来源进行中的请求。 */
  setEnabled(id: SourceId, enabled: boolean): Promise<MusicSourceStatus>;

  /**
   * 跨来源搜索。
   *
   * 必须容忍部分来源失败：单个来源出错不应使整体失败，
   * 失败信息随结果一并返回。
   */
  searchAll(
    query: SearchQuery,
    sourceIds?: readonly SourceId[],
  ): Promise<MultiSourceSearchResult>;
}

export interface MultiSourceSearchResult {
  readonly results: readonly SearchResult[];
  /** 本次搜索中失败的来源。空数组表示全部成功。 */
  readonly failures: readonly MusicSourceError[];
  /** 因来源被禁用而跳过的来源。 */
  readonly skipped: readonly SourceId[];
}
