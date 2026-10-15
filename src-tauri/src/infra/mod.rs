//! 基础设施层：这是**唯一**触碰外部世界的地方
//! （文件系统、SQLite、音频设备、网络、系统时钟）。
//!
//! 每个子模块都实现 `domain` 层定义的端口，或提供 `app` 层可编排的
//! 能力。业务逻辑不写在这里。

pub mod audio;
pub mod db;
pub mod scan;
pub mod time;
