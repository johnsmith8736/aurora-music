#!/usr/bin/env bash
#
# 为开发模式安装桌面条目与图标，使 KDE / GNOME 能正确显示应用图标与名称。
#
# ── 为什么需要这一步 ────────────────────────────────────────────────
#
# Wayland 的 xdg-shell 协议没有设置窗口图标的机制，因此 Tauri 传给窗口的
# 图标在 Wayland 下完全不生效（tao 源码 window.rs 明确标注
# "Linux(Wayland) / iOS / Android: Unsupported"）。
#
# 合成器改而使用这条链路查图标：
#
#     窗口 app_id  →  <app_id>.desktop  →  Icon= 字段  →  图标主题
#
# 实测（WAYLAND_DEBUG=1，观察 xdg_toplevel.set_app_id）：
#
#     xdg_toplevel.set_app_id("aurora-music")
#
# 即 app_id 就是**可执行文件名**。
#
# ── 重要：不要打开 enableGTKAppId ───────────────────────────────────
#
# tauri.conf.json 有一个 app.enableGTKAppId 选项，文档称它能让桌面环境
# 关联到同名 .desktop。但在本项目实测无效且有害：
#
#   1. 对 Wayland app_id 没有影响 —— 开与不开都是 "aurora-music"
#      （GDK 取的是进程名，不是 GApplication id；GApplication id 只在
#       会话总线上可见，合成器不看那个）
#   2. 打开后 tao 用 gio::ApplicationFlags::empty() 注册 GApplication，
#      没有 NON_UNIQUE 标志，导致应用变为单实例
#   3. 更糟的是第二个实例不会静默退出，而是向第一个实例发 activate 信号，
#      使 Tauri 的 setup 二次执行并 panic：
#
#         Failed to setup app: a webview with label `main` already exists
#         (tauri/src/app.rs:1444)
#
# 也就是说：开着它，重复 `npm run tauri dev` 会把正在运行的实例搞崩。
# 结论是保持默认的 false，改用 .desktop 文件名对齐真实 app_id 来拿到图标。
#
# ── 打包后不需要本脚本 ──────────────────────────────────────────────
#
# `npm run tauri build` 产出的 .deb / .rpm 会由 tauri CLI 自动生成
# .desktop 与 hicolor 图标。注意届时会同时存在系统级与用户级两个条目，
# 用户级的优先，此时应运行本脚本 --uninstall 以免 dev 条目遮蔽正式条目。
#
# 用法：
#   scripts/install-dev-desktop.sh              安装
#   scripts/install-dev-desktop.sh --uninstall  卸载
#   scripts/install-dev-desktop.sh --status     查看当前状态
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICON_SRC="$REPO_ROOT/src-tauri/icons"
BIN="$REPO_ROOT/src-tauri/target/debug/aurora-music"

# ⚠️ 必须等于**可执行文件名**，即 Wayland app_id。
#    不是 tauri.conf.json 里的 identifier（那个 com.arch.aurora-music
#    只在会话总线上出现，合成器不读它）。详见文件头的实测记录。
APP_ID="aurora-music"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor"
DESKTOP_FILE="$APPS_DIR/$APP_ID.desktop"

# 上一版脚本用 identifier 命名，若存在则一并清理
LEGACY_APP_ID="com.arch.aurora-music"

# hicolor 尺寸 <- 源文件 的映射
ICON_MAP=(
  "32:32x32.png"
  "64:64x64.png"
  "128:128x128.png"
  "256:128x128@2x.png"
  "512:icon.png"
)

die() { printf '错误：%s\n' "$1" >&2; exit 1; }
info() { printf '  %s\n' "$1"; }

check_sources() {
  [ -d "$ICON_SRC" ] || die "找不到图标目录：$ICON_SRC"
  for pair in "${ICON_MAP[@]}"; do
    local src="$ICON_SRC/${pair#*:}"
    [ -f "$src" ] || die "缺少图标源文件：$src"
  done
}

icon_path() { printf '%s/%sx%s/apps/%s.png' "$ICONS_DIR" "$1" "$1" "$APP_ID"; }

do_status() {
  printf 'Wayland app_id：%s\n' "$APP_ID"
  printf '（= 可执行文件名；用 WAYLAND_DEBUG=1 观察 xdg_toplevel.set_app_id 得到）\n\n'

  printf '桌面条目：'
  if [ -f "$DESKTOP_FILE" ]; then
    printf '已安装  %s\n' "$DESKTOP_FILE"
  else
    printf '未安装\n'
  fi

  printf '图标：\n'
  local n=0
  for pair in "${ICON_MAP[@]}"; do
    local size="${pair%%:*}"
    if [ -f "$(icon_path "$size")" ]; then printf '  %-4s ✅\n' "$size"; n=$((n + 1))
    else printf '  %-4s ❌\n' "$size"; fi
  done
  printf '\n已安装 %d/%d 个尺寸\n' "$n" "${#ICON_MAP[@]}"

  printf '\n遗留的 identifier 命名条目：'
  if [ -f "$APPS_DIR/$LEGACY_APP_ID.desktop" ]; then
    printf '仍存在 ⚠️（重跑本脚本可清理）\n'
  else
    printf '无 ✅\n'
  fi

  printf '\nenableGTKAppId：'
  if grep -q '"enableGTKAppId"[[:space:]]*:[[:space:]]*true' \
       "$REPO_ROOT/src-tauri/tauri.conf.json" 2>/dev/null; then
    printf 'true ⚠️ 会造成单实例并使重复启动崩溃，建议移除该项\n'
  else
    printf '未启用 ✅\n'
  fi
}

remove_legacy() {
  local found=0
  if [ -f "$APPS_DIR/$LEGACY_APP_ID.desktop" ]; then
    rm -f "$APPS_DIR/$LEGACY_APP_ID.desktop"
    info "已清理遗留桌面条目 $LEGACY_APP_ID.desktop"
    found=1
  fi
  for pair in "${ICON_MAP[@]}"; do
    local size="${pair%%:*}"
    local p="$ICONS_DIR/${size}x${size}/apps/$LEGACY_APP_ID.png"
    if [ -f "$p" ]; then
      rm -f "$p"
      info "已清理遗留图标 ${size}x${size}/$LEGACY_APP_ID.png"
      found=1
    fi
  done
  return 0
}

do_install() {
  check_sources

  if [ ! -x "$BIN" ]; then
    printf '提示：尚未构建 %s\n' "$BIN"
    printf '      桌面条目会先指向该路径，构建后即可用。\n\n'
  fi

  mkdir -p "$APPS_DIR"
  remove_legacy

  # ── 图标 ──────────────────────────────────────────────
  printf '安装图标到 %s\n' "$ICONS_DIR"
  for pair in "${ICON_MAP[@]}"; do
    local size="${pair%%:*}" name="${pair#*:}"
    local dir="$ICONS_DIR/${size}x${size}/apps"
    mkdir -p "$dir"
    install -m 0644 "$ICON_SRC/$name" "$dir/$APP_ID.png"
    info "${size}x${size}  <-  $name"
  done

  # ── 桌面条目 ──────────────────────────────────────────
  # 文件名必须与 app_id 一致，合成器才能关联上
  printf '\n写入桌面条目 %s\n' "$DESKTOP_FILE"
  cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=Aurora Music
GenericName=Music Player
Comment=本地音乐播放器
Exec="$BIN"
Icon=$APP_ID
Terminal=false
Categories=AudioVideo;Audio;Player;
StartupNotify=true
StartupWMClass=$APP_ID
EOF
  info "Icon=$APP_ID"
  info "Exec=\"$BIN\""

  # ── 校验与刷新缓存 ────────────────────────────────────
  if command -v desktop-file-validate >/dev/null 2>&1; then
    printf '\n校验桌面条目：'
    if desktop-file-validate "$DESKTOP_FILE"; then
      printf '通过 ✅\n'
    else
      die "desktop-file-validate 报告了问题，见上方输出"
    fi
  else
    printf '\n未安装 desktop-file-validate，跳过语法校验\n'
  fi

  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" 2>/dev/null \
      && printf '已刷新桌面数据库 ✅\n'
  fi

  printf '\n完成。重新运行 npm run tauri dev 查看效果。\n'
}

do_uninstall() {
  printf '卸载 %s 的桌面条目与图标\n' "$APP_ID"

  if [ -f "$DESKTOP_FILE" ]; then
    rm -f "$DESKTOP_FILE"; info "已删除 $DESKTOP_FILE"
  else
    info "桌面条目本就不存在"
  fi

  local n=0
  for pair in "${ICON_MAP[@]}"; do
    local size="${pair%%:*}" p
    p="$(icon_path "$size")"
    if [ -f "$p" ]; then
      rm -f "$p"; info "已删除 ${size}x${size} 图标"; n=$((n + 1))
    fi
  done
  [ "$n" -eq 0 ] && info "图标本就不存在"

  remove_legacy

  command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$APPS_DIR" 2>/dev/null || true

  printf '\n完成。\n'
}

case "${1:-}" in
  "")           do_install ;;
  --uninstall)  do_uninstall ;;
  --status)     do_status ;;
  -h|--help)    sed -n '2,50p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' ;;
  *)            die "未知参数：$1（可用：--uninstall / --status / --help）" ;;
esac
