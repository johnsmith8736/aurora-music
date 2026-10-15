# 安全与错误处理

> 状态：设计阶段 · 最后更新：2026-10-15
> 证据标记约定见 [`README.md`](README.md) §0。

---

## 1. 授权边界

### 1.1 三条不可逾越的线

| # | 禁止 | 依据 |
|---|---|---|
| A-1 | 绕过 DRM、付费墙、认证、访问控制或地区限制 | `requirements.md` NG-2 / NG-3 |
| A-2 | 把搜索结果或元数据当作播放授权 | 本模块核心安全属性 |
| A-3 | 连接非用户自有的服务器（Navidrome 场景） | 见 `source-selection.md` §4 |

### 1.2 "可访问"与"被授权"的分离

设计上强制这一分离的机制：

- `SearchResult` 携带的 `PlaybackCapability` **只用于 UI 置灰**，
  **不构成播放授权**（`types.ts` 的 `SearchHit` 注释已明确）
- 播放**必须**经 `resolvePlayback()` 重新解析
- 即使搜索结果里已经是 `direct-url`，链接也可能已过期

**【推断】** 这一分离的价值在实现期才会完全体现：
当某个来源返回的搜索链接有效期只有几分钟时，
"搜索结果直接可播"的设计会在用户点击时静默失败。

### 1.3 GD Studio 的额外约束

在 D4/D5（授权与播放权限）取得可靠证据前：

- 不发起任何真实请求
- 不构造示例响应
- 不在文档中描述其 API 形态（哪怕是"可能是 REST"这类推测）

---

## 2. 凭证存储

### 2.1 方案对比

| 方案 | 安全性 | 依赖 | 状态 |
|---|---|---|---|
| 系统密钥环（Secret Service D-Bus） | 高：由桌面环境加密管理 | `secret-service` / `keyring` crate | **【待决策 D-2】** |
| 受限文件（`0600`，位于 `$XDG_DATA_HOME`） | 中：依赖文件权限 | 无 | 备选 |
| 数据库明文 | ❌ **禁止** | — | — |
| 前端 `localStorage` | ❌ **禁止**（违反 NFR-4.2 精神且易泄露） | — | — |

### 2.2 硬性规则

| # | 规则 |
|---|---|
| C-1 | 凭据**不得**写入数据库、日志、错误消息或 IPC 载荷 |
| C-2 | 凭据**不得**作为函数参数在前端与后端之间传递 |
| C-3 | 日志中的 URL 必须剥离查询串（签名参数即凭据） |
| C-4 | 错误消息面向用户时只说明"认证失败"，**不附带**服务端原始响应中的敏感字段 |
| C-5 | 登出必须真正删除已存储的凭据，而非仅置标志位 |

> C-3 与现有实现一致：`docs/architecture.md` §7.3 已定义 `redact_url()`。
> 本模块复用它，不另立规则。

### 2.3 认证失败的信息泄漏防护

**【项目内已验证】** 现有 `ErrorPayload` 的设计已要求消息脱敏
（`domain/error.rs` 的 `user_message()`）。

来源认证失败时的映射：

```
服务端原始响应（可能含用户名、服务器地址、内部错误）
        │  ← 完整内容只进日志（且脱敏）
        ▼
MusicSourceError::AuthFailed { message: "认证失败，请检查用户名与密码。" }
        │
        ▼
ErrorPayload { code: "SOURCE_AUTH_FAILED", message: <脱敏文案>, retryable: false }
```

**测试要求见 `testing-plan.md` §4.2** —— 必须有测试证明
认证失败时响应体中的凭据不出现在错误消息中。

---

## 3. URL 安全

### 3.1 校验矩阵

沿用 `docs/architecture.md` §2.10 的 `ResolvedStream::validate()` 设计。
**下表每一项都必须有对应的测试用例**（见 `testing-plan.md` §3）。

| # | 校验项 | 拒绝原因 | 说明 |
|---|---|---|---|
| U-1 | 可解析为绝对 URL | `Unparseable` | — |
| U-2 | scheme 必须是 `https` | `DisallowedScheme` | 开发模式可对 `localhost` 放宽 |
| U-3 | host 必须在来源声明的白名单内 | `DisallowedHost` | 白名单由适配器提供，不是全局 |
| U-4 | 不得含 userinfo（`user:pass@`） | `ContainsCredentials` | — |
| U-5 | 端口必须在允许集合内 | `DisallowedPort` | — |
| U-6 | 禁止 `file://` / `data:` / `javascript:` / `blob:` | `DisallowedScheme` | — |
| U-7 | MIME 必须在音频白名单内 | `NotAudio` | 防止把 HTML 错误页当音频 |
| U-8 | 体积不超过上限 | `TooLarge` | 上限由策略配置 |
| U-9 | 重定向跳数 ≤ 5 | `TooManyRedirects` | — |
| U-10 | 过期链接不得使用 | `Expired` | 见 §5 |

### 3.2 重定向必须逐跳校验

```
请求 → 302 Location: https://evil.example/x.mp3
         │
         └─► ⚠️ 危险：初始 URL 合法，重定向目标不合法
```

**处置：** 适配器必须设置 `redirect::Policy::none()`，自行跟随重定向，
**每一跳都重新执行 U-1 ~ U-9**。

**【推断】** 这是最容易被遗漏的绕过路径：只校验初始 URL 的实现
在遇到恶意重定向时会直接把未验证的地址交给引擎。

### 3.3 URL 的生命周期

| 阶段 | 处理 |
|---|---|
| 解析 | `resolvePlayback()` 校验后产出 `ResolvedPlayback` |
| 使用 | 交给引擎前再次检查 `isPlaybackExpired()` |
| 持久化 | ❌ **不落库**。队列只存 `sourceId` + `sourceTrackId` |
| 日志 | 只记 `scheme://host/path`，丢弃 query 与 fragment |
| 错误消息 | 完全不出现 URL |

---

## 4. 限流、超时与重试

### 4.1 超时（由调用方统一施加）

| 操作 | 建议超时 | 依据 |
|---|---|---|
| 连通性探测 | 5 s | 需要快速反馈 |
| 搜索 | 10 s | 用户等待上限 |
| 播放解析 | 10 s | 同上 |
| 音频流本身 | **不设总超时** | 流式传输可能持续很久 |

> 具体数值**【推断】**，实施时应实测调整。

### 4.2 重试策略

| 情况 | 是否重试 | 次数 |
|---|---|---|
| 网络错误 | ✅ | 2 次，指数退避 |
| 超时 | ✅ | 1 次 |
| 429 限流 | ✅ | 遵循 `Retry-After`；无该头时退避后重试 1 次 |
| 5xx | ✅ | 1 次 |
| 401 / 403 | ❌ | 认证问题，重试无意义 |
| 404 | ❌ | — |
| 解析失败 | ❌ | 响应格式问题，重试不会变 |
| **用户取消** | ❌ | **不得重试** |

### 4.3 取消优先级

取消必须**立即**生效，且**不得**被当作错误上报给用户：

```
取消 → CancellationToken 触发 → 在途请求中止
     → 返回 MusicSourceError::Cancelled
     → 上层识别为 Cancelled，静默丢弃（不显示错误横幅）
```

**【项目内已验证】** 现有 `PlaybackError` 无 `Cancelled` 变体；
本模块在 `MusicSourceError` 中新增，映射到 `ErrorPayload` 时
应使用非错误路径或可被前端静默忽略的 code。

### 4.4 并发限制

**【推断】** 跨来源搜索时，若来源数量较多，应限制并发数（如 ≤ 4），
避免同时打开过多连接。具体上限实施时实测确定。

---

## 5. 过期链接

### 5.1 三态语义

| `expiresAt` | 含义 | 实现方义务 |
|---|---|---|
| `Some(t)` | 声明 `t` 后失效 | 使用前检查 `isPlaybackExpired()` |
| `None` | **未声明**过期 | ⚠️ **不得**当作永久有效 |

### 5.2 处理策略

| 场景 | 处理 |
|---|---|
| 解析后立即播放 | 检查未过期即可 |
| 长时间播放中途过期 | **不中断播放**（流已建立）；仅标记凭证过期 |
| 暂停后恢复 | 恢复前重新解析 |
| 从持久化队列恢复播放 | **必须**重新解析，不信任历史记录 |
| 解析时已过期 | 重新解析一次；仍过期则返回 `Expired` |

### 5.3 为什么不落库

```
若把签名 URL 存进队列表：
  用户关闭应用 → 数小时后重开 → 播放队列
  → 使用过期 URL → 播放失败
  → 错误消息不能含 URL（脱敏规则）
  → 用户看到"无法播放"但不知原因 → 无法诊断
```

**结论：** 队列只持久化 `sourceId` + `sourceTrackId`，
播放时重新解析。代价是恢复播放慢一点，收益是行为可预测。

---

## 6. 来源禁用策略

### 6.1 禁用时的完整行为

| # | 行为 | 理由 |
|---|---|---|
| F-1 | 立即取消该来源**全部在途请求** | 避免用户等了半天才失败 |
| F-2 | 队列中的该来源曲目**标记为不可播放**，但**不从队列移除** | 静默移除会让用户以为数据丢了 |
| F-3 | 播放到该曲时按 FR-2.6 **跳过并提示** | 与损坏文件的处理一致 |
| F-4 | **本地来源的能力完全不受影响** | I-3，FR-9.2 推论 |
| F-5 | 已保存的凭据**保留**，除非用户主动登出 | 禁用 ≠ 登出 |
| F-6 | 该来源不出现在跨来源搜索中，计入 `skipped` | 而非 `failures` |

### 6.2 与"登出"的区别

| | 禁用 | 登出 |
|---|---|---|
| 凭据 | 保留 | 删除 |
| 在途请求 | 取消 | 取消 |
| 重新启用后 | 直接可用 | 需重新认证 |
| 用户意图 | "暂时不用" | "不再用这个账号" |

---

## 7. 错误码

### 7.1 映射到现有 `AppError` 体系

**【项目内已验证】** 现有 `domain/error.rs` 的 `AppError` 有五个顶层变体
（`Library` / `Playback` / `Storage` / `InvalidArgument` / `Internal`），
每个错误码的格式是 `SCREAMING_SNAKE_CASE`。

**本模块新增 `AppError::Source(MusicSourceError)` 变体**，
不另立并行通道。错误码命名沿用 `SOURCE_*` 前缀。

### 7.2 错误码表

| 错误 | code | retryable | 用户可见文案（脱敏） |
|---|---|---|---|
| `NotConfigured` | `SOURCE_NOT_CONFIGURED` | ✗ | "该来源尚未配置。" |
| `Disabled` | `SOURCE_DISABLED` | ✗ | "该来源已被禁用。" |
| `AuthRequired` | `SOURCE_AUTH_REQUIRED` | ✗ | "请先登录该来源。" |
| `AuthFailed` | `SOURCE_AUTH_FAILED` | ✗ | "认证失败，请检查凭据。" |
| `Network` | `SOURCE_NETWORK` | ✓ | "网络连接失败，请重试。" |
| `Timeout` | `SOURCE_TIMEOUT` | ✓ | "请求超时，请重试。" |
| `Cancelled` | `SOURCE_CANCELLED` | — | **不显示**（用户主动取消） |
| `RateLimited` | `SOURCE_RATE_LIMITED` | ✓ | "请求过于频繁，请稍后再试。" |
| `NotFound` | `SOURCE_NOT_FOUND` | ✗ | "该内容不存在。" |
| `Forbidden` | `SOURCE_FORBIDDEN` | ✗ | "无权访问该内容。" |
| `ServerError` | `SOURCE_SERVER_ERROR` | ✓ | "服务器返回错误。" |
| `InvalidResponse` | `SOURCE_INVALID_RESPONSE` | ✗ | "服务器返回了无法识别的数据。" |
| `StreamRejected` | `SOURCE_STREAM_REJECTED` | ✗ | "该内容的播放地址未通过安全检查。" |
| `NotImplemented` | `SOURCE_NOT_IMPLEMENTED` | ✗ | "该来源暂不可用。" |

> 上表与 `docs/architecture.md` §6.3 的既有错误码表风格一致，
> 且**不重命名**任何已有 code。

### 7.3 脱敏要求

| 字段 | 要求 |
|---|---|
| `message` | 不含 URL、服务器地址、用户名、令牌、查询串 |
| 日志中的同一错误 | 可含完整细节，但 URL 仍需 `redact_url()` |
| `correlation_id` | 关联两者（现有 `ErrorPayload` 已有该设计） |

---

## 8. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| 0.1.0 | 2026-10-15 | 初稿 |
