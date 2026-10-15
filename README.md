# Aurora Music

一个独立实现的 Linux 桌面音乐播放器。本地优先，不依赖任何在线服务。

- **桌面**：Arch Linux / KDE Plasma 6 / Wayland（开发环境；理论上兼容其它 GTK 桌面）
- **技术栈**：Tauri 2 + Rust + React 19 + TypeScript + SQLite
- **音乐来源**：仅本地文件。**不绕过 DRM、付费墙或访问控制**，不抓取违反服务条款的内容

## 功能现状

**已实现**

| 模块 | 内容 |
|---|---|
| 本地资料库 | 递归扫描目录，读取 MP3 / FLAC / WAV 元数据，SQLite 持久化，FTS5 全文搜索（支持中文分词与去音标） |
| 播放 | 播放 / 暂停 / 停止 / 上一首 / 下一首 / 跳转 / 音量；队列管理；无损与有损格式均由 symphonia 解码 |
| 播放模式 | 随机播放（固定种子置换），循环（顺序 / 列表 / 单曲） |
| 播放界面 | 主内容区可从播放栏展开为完整播放视图，显示技术元数据 |
| 容错 | 损坏文件被检测并跳过而不中断队列；无声卡时降级为静默引擎而非崩溃 |

**尚未实现**：封面、歌词、歌单、收藏、播放历史、MPRIS 桌面集成、队列持久化、在线来源适配器。

路线图见 [`docs/roadmap.md`](docs/roadmap.md)，需求与架构见 [`docs/requirements.md`](docs/requirements.md) 与 [`docs/architecture.md`](docs/architecture.md)。

## 安装

### 从 AUR

```bash
# 使用你惯用的 AUR 助手
paru -S aurora-music
```

### 从源码构建

依赖：

```bash
sudo pacman -S --needed base-devel rust nodejs npm pkgconf \
  webkit2gtk-4.1 gtk3 libsoup3 alsa-lib
```

构建与运行：

```bash
git clone https://github.com/johnsmith8736/aurora-music.git
cd aurora-music
npm ci
npm run tauri dev          # 开发模式
npm run tauri build        # 产出安装包
```

> ⚠️ **注意**：`cargo build --release` **不会**产出可独立运行的二进制。
> Tauri 依据 `custom-protocol` 特性（而非 debug/release profile）决定是内嵌前端资源
> 还是去加载开发服务器地址。必须经由 `tauri build`，否则程序启动后会尝试连接
> `localhost:1420` 并失败。

## 测试

```bash
cd src-tauri && cargo test     # 134 个测试
cargo clippy --all-targets     # 零告警
npm run build                  # 前端类型检查与构建
```

## 许可证

Copyright (C) 2026 Stanley Chan

本程序是自由软件：你可以依据自由软件基金会发布的 GNU 通用公共许可证
（第 3 版）条款重新分发和/或修改它。

本程序的分发是希望它有用，但**不提供任何担保**，甚至不提供适销性或
特定用途适用性的默示担保。详见 GNU 通用公共许可证。

完整条款见 [`LICENSE`](LICENSE)。
