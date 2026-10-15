/**
 * 类型级测试 —— 用编译器验证安全约束。
 *
 * # 为什么不需要测试框架
 *
 * TypeScript 的 `@ts-expect-error` 注释由 `tsc` 本身校验：
 *
 * - 若**下一行确实有类型错误** → 通过（错误被"期望"掉了）
 * - 若**下一行没有类型错误** → **编译失败**（"未使用的 @ts-expect-error"）
 *
 * 因此 `npx tsc --noEmit` 就足以执行这些断言，
 * 无需引入 vitest / jest 等依赖（本项目当前也没有前端测试框架）。
 *
 * # 这些测试在保护什么
 *
 * 核心安全属性：**元数据不能充当播放授权**。
 * 如果哪天有人放宽了 `Track` / `ResolvedPlayback` 的定义，
 * 让两者可以互相赋值，这些测试会立刻失败。
 *
 * 具体论证见 `docs/music-sources/interface-design.md` §5。
 */

import type {
  MusicSourceError,
  PlaybackCapability,
  ResolvedPlayback,
  SearchHit,
  Track,
} from "./types";
import { createResolvedPlayback, isPlaybackExpired } from "./types";

// 测试用样本，由调用方提供（此处只关心类型，不关心运行期值）
declare const aTrack: Track;
declare const aResolved: ResolvedPlayback;

// ══════════════════════════════════════════════════════════════
// 防线一：元数据不能充当播放凭证
// ══════════════════════════════════════════════════════════════

/**
 * `Track` 缺少 `capability` 与品牌字段，**必须**无法赋给 `ResolvedPlayback`。
 *
 * 这条断言是"元数据 ≠ 播放授权"在类型层面的唯一保障。
 */
// @ts-expect-error Track 不具备播放凭证所需的品牌字段与 capability
export const metadataIsNotAGrant: ResolvedPlayback = aTrack;

// ══════════════════════════════════════════════════════════════
// 防线二：凭证不能凭空构造
// ══════════════════════════════════════════════════════════════

/**
 * `PlaybackGrantBrand` 是模块私有的 `unique symbol`，
 * 因此外部对象字面量**无法**满足 `ResolvedPlayback`。
 */
// @ts-expect-error 品牌字段不可从模块外提供，对象字面量缺少该属性
export const handRolledGrant: ResolvedPlayback = {
  sourceId: "navidrome:x",
  sourceTrackId: "42",
  capability: { kind: "unsupported", reason: "unknown", message: "x" },
  resolvedAt: 0,
  track: aTrack,
};

/** 唯一合法构造路径。 */
export const legitimateGrant: ResolvedPlayback = createResolvedPlayback({
  sourceId: "navidrome:x",
  sourceTrackId: "42",
  capability: { kind: "unsupported", reason: "unknown", message: "x" },
  resolvedAt: 0,
  track: aTrack,
});

// ══════════════════════════════════════════════════════════════
// 防线三：正向约束（必须能编译）
// ══════════════════════════════════════════════════════════════

/**
 * **搜索结果允许曲目不可播放** —— 这是合法状态，不是错误。
 *
 * 若这条断言有一天失败，说明 `PlaybackCapability` 被改成了
 * "必须有 URL" 的形状，那会破坏 `interface-design.md` §4.1 的设计。
 */
export const searchHitMayBeUnplayable: SearchHit = {
  track: aTrack,
  playback: {
    kind: "unsupported",
    reason: "not-entitled",
    message: "当前账号无权播放该曲目。",
  },
};

/** 本地文件能力：携带路径。 */
export const localFileCapability: PlaybackCapability = {
  kind: "local-file",
  path: "/music/a.flac",
};

/** 直接 URL：必须显式声明是否有时效信息。 */
export const directUrlCapability: PlaybackCapability = {
  kind: "direct-url",
  url: "https://example.invalid/stream",
  mimeType: "audio/flac",
  expiresAt: null, // 来源未声明过期 —— 不代表永久有效
  contentLength: 1024,
};

/** 外部播放器移交：保留此形态，避免"所有来源都必须给 URL"的错误假设。 */
export const externalPlayerCapability: PlaybackCapability = {
  kind: "external-player",
  uri: "someapp://track/42",
  provider: "SomeApp",
};

// ══════════════════════════════════════════════════════════════
// 防线四：判别联合的穷尽性
// ══════════════════════════════════════════════════════════════

/**
 * 若将来给 `PlaybackCapability` 增加分支，此函数会因
 * "函数缺少结尾 return" 而编译失败 —— 提醒调用方处理新分支。
 */
export function describeCapability(cap: PlaybackCapability): string {
  switch (cap.kind) {
    case "local-file":
      return "本地文件";
    case "direct-url":
      return cap.expiresAt === null ? "在线流（未声明时效）" : "在线流（有时效）";
    case "external-player":
      return `由 ${cap.provider} 播放`;
    case "unsupported":
      return `不可播放：${cap.message}`;
  }
}

/** 错误类型同样是判别联合，穷尽检查保护错误映射表。 */
export function describeError(err: MusicSourceError): string {
  switch (err.kind) {
    case "not-configured":
      return "未配置";
    case "disabled":
      return "已禁用";
    case "auth-required":
      return "需要登录";
    case "auth-failed":
      return "认证失败";
    case "network":
      return "网络错误";
    case "timeout":
      return "超时";
    case "cancelled":
      return "已取消";
    case "rate-limited":
      return "限流";
    case "not-found":
      return "不存在";
    case "forbidden":
      return "无权限";
    case "server-error":
      return "服务端错误";
    case "invalid-response":
      return "响应格式错误";
    case "stream-rejected":
      return "播放地址被拒绝";
    case "not-implemented":
      return "尚未实现";
  }
}

// ══════════════════════════════════════════════════════════════
// 防线五：过期判定的语义
// ══════════════════════════════════════════════════════════════

/**
 * `expiresAt === null` **不得**被判为过期。
 *
 * 注意这只是类型/逻辑约束的运行期表达；实际断言在
 * `docs/music-sources/testing-plan.md` §4.3 中规划。
 */
export function nullExpiryIsNotExpired(grant: ResolvedPlayback): boolean {
  const expired = isPlaybackExpired(grant, Date.now());
  if (grant.capability.kind !== "direct-url") return true;
  if (grant.capability.expiresAt === null) {
    return !expired; // 未声明时效 ⇒ 不得判为过期
  }
  return true;
}

/** 本地文件凭证永远不过期（它没有时效概念）。 */
export function localGrantNeverExpires(grant: ResolvedPlayback): boolean {
  if (grant.capability.kind !== "local-file") return true;
  return !isPlaybackExpired(grant, Number.MAX_SAFE_INTEGER);
}

/** 保留引用，避免 `noUnusedLocals` 报错。 */
export const __typeTestRefs = { aResolved, legitimateGrant };
