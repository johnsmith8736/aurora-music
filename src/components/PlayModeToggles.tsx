import type { RepeatMode } from "../ipc/types";
import type { PlayerController } from "../state/player";

/**
 * 播放模式开关：随机与循环。
 *
 * 播放栏与播放界面共用（两处各放一份，但实现只有这一处）——
 * 模式状态必须永远一致，各自实现必然漂移。
 * 状态的**唯一真相源是 Rust**：这里只读快照、只发命令，不保存本地 state。
 *
 * # 图标为什么不用 🔀 / 🔁 / 🔂
 *
 * 那三个码位（U+1F500 / U+1F501 / U+1F502）在本机**只被 Noto Color Emoji
 * 覆盖，没有任何单色字体**（`fc-list :charset=1f500` 的非 emoji 命中数为 0）。
 * 因此它们会渲染成彩色 emoji，与界面上其它单色控制字符（⏮ ⏸ ▶ ⏭ ⏹）
 * 冲突，而且加 U+FE0E 也救不回来 —— 变异选择符无法召唤不存在的单色字形。
 *
 * 改用只被单色字体覆盖的 `⇄`(U+21C4) / `⟳`(U+27F3) / `⟳¹`，见下方常量。
 */

const REPEAT_GLYPH: Record<RepeatMode, string> = {
  off: "⟳",
  all: "⟳",
  // 上标 ¹ 让"单曲"与"列表"在**字形**上就区分开，
  // 而不是只靠颜色 —— NFR-5.4 不允许颜色单独传达状态
  one: "⟳¹",
};

const REPEAT_NAME: Record<RepeatMode, string> = {
  off: "顺序播放",
  all: "列表循环",
  one: "单曲循环",
};

/**
 * 点击后的下一个模式：顺序 → 列表循环 → 单曲循环 → 顺序。
 *
 * 写成穷尽 switch 而非数组取模：一是不受 `noUncheckedIndexedAccess`
 * 影响（数组索引会多出 `undefined`），二是将来给 `RepeatMode`
 * 增加分支时这里会**编译失败**，而不是静默漏掉一个模式。
 */
function nextMode(current: RepeatMode): RepeatMode {
  switch (current) {
    case "off":
      return "all";
    case "all":
      return "one";
    case "one":
      return "off";
  }
}

export function ShuffleToggle({ player }: { player: PlayerController }) {
  const on = player.snapshot?.shuffle ?? false;

  return (
    <button
      className="ctrl ctrl-toggle"
      onClick={() => void player.setShuffle(!on)}
      aria-pressed={on}
      aria-label={`随机播放：${on ? "已开启" : "已关闭"}`}
      title={
        on ? "随机播放已开启，点击关闭" : "随机播放已关闭，点击开启"
      }
    >
      ⇄
    </button>
  );
}

export function RepeatToggle({ player }: { player: PlayerController }) {
  const mode = player.snapshot?.repeat ?? "off";
  const next = nextMode(mode);

  return (
    <button
      className="ctrl ctrl-toggle"
      onClick={() => void player.setRepeat(next)}
      // 关闭态与列表循环态共用同一个字形，因此 aria-pressed 是
      // 这两者唯一的语义区分 —— 必须给，不能只靠颜色
      aria-pressed={mode !== "off"}
      aria-label={`循环模式：${REPEAT_NAME[mode]}`}
      title={`循环模式：${REPEAT_NAME[mode]}，点击切换为${REPEAT_NAME[next]}`}
    >
      {REPEAT_GLYPH[mode]}
    </button>
  );
}
