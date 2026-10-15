# Aurora Music — 测试策略

> 文档版本：0.1.0 · 状态：待评审 · 最后更新：2026-10-15
>
> 需求编号见 `requirements.md`，模块职责见 `architecture.md`。
>
> **核心命题：一个音乐播放器最难测的部分是"时间"和"声音"。本文档的大部分设计都是为了把这两样东西变成可替换的。**

---

## 1. 指导原则

| # | 原则 | 含义 |
|---|---|---|
| T1 | **可测性是设计属性，不是事后补丁** | 端口化、时钟注入、纯逻辑分离，在 M0 就确立 |
| T2 | **不测第三方库** | 不验证 rodio 能否解码 MP3；验证**我们是否正确使用了它** |
| T3 | **测试必须确定性** | 禁止 `sleep()`、禁止依赖真实时间、禁止依赖网络与声卡 |
| T4 | **测试即文档** | 测试名描述行为，不描述实现（用 `plays_next_track_when_current_ends`，不用 `test_advance_2`） |
| T5 | **回归测试必须随 bug 修复一起提交** | 没有回归测试的修复视为未完成 |
| T6 | **不为了让测试通过而削弱断言** | 测试红了先查代码，再查测试 |
| T7 | **关键路径必须有真实验证** | 出声、媒体键、封面渲染无法靠假数据验证，必须人工确认 |

---

## 2. 测试分层

```
        ╱╲          E2E（少量，M3+ 评估）
       ╱  ╲         真实应用 + WebDriver
      ╱────╲
     ╱      ╲       集成测试（中等）
    ╱        ╲      DB + 扫描 + 来源，用 :memory: / tempdir / mock server
   ╱──────────╲
  ╱            ╲    契约测试（少量但关键）
 ╱              ╲   每个 MusicSource 实现须通过同一套一致性套件
╱────────────────╲
╱                  ╲ 单元测试（大量）
────────────────────  纯逻辑：队列、LRC、URL 校验、错误映射
```

| 层级 | 数量占比 | 执行时间 | 依赖 |
|---|---|---|---|
| 单元 | ~65% | < 1 s 总计 | 无 |
| 契约 | ~10% | < 5 s | mock HTTP |
| 集成 | ~20% | < 30 s | `:memory:` / tempdir / mock HTTP |
| **真实硬件** | 少量但不可省 | < 10 s | **真实声卡**（无设备时自动跳过） |
| 前端 | ~5% | < 20 s | jsdom + mock IPC |
| E2E | 少量 | 数分钟 | 真实二进制 + 显示服务器 |

### 关于「真实硬件」层（`tests/real_playback.rs`）

无声替身能证明**逻辑正确**，但不能证明**真的出声**。历史上最常见的失败
模式恰恰是：逻辑全绿、界面正常、**但没有任何声音**。

这一层用真正的 rodio + cpal 引擎播放断言：

| 测试 | 证明 |
|---|---|
| `real_engine_plays_a_file_to_completion` | 位置真的推进，且结束位置精确等于声明时长 |
| `real_engine_reports_early_end_for_truncated_audio` | 截断检测的前提在真实引擎上成立（实测 64%） |
| `real_engine_rejects_corrupt_file_at_load` | 两种损坏模式都在加载期被拒绝 |
| `real_engine_seek_moves_position` | 跳转生效且越界报错 |
| `real_engine_survives_rapid_track_switching` | 快速切歌不崩溃 |
| `engine_is_shareable_across_threads` | `Send + Sync` 在运行期确实可用 |

**这一层立刻证明了价值**：它在第一次运行时就发现了「曲目播完后
`state()` 仍返回 `Playing`」这一真实缺陷（见 `architecture.md` §10.3 F-12），
而当时全部 73 个单元测试都是绿的。

> 无音频设备时这些测试**跳过**而非失败 —— CI 容器通常没有声卡。

---

## 3. 可测性的四个关键设计

这四项是整个测试策略成立的前提，**必须在 M0 落地**。

### 3.1 音频输出可替换（最重要）

```rust
// domain/ports.rs
pub trait AudioOutput: Send + Sync { /* 见 architecture.md §2.3 */ }
```

测试替身：

```rust
// infra/audio/null_engine.rs  —— 编译进生产二进制，测试与"无设备"降级共用
pub struct NullAudioEngine {
    state: Mutex<EngineState>,
    clock: Arc<dyn Clock>,
}

pub struct EngineState {
    loaded: Option<LoadedTrack>,
    pos: Duration,
    playing: bool,
    volume: f32,
    events: Vec<EngineEvent>,
}

impl AudioOutput for NullAudioEngine {
    fn load(&self, s: ResolvedStream) -> Result<(), PlaybackError> {
        let mut st = self.state.lock();
        st.loaded = Some(LoadedTrack {
            duration: st.probed_duration(),   // 由测试预设
            pos: Duration::ZERO,
        });
        Ok(())
    }

    fn position(&self) -> Option<Duration> {
        let st = self.state.lock();
        if !st.playing { return Some(st.pos); }
        Some(st.pos + self.clock.elapsed_since_last_advance())
    }

    // 关键：测试调用此方法模拟"时间流逝到曲目结束"
    fn take_events(&self) -> Vec<EngineEvent> { /* 从队列取出 */ }
}
```

**收益：** 整个播放推进逻辑（自动续播、循环模式、随机顺序、历史记录）可以在**没有声卡、没有真实时间**的条件下被完整测试。这是 `EndingDetector` 之外最值得投入的测试基础设施。

> `NullAudioEngine` 同时用于生产环境的一个真实场景：当系统没有可用音频设备时，应用降级到该引擎，界面仍可正常浏览资料库（FR-2.7），而不是启动失败。

### 3.2 时钟可注入

```rust
// domain/ports.rs
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn monotonic(&self) -> Duration;   // 用于位置计算
}

// 生产：SystemClock（直接转发）
// 测试：FakeClock（手动推进）
pub struct FakeClock { inner: Mutex<Duration> }
impl FakeClock {
    pub fn advance(&self, d: Duration) { self.inner.lock().0 += d; }
}
```

**禁止**在任何 `domain/` 或 `app/` 代码中直接调用 `SystemTime::now()` 或 `Instant::now()`。以 clippy 自定义 lint 或代码审查强制。

### 3.3 文件系统可构造

使用 `tempfile::TempDir` + `assert_fs` 在测试内构造目录树：

```rust
let dir = TempDir::new()?;
dir.child("Artist/Album/01 - Track.flac").write_binary(&fixture("flac_min"))?;
dir.child("Artist/Album/not_audio.txt").write_str("ignore me")?;
dir.child("Artist/Album/cover.jpg").write_binary(&fixture("cover_256"))?;
```

**不依赖仓库里的固定目录结构**——每个测试自建所需的最小结构，测试间零耦合。

### 3.4 网络可伪造

`wiremock` 启动本地 mock HTTP 服务器，返回预设响应：

```rust
let server = MockServer::start().await;
Mock::given(method("POST"))
    .and(path("/api/search"))
    .respond_with(ResponseTemplate::new(200).set_body_json(search_response()))
    .mount(&server).await;
```

**所有网络测试禁止访问真实互联网。** CI 中应无外网依赖。

---

## 4. 各模块测试要点

### 4.1 `domain/queue.rs`（单元，重点）

| 测试 | 验证 |
|---|---|
| 空队列 `advance()` 返回 `None` | 边界 |
| 单曲队列循环模式 | off / all / one 行为差异 |
| `RepeatMode::One` 时 `advance()` 返回自身 | FR-3.4 |
| 随机置换：连续 `advance()` N 次覆盖全部且无重复 | FR-3.5 |
| 随机置换：`advance()` 后 `previous()` 回到原曲 | FR-3.5（关键） |
| 序列化后再反序列化，顺序完全一致 | 持久化正确性 |
| **属性测试（`proptest`）**：任意操作序列后，队列长度、当前索引、置换三者始终自洽 | 不变量 |

> 队列逻辑是纯函数式的，是**最适合属性测试**的模块。建议用 `proptest` 定义不变量而非逐一列举用例。

### 4.2 `domain/stream.rs`（单元，安全关键）

`ResolvedStream::validate` 的拒绝矩阵——**每一个拒绝原因都必须有测试**：

| 输入 | 期望 |
|---|---|
| `https://cdn.example.com/a.mp3` | ✅ 通过 |
| `http://cdn.example.com/a.mp3` | ❌ `DisallowedScheme` |
| `file:///etc/passwd` | ❌ `DisallowedScheme` |
| `data:audio/mp3;base64,...` | ❌ `DisallowedScheme` |
| `javascript:alert(1)` | ❌ `DisallowedScheme` |
| `https://evil.com/a.mp3` | ❌ `DisallowedHost` |
| `https://user:pass@cdn.example.com/a.mp3` | ❌ `ContainsCredentials` |
| `https://cdn.example.com:22/a.mp3` | ❌ `DisallowedPort` |
| `https://cdn.example.com/page.html`（mime=text/html） | ❌ `NotAudio` |
| `not a url at all` | ❌ `Unparseable` |
| 大小为 0 字节 | ❌ `TooLarge`? 边界需明确 |
| 重定向链 6 跳 | ❌ `TooManyRedirects` |

**这是 P4 的验证点。** 测试文件命名为 `stream_validation_rejects.rs`，新增拒绝原因时必须同步新增用例。

### 4.3 `infra/lyrics/lrc.rs`（单元）

| 测试 | 验证 |
|---|---|
| 标准 LRC（`[mm:ss.xx]`） | FR-8.3 |
| 多时间戳行（`[00:10.00][00:20.00]同一句`） | 展开为两行 |
| 元数据标签（`[ar:]` `[ti:]` `[offset:]`） | 正确解析，不当作歌词 |
| 空行、无时间戳行 | 不崩溃 |
| 时间轴乱序 | 排序后再输出 |
| 无 `[offset:]` 时偏移为 0 | 默认值 |
| **GBK 编码文件** | 正确解码（FR-8.4） |
| **Shift-JIS 编码文件** | 正确解码 |
| UTF-8 带 BOM | 不残留 `\u{feff}` |
| 损坏的 LRC | 降级为纯文本，不报错 |

### 4.4 `infra/scan/`（集成）

| 测试 | 验证 |
|---|---|
| 新目录全量扫描 | 曲目数正确 |
| 再次扫描无变化 | 全部跳过（`files_skipped == files_seen`） |
| 修改文件 mtime | 仅该文件被更新 |
| 删除文件后扫描 | 该曲目标记 `missing_since`，**文件未被删除** |
| 目录内含非音频文件 | 被忽略 |
| 单文件不可读（权限 000） | 记入失败计数，其余正常完成（FR-1.10） |
| 符号链接指向父目录 | 不无限递归 |
| 取消令牌触发 | 扫描停止，已入库数据一致，会话状态 `cancelled` |
| 标签全缺失的文件 | 回退到文件名（FR-1.5） |
| 超长文件名 / 非 UTF-8 文件名 | 不崩溃（Linux 文件名可为任意字节） |

> 非 UTF-8 文件名在 Linux 上是**真实场景**（从其他系统拷贝的音乐）。必须处理，不能假设 UTF-8。

### 4.5 `infra/db/`（集成）

| 测试 | 验证 |
|---|---|
| 空库迁移到 v1 | schema 正确，`user_version` 正确 |
| 重复迁移 | 幂等，不重复执行 |
| 迁移失败 | 事务回滚，数据库保持可用 |
| **比程序更新的 schema** | 返回 `SchemaTooNew`，不损坏数据（NFR-2.3） |
| 外键级联：删除曲目 | `playlist_items` / `favorites` / `play_history` 一并清理 |
| FTS 触发器：插入/更新/删除曲目 | 搜索结果同步变化 |
| FTS：中文查询 | 返回预期结果（若 M3 实测结论为不支持，此测试改为记录已知限制） |
| 稀疏 position 重排 | 只更新被移动项，不重写整表 |
| WAL 模式生效 | `PRAGMA journal_mode` 返回 `wal` |

### 4.6 `infra/sources/`（契约 + 集成）

**契约测试（关键）：** 定义一套所有 `MusicSource` 实现都必须通过的套件：

```rust
/// 任何 MusicSource 实现都必须满足的行为契约。
pub async fn assert_source_contract<S: MusicSource>(src: &S) {
    // 1. id() 与 display_name() 非空
    // 2. search(空查询) 返回空或错误，不 panic
    // 3. search 结果中的每个 candidate 都能被 resolve（或返回明确错误）
    // 4. resolve 返回的 ResolvedStream 已通过校验（类型保证）
    // 5. resolve 不存在/无效的 candidate → 明确错误，不 panic
    // 6. 网络错误 → SourceError::Network，不 panic
}
```

每个适配器 + `LocalSource` 都必须运行此套件。**这是 P3（来源解耦）的可执行验证**——新增适配器时，契约测试自动强制其行为一致。

### 4.7 `app/playback_service.rs`（集成，用 NullAudioEngine + FakeClock）

| 测试 | 验证 |
|---|---|
| 播放 → 推进时钟 → 投递 `TrackEnded` | 自动加载下一首（FR-2.5） |
| `RepeatMode::One` 下的自然结束 | 重播同一首 |
| 队列最后一首 + `RepeatMode::Off` 结束 | 状态转为 `Stopped` |
| 队列最后一首 + `RepeatMode::All` 结束 | 回到第一首 |
| 解码失败事件 | 跳过并播放下一首（FR-2.6） |
| 播放开始 | 写入 `play_history`（FR-6.1） |
| 播放结束 | 更新 `ms_played` 与 `completed` |
| 播放中切换曲目 | 前一首的历史被正确结算 |
| 事件轮询线程与状态更新 | 无死锁（超时即失败） |

### 4.8 前端（Vitest + Testing Library）

| 测试 | 验证 |
|---|---|
| `ipc/client.ts` 错误归一化 | `AppError` → 前端类型映射正确 |
| 位置插值函数（纯函数） | 给定锚点与经过时间，输出位置正确且被 duration 截断 |
| 播放条组件 | 给定状态渲染正确的播放/暂停图标 |
| 歌词滚动定位 | 给定位置，高亮行索引正确（二分查找边界） |
| 虚拟列表 | 大量项下只渲染可见区域 |
| 键盘导航 | Tab 顺序合理，Enter/Space 可激活 |

IPC 在测试中必须 mock，不启动真实后端：

```ts
import { mockIPC } from "@tauri-apps/api/mocks";
mockIPC((cmd, args) => { /* 返回预设值 */ });
```

> ⚠️ `@tauri-apps/api/mocks` 的具体 API 需在 M1 实测确认，不要假定。

---

## 5. 测试夹具（Fixtures）

### 5.1 生成策略

**禁止下载或提交第三方音频文件。** 全部夹具由脚本本地生成：

```
tests/fixtures/
├── audio/
│   ├── tone_1s.wav          # 1 秒 440 Hz 正弦，纯代码生成
│   ├── tone_1s.flac         # ffmpeg 生成
│   ├── tone_1s.mp3          # ffmpeg 生成
│   ├── tone_1s.ogg          # ffmpeg 生成
│   ├── tone_1s.opus         # ffmpeg 生成
│   ├── tone_bad.flac        # 合法 flac 截断 50%（解码失败用例）
│   ├── tone_truncated.mp3   # 同上
│   └── not_really.mp3       # 文本文件改名（伪装用例）
├── tags/
│   ├── full_tags.flac       # 完整标签
│   ├── no_tags.mp3          # 无标签（测试回退）
│   └── cjk_tags.mp3         # 中日韩标签（编码用例）
├── lyrics/
│   ├── plain.lrc            # UTF-8 标准
│   ├── synced.lrc           # 多时间戳
│   ├── gbk.lrc              # GBK 编码
│   ├── sjis.lrc             # Shift-JIS 编码
│   ├── malformed.lrc        # 损坏
│   └── no_timestamps.lrc    # 纯文本
├── artwork/
│   ├── cover_256.jpg
│   ├── cover_rgba.png       # 带透明通道（resize 边界）
│   └── cover_corrupt.jpg
└── scripts/
    └── gen-fixtures.sh      # 重新生成全部夹具
```

### 5.2 生成脚本

```bash
#!/usr/bin/env bash
# tests/fixtures/scripts/gen-fixtures.sh
# 用途：重新生成全部测试夹具。所有音频均为程序合成的正弦波，
#       不包含任何第三方版权内容。
set -euo pipefail
cd "$(dirname "$0")/.."

# 440 Hz 正弦，1 秒，单声道 44.1 kHz
ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" \
       -ac 1 -y audio/tone_1s.wav
ffmpeg -i audio/tone_1s.wav -y audio/tone_1s.flac
ffmpeg -i audio/tone_1s.wav -b:a 128k -y audio/tone_1s.mp3
ffmpeg -i audio/tone_1s.wav -c:a libvorbis -y audio/tone_1s.ogg
ffmpeg -i audio/tone_1s.wav -c:a libopus -y audio/tone_1s.opus

# 损坏样本：截断合法文件
head -c $(( $(stat -c%s audio/tone_1s.flac) / 2 )) audio/tone_1s.flac > audio/tone_bad.flac
head -c $(( $(stat -c%s audio/tone_1s.mp3)  / 2 )) audio/tone_1s.mp3  > audio/tone_truncated.mp3

# 编码夹具
iconv -f UTF-8 -t GBK  lyrics/plain.lrc > lyrics/gbk.lrc
iconv -f UTF-8 -t SHIFT-JIS lyrics/plain.lrc > lyrics/sjis.lrc

echo "夹具生成完成"
```

**单个夹具必须 < 20 KB。** 仓库体积受控。

### 5.3 未决

| 问题 | 说明 |
|---|---|
| `ffmpeg` 是否为测试依赖 | CI 需要安装；或改为**一次性生成后提交产物**，脚本仅用于重新生成 |

> **建议：提交生成的产物**（每个 < 20 KB，总数 < 200 KB），脚本用于必要时重生成。这样 CI 不需要 `ffmpeg`，测试也可离线运行。

---

## 6. 静态检查与门禁

### 6.1 Rust

| 检查 | 命令 | 门禁 |
|---|---|---|
| 格式 | `cargo fmt --all -- --check` | 必须通过 |
| 静态分析 | `cargo clippy --all-targets --all-features -- -D warnings` | **零告警** |
| 测试 | `cargo test --all-features` | 必须全绿 |
| 文档测试 | `cargo test --doc` | 必须通过 |
| 依赖重复 | `cargo tree -d` | 人工审查（防 symphonia 双版本） |

**必须启用的 clippy lint（在 `lib.rs` 顶部声明）：**

```rust
#![deny(clippy::unwrap_used)]        // 强制 §6.4 规则 1
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::indexing_slicing)]   // 用 .get() 代替 [i]
#![deny(clippy::todo, clippy::unimplemented)]
#![warn(clippy::pedantic)]
```

> `unwrap_used` / `expect_used` / `panic` 的 `deny` 是**把架构约束变成编译错误**的手段——比代码审查可靠。测试模块内用 `#[allow]` 局部豁免。

### 6.2 TypeScript

| 检查 | 命令 | 门禁 |
|---|---|---|
| 类型 | `tsc --noEmit` | **零错误** |
| 规范 | `eslint . --max-warnings 0` | 零告警 |
| 格式 | `prettier --check .` | 必须通过 |
| 测试 | `vitest run` | 必须全绿 |

**`tsconfig.json` 必须开启：**

```jsonc
{
  "compilerOptions": {
    "strict": true,
    "noUncheckedIndexedAccess": true,   // 数组访问返回 T | undefined
    "noImplicitOverride": true,
    "exactOptionalPropertyTypes": true
  }
}
```

### 6.3 CI 流水线

```yaml
# .github/workflows/ci.yml（结构示意）
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: 安装 Tauri 系统依赖
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
            libsoup-3.0-dev librsvg2-dev libasound2-dev libxkbcommon-dev
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all-features

  frontend:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: '26' }
      - run: npm ci
      - run: npx tsc --noEmit
      - run: npx eslint . --max-warnings 0
      - run: npx vitest run
```

**CI 中不执行 E2E。** E2E 在本地或专门的 runner 上跑。

### 6.4 覆盖率

**不设硬性覆盖率门槛。** 覆盖率是参考指标，不是目标——为凑覆盖率写的测试没有价值。

但以下模块要求**逻辑分支全覆盖**（人工审查）：

- `domain/stream.rs`（安全边界）
- `domain/queue.rs`（用户可感知行为）
- `domain/error.rs`（错误码映射）

---

## 7. 无法自动化的验证（人工清单）

以下内容**必须人工验证**，不列入 CI。每次发版前执行。

### 7.1 音频（需真实声卡）

- [ ] MP3 / FLAC / OGG / M4A / Opus 各播放一首，确认出声且无杂音
- [ ] 24-bit / 96 kHz FLAC 正确播放（位深与采样率转换）
- [ ] 单声道 / 立体声 / 5.1 文件不崩溃
- [ ] 曲目切换时无可闻爆音
- [ ] 暂停 → 恢复，位置连续
- [ ] 播放中拔掉耳机 → 不崩溃，重新插上可恢复（FR-2.7 相关）
- [ ] 音量 0% 时静音，100% 时不削波
- [ ] 系统音量与播放器音量独立生效

### 7.2 KDE / Wayland 集成

- [ ] 键盘媒体键控制播放/暂停/上一首/下一首
- [ ] `playerctl status` 与 `playerctl metadata` 输出正确
- [ ] Plasma 任务栏媒体控件显示标题、艺术家、封面
- [ ] 切换虚拟桌面、最小化、恢复窗口无渲染异常
- [ ] 分数缩放（125% / 150%）下界面不错位
- [ ] 多显示器下窗口行为正确
- [ ] 100% → 200% DPI 切换后界面重新布局正确

### 7.3 性能

- [ ] 冷启动到可交互 < 1.5 s（`systemd-analyze` 或手工秒表）
- [ ] 10 万曲目下搜索 p95 < 100 ms（需基准数据）
- [ ] 10 万曲目全量扫描 < 5 min
- [ ] 空闲内存 < 300 MB（`ps_mem` 或 `smem`）
- [ ] 扫描期间 UI 无卡顿（手工滚动列表观察）

### 7.4 数据安全

- [ ] **运行前后对音乐目录做校验和比对，确认零改动**
  ```bash
  find "$MUSIC_DIR" -type f -exec sha256sum {} + | sort > /tmp/before.txt
  # ... 运行应用并扫描 ...
  find "$MUSIC_DIR" -type f -exec sha256sum {} + | sort > /tmp/after.txt
  diff /tmp/before.txt /tmp/after.txt && echo "音乐目录未被修改 ✅"
  ```
- [ ] 日志文件不含完整文件路径
- [ ] 日志文件不含在线来源 URL 的查询串
- [ ] 日志文件不含凭据

### 7.5 容错

- [ ] 删除正在播放的文件 → 不崩溃，给出可读错误
- [ ] 拔出 USB 移动硬盘（资料库所在）→ 不崩溃
- [ ] 数据库文件被外部锁定 → 给出可读错误
- [ ] 数据库文件被替换为垃圾内容 → 检测到损坏，不崩溃
- [ ] 磁盘写满时扫描 → 优雅失败

---

## 8. E2E 测试（评估项，非承诺）

**目标：** 启动真实应用二进制，通过 WebDriver 驱动界面。

**技术路径：** `tauri-driver` + WebdriverIO。

**已知障碍：**

| 障碍 | 说明 |
|---|---|
| Linux 上 `tauri-driver` 依赖 **WebKitWebDriver** | 需要额外安装 |
| Wayland 支持有限 | 可能需要 Xvfb 提供虚拟 X 显示 |
| 音频设备 | CI 无声卡，需以 `NullAudioEngine` 启动（可通过环境变量切换） |

**决策：推迟到 M3 之后评估。** 理由：

1. M0–M2 的核心价值在 `domain/` 与 `app/` 层，已被单元/集成测试覆盖
2. E2E 的维护成本高、脆弱性强
3. 真正需要 E2E 的是"跨 UI 与后端的完整流程"（如：搜索 → 加入队列 → 播放 → 重启恢复），这类流程可以用**集成测试 + 前端组件测试**的组合覆盖大部分风险

> **若 M3 评估后仍不可行，明确记录为已知缺口**，并把"重启后状态恢复"等关键流程纳入人工清单。**不要为了"有 E2E"而写脆弱的 E2E。**

---

## 9. 反模式（禁止）

| 反模式 | 为什么禁止 | 正确做法 |
|---|---|---|
| `std::thread::sleep()` 在测试中 | 慢且不确定 | `FakeClock::advance()` |
| 测试访问真实网络 | CI 不稳定、依赖外部 | `wiremock` |
| 测试依赖真实声卡 | CI 无声卡 | `NullAudioEngine` |
| 测试依赖仓库固定目录结构 | 测试间耦合 | `TempDir` 自建 |
| 用 `assert!(result.is_ok())` 而不检查内容 | 掩盖真实行为 | 断言具体值或错误码 |
| 为通过测试而放宽断言 | 掩盖真实缺陷 | 先查代码 |
| 测试内部实现细节（如断言私有方法调用顺序） | 重构即碎 | 测试公开行为 |
| 大量快照测试（`insta`）覆盖逻辑 | 快照易被"无脑接受" | 快照只用于稳定的序列化格式 |
| 在 `domain/` 中 mock 掉被测逻辑本身 | 测试无意义 | 只 mock 端口（I/O 边界） |

### 关于快照测试的使用边界

`insta` 适合：错误码映射表、DTO 序列化格式、LRC 解析输出。

`insta` **不适合**：队列推进逻辑、搜索排序——这些必须用显式断言。

---

## 10. 测试与需求的对应

| 需求 | 测试层级 | 位置 |
|---|---|---|
| FR-1.6 增量扫描 | 集成 | `infra/scan/` |
| FR-1.10 单文件错误隔离 | 集成 | `infra/scan/` |
| FR-2.5 自动续播 | 集成（虚拟时钟） | `app/playback_service.rs` |
| FR-2.6 解码失败跳过 | 集成 | `app/playback_service.rs` |
| FR-2.8 线程隔离 | 手工 + 超时断言 | 人工清单 §7.1 |
| FR-3.5 随机置换 | 单元 + 属性测试 | `domain/queue.rs` |
| FR-6.1–6.2 历史 | 集成 | `infra/db/`, `app/` |
| FR-8.4 歌词多编码 | 单元 | `infra/lyrics/` |
| FR-9.6 URL 校验（P4） | 单元（拒绝矩阵） | `domain/stream.rs` |
| FR-11.1–11.4 MPRIS | **手工** | 人工清单 §7.2 |
| NFR-2.1 WAL | 集成 | `infra/db/` |
| NFR-2.3 schema 版本 | 集成 | `infra/db/` |
| NFR-4.5 日志脱敏 | 单元 | `infra/logging.rs` |
| NFR-1.x 性能 | 基准 + 手工 | 人工清单 §7.3 |

**注意 MPRIS 只能用人工验证** —— 这一点在 `roadmap.md` M2 退出标准中已明确要求使用 `playerctl` 这类**外部工具**验证，不接受自制测试自证。

---

## 11. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| 0.1.0 | 2026-10-15 | 初稿 |
