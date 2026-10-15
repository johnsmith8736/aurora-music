#!/usr/bin/env bash
# 重新生成音频测试夹具。
#
# 所有音频均为 ffmpeg 合成的正弦波，不含任何第三方版权内容。
# 生成的产物已提交到仓库，因此 CI 不需要安装 ffmpeg。
#
# 用法: bash tests/fixtures/gen-fixtures.sh
set -euo pipefail

cd "$(dirname "$0")"
OUT="audio"
mkdir -p "$OUT"
rm -f "$OUT"/*.mp3 "$OUT"/*.flac "$OUT"/*.wav

# 短小单声道正弦波：1 秒 / 8000 Hz，体积可控
TONE="sine=frequency=440:duration=1:sample_rate=8000"

# ── 带完整标签的 MP3 ─────────────────────────────────────────
ffmpeg -hide_banner -loglevel error -f lavfi -i "$TONE" -ac 1 \
  -metadata title="Fixture MP3" \
  -metadata artist="Fixture Artist" \
  -metadata album="Fixture Album" \
  -metadata album_artist="Fixture Album Artist" \
  -metadata track="3" \
  -metadata date="2026" \
  -y "$OUT/tagged.mp3"

# ── 带完整标签的 FLAC ────────────────────────────────────────
ffmpeg -hide_banner -loglevel error -f lavfi -i "$TONE" -ac 1 \
  -metadata title="Fixture FLAC" \
  -metadata artist="Fixture Artist" \
  -metadata album="Fixture Album" \
  -metadata track="1" \
  -metadata date="2026" \
  -y "$OUT/tagged.flac"

# ── 无标签 WAV（验证文件名回退）──────────────────────────────
ffmpeg -hide_banner -loglevel error -f lavfi -i "$TONE" -ac 1 \
  -y "$OUT/07 - Untagged Track.wav"

# ── 损坏样本：合法 FLAC 截断 50%（头部完好，音频数据缺失）────
head -c $(( $(stat -c%s "$OUT/tagged.flac") / 2 )) "$OUT/tagged.flac" \
  > "$OUT/truncated.flac"

# ── 伪装样本：纯文本改名为 .mp3 ──────────────────────────────
printf 'this is definitely not audio' > "$OUT/not_audio.mp3"

# ── 非音频文件（应被完全忽略）────────────────────────────────
printf 'readme' > "$OUT/notes.txt"

echo "夹具已生成到 $OUT/"
ls -la "$OUT"
