# 测试计划

> 状态：设计阶段 · 最后更新：2026-10-15
> 证据标记约定见 [`README.md`](README.md) §0。

---

## 1. 本阶段已执行的测试

### 1.1 类型级测试（已执行 ✅）

由于项目**尚无前端测试框架**（已核实：`package.json` 无 vitest/jest），
本阶段用 TypeScript 编译器本身作为测试执行器。

**机制：** `@ts-expect-error` 注释由 `tsc` 校验 ——
若下一行**没有**类型错误，编译会因"未使用的 @ts-expect-error"而失败。

**文件：** `src/services/music-sources/type-tests.ts`
**执行：** `npx tsc --noEmit`

| 断言 | 保护的安全属性 |
|---|---|
| `Track` 不能赋给 `ResolvedPlayback` | 元数据 ≠ 播放授权 |
| 对象字面量不能构造 `ResolvedPlayback` | 凭证不可凭空伪造 |
| `SearchHit` 允许 `unsupported` 播放能力 | 搜索结果可以为不可播 |
| `PlaybackCapability` 有四种形态 | 不强行统一为直接 URL |
| 判别联合穷尽性（两个函数） | 新增分支时强制处理 |
| `expiresAt === null` 不判过期 | 三态语义 |

**【项目内已验证】** 已做受控实验证明这些断言**真的在拦截错误**：

```
实验 1（移除 @ts-expect-error）：
  error TS2739: Type 'Track' is missing the following properties
  from type 'ResolvedPlayback': capability, resolvedAt, track, [PlaybackGrantBrand]

实验 2（试图凭空构造凭证）：
  error TS2741: Property '[PlaybackGrantBrand]' is missing in type
  '{ sourceId: string; ... }' but required in type 'ResolvedPlayback'

恢复后：tsc 退出码 0
```

若不进行这两个实验，"tsc 通过"本身并不能证明断言有效。

### 1.2 本阶段**未**执行的测试

本阶段为纯设计阶段，**不连接任何真实服务**，因此：

| 未执行 | 原因 |
|---|---|
| 适配器集成测试 | 适配器尚未实现 |
| 真实服务器的契约测试 | 禁止发起真实第三方请求 |
| GD Studio 任何测试 | API 文档未核实，实现被阻塞 |
| 网络失败场景测试 | 无实现可测 |

---

## 2. 实施阶段的测试层级

```
        ╱╲          E2E（不含在线来源）
       ╱  ╲
      ╱────╲        本地功能回归 —— **最关键**（§5）
     ╱      ╲
    ╱────────╲      适配器测试（wiremock 模拟服务器，§4）
   ╱          ╲
  ╱────────────╲    契约测试：所有适配器共用一套（§3）
 ╱              ╲
╱────────────────╲  单元测试：URL 校验、错误映射、过期判定（§6）
```

**测试位置：** Rust 侧（`src-tauri/tests/`），与现有 105 个测试同体系。
因为实现位于 Rust（见 `interface-design.md` §1.1）。

---

## 3. 契约测试

沿用现有 `testing.md` §4.6 的模式：**所有 `MusicSource` 实现必须通过同一套套件。**

```rust
/// 任何 MusicSource 实现都必须满足的行为契约。
pub async fn assert_source_contract<S: MusicSource>(src: &S) { /* ... */ }
```

| # | 契约 | 断言 |
|---|---|---|
| CT-1 | `id()` / `display_name()` 非空 | — |
| CT-2 | `capabilities()` 与实际行为一致 | 声明不支持搜索则 `search()` 返回 `Unsupported` 类错误 |
| CT-3 | 空查询不 panic | 返回空结果或明确错误 |
| CT-4 | 搜索结果的每个 hit 都能被 `resolve_playback()` 处理 | 或返回明确的 `Unsupported` |
| CT-5 | **`resolve_playback()` 永不返回裸 URL** | 返回值类型强制（编译期） |
| CT-6 | 不存在的 track id → `NotFound`，不 panic | — |
| CT-7 | 网络错误 → `Network`，不 panic | — |
| CT-8 | 取消 → `Cancelled`，且**不重试** | — |
| CT-9 | `auth_status()` 不发起网络请求 | 用 mock 计数器验证 |
| CT-10 | `placeholder: true` 的来源不返回伪造结果 | 见 §4.4 |

---

## 4. 适配器测试

### 4.1 通用原则

**全部使用 `wiremock` 或等价工具启动本地模拟服务器，禁止访问真实互联网。**
这与现有测试的约束一致（`testing.md` §3.4）。

### 4.2 搜索场景

| # | 场景 | 输入 | 期望 |
|---|---|---|---|
| S-1 | 正常结果 | 有效响应 | 映射为 `SearchHit[]`，字段正确 |
| S-2 | 空结果 | 零命中响应 | `hits: []`，**不报错** |
| S-3 | 无效应答 | 非 JSON / 缺字段 / 类型不符 | `InvalidResponse`，不 panic |
| S-4 | 分页 | 带 offset | `hasMore` 正确 |
| S-5 | 超时 | 服务器延迟 > 超时 | `Timeout` |
| S-6 | 网络错误 | 连接被拒 | `Network` |
| S-7 | 中途取消 | 请求中触发 CancellationToken | `Cancelled` |
| S-8 | 部分曲目不可播 | 响应含无权限曲目 | 映射为 `Unsupported`，**其余照常返回** |

### 4.3 HTTP 状态码矩阵（**必测**）

| 状态码 | 期望错误 | retryable |
|---|---|---|
| 401 | `AuthRequired` 或 `AuthFailed` | ✗ |
| 403 | `Forbidden` | ✗ |
| 404 | `NotFound` | ✗ |
| 429 | `RateLimited`（读 `Retry-After`） | ✓ |
| 500 / 502 / 503 | `ServerError` | ✓ |

> ⚠️ **Subsonic 协议特有：** 错误可能以 **HTTP 200 + 响应体标记失败** 的形式返回
> （见 `source-selection.md` §3.4）。因此必须有测试覆盖
> **"HTTP 200 但语义失败"**的用例，否则适配器会把它当成功处理。
> 具体判定字段 **【待核实】**（S-5）。

### 4.4 GD Studio 专项（**阻塞性测试**）

这些测试**在适配器实现之前就要写好**，用于证明"未核实的来源不会意外启用"：

| # | 测试 | 断言 |
|---|---|---|
| G-T1 | 注册表初始化后 | GD Studio 的 `enabled === false` 且 `placeholder === true` |
| G-T2 | 尝试启用 | 返回明确错误，而非静默成功 |
| G-T3 | 调用 `search()` | 返回 `NotImplemented`，**不发起任何网络请求**（用 mock 计数验证为零） |
| G-T4 | 调用 `resolve_playback()` | 返回 `NotImplemented`，**不返回任何 `ResolvedStream`** |
| G-T5 | UI 投影 | `ready === false`，`capabilities` 全为 `false` |

### 4.5 凭证泄漏防护（**必测**）

| # | 测试 | 断言 |
|---|---|---|
| C-T1 | 认证失败时 | 错误消息中**不出现**用户名、密码、令牌、服务器地址 |
| C-T2 | 认证失败时 | 日志（若可捕获）中的 URL 查询串已被 `redact_url()` 剥离 |
| C-T3 | `ErrorPayload` 序列化后 | 不含任何凭据字段 |
| C-T4 | 登出后 | 存储中确实不再有凭据（而非仅置标志位） |

**C-T1 的具体构造：** 让 mock 服务器在 401 响应体中**故意回显**
收到的用户名与令牌，然后断言它们**没有**出现在最终的用户可见消息里。
这样即使服务端行为不友好，客户端也不会转发敏感信息。

### 4.6 URL 安全（**对应 `security-and-errors.md` §3.1 的 U-1 ~ U-10**）

每个校验项一个用例。这是**安全关键测试**，覆盖率要求 100%：

| 输入 | 期望 |
|---|---|
| `https://cdn.example.invalid/a.flac` | ✅ 通过 |
| `http://...` | ❌ `DisallowedScheme` |
| `file:///etc/passwd` | ❌ `DisallowedScheme` |
| `data:audio/mp3;base64,...` | ❌ `DisallowedScheme` |
| `javascript:alert(1)` | ❌ `DisallowedScheme` |
| `https://evil.invalid/a.flac` | ❌ `DisallowedHost` |
| `https://user:pass@cdn.example.invalid/a` | ❌ `ContainsCredentials` |
| `https://cdn.example.invalid:22/a` | ❌ `DisallowedPort` |
| MIME = `text/html` | ❌ `NotAudio` |
| 体积超上限 | ❌ `TooLarge` |
| 重定向 6 跳 | ❌ `TooManyRedirects` |
| **合法 URL → 重定向到 `file://`** | ❌ **必须拒绝**（逐跳校验） |
| **合法 URL → 重定向到非白名单主机** | ❌ **必须拒绝** |

> 最后两条是最容易遗漏的绕过路径，**必须**有独立用例。

### 4.7 过期与队列恢复

| # | 场景 | 期望 |
|---|---|---|
| E-1 | `expiresAt` 已过 | `resolve_playback()` 返回 `Expired` |
| E-2 | `expiresAt === null` | **不判为过期** |
| E-3 | 从持久化队列恢复播放 | **重新解析**，不使用历史 URL |
| E-4 | 队列表内容检查 | **不含任何 URL**，只有 `sourceId` + `sourceTrackId` |

### 4.8 生命周期

| # | 场景 | 期望 |
|---|---|---|
| L-1 | 禁用来源 | 在途请求被取消 |
| L-2 | 禁用来源 | 队列中该来源曲目标记不可播，**但不被移除** |
| L-3 | 禁用来源 | 重启用后凭据仍在 |
| L-4 | 登出 | 凭据被删除，重启用需重新认证 |
| L-5 | 授权过期 | `authStatus` 变为 `Expired`，搜索返回 `AuthRequired` |
| L-6 | 未配置就搜索 | `NotConfigured` |

---

## 5. 本地功能回归（**最关键的一组**）

**对应需求 I-3 / FR-9.2 推论：禁用全部在线来源后，本地音乐必须完全可用。**

这组测试是"在线来源模块没有破坏既有功能"的唯一保证。

### 5.1 测试前置

```rust
// 启用全部在线来源 → 再全部禁用 → 断言本地功能不受影响
```

### 5.2 断言矩阵

| # | 本地能力 | 断言 | 对应现有测试 |
|---|---|---|---|
| R-1 | 目录扫描 | 结果与禁用前**完全一致** | `scan_real_audio.rs` |
| R-2 | 曲目列表 | 数量、内容一致 | `ipc_commands.rs` |
| R-3 | 搜索 | `search_finds_real_tracks_by_tag_content` 仍通过 | 同文件 |
| R-4 | 播放 | 本地文件播放不受影响 | `real_playback.rs` |
| R-5 | 队列 | 前进/后退/跳转行为一致 | `playback_service.rs` 单测 |
| R-6 | 收藏 | **若已实现**，读写正常 | 待实现 |
| R-7 | 启动 | **无任何在线来源时应用正常启动** | — |
| R-8 | 启动 | 无网络时应正常启动（不因来源初始化失败而卡住） | — |

> **R-7 / R-8 特别重要：** 应用启动路径**不得**同步等待任何在线来源。
> 来源初始化失败必须降级为"该来源不可用"，而不是启动失败。

### 5.3 执行方式

**【推断】** 最可靠的实现方式不是新增测试，而是**在现有 105 个测试
运行时把在线来源全部注册为"已禁用"**，确认它们仍全部通过。
这样回归保护是自动获得的，不依赖新写的测试。

---

## 6. 单元测试

| 模块 | 测试内容 |
|---|---|
| URL 校验 | §4.6 的完整矩阵（安全关键，100% 覆盖） |
| 错误映射 | `MusicSourceError` → `ErrorPayload` 的 code 与消息脱敏 |
| 过期判定 | 三态语义；本地文件永不过期 |
| 重定向跟随 | 逐跳校验、跳数上限 |
| 响应解析 | 缺字段降级为 `null` 而非整体失败 |
| 跨来源聚合 | 部分失败不使整体失败；`skipped` 与 `failures` 区分正确 |

---

## 7. 测试数据

| 类型 | 来源 | 是否入库 |
|---|---|---|
| mock 服务器响应 | 手写在测试内 | ✅ |
| 真实服务器响应样本 | **禁止**（含真实凭据与签名 URL） | ❌ |
| 音频夹具 | 复用现有 `tests/fixtures/audio/` | ✅ 已在库 |

> ⚠️ **禁止把真实服务器响应粘贴进测试。** 这类响应常含用户名、
> 令牌与带签名的 URL。所有 mock 响应必须手工构造，字段值使用
> 明显的占位符（`example.invalid`、`TEST_TOKEN`）。

---

## 8. 明确不做的测试

| # | 不测 | 理由 |
|---|---|---|
| N-1 | 对真实 Navidrome 服务器的集成测试 | 需要用户凭据与网络；应作为**人工验收**而非自动化测试 |
| N-2 | GD Studio 的任何真实请求 | API 文档未核实 |
| N-3 | 真实流媒体服务的播放授权验证 | 法律风险 |
| N-4 | 跨服务器实现的兼容性矩阵 | 需要多个真实服务器；标记为已知缺口 |

**人工验收清单**（自动化测试无法覆盖）：

- [ ] 用户配置自己的 Navidrome 地址与凭据后可搜索
- [ ] 用户可播放解析出的流
- [ ] 凭据在系统密钥环中可见（或按所选方案存储）
- [ ] 禁用来源后本地功能完全不受影响
- [ ] 无网络环境下应用正常启动

---

## 9. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| 0.1.0 | 2026-10-15 | 初稿；记录类型级测试已执行及受控实验证据 |
