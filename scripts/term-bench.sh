#!/usr/bin/env bash
# Terminal rendering benchmark — run in both xterm.js and ghostty-web tabs to compare.
# Pre-generates all data in tmpfiles, then cats in one shot to measure renderer, not bash.

set -e

COLS=$(tput cols)
ROWS=$(tput lines)
TMP=$(mktemp -d)
trap "rm -rf $TMP" EXIT

declare -a LABELS TIMES EXTRAS

# ── Helper: time a single cat, store result in BENCH_MS ──
BENCH_MS=0
bench() {
  local file="$1"
  local start end
  start=$(date +%s%N)
  cat "$file"
  end=$(date +%s%N)
  BENCH_MS=$(( (end - start) / 1000000 ))
}

safe_div() {
  local num="$1" den="$2"
  echo $(( num / (den > 0 ? den : 1) ))
}

echo -e "\033[1;35mPre-generating test data...\033[0m"

# ── 1. Raw Throughput (10MB) ──
echo -n "  [1/5] Raw throughput data..."
head -c 10000000 /dev/urandom | base64 > "$TMP/1.txt"
SIZE=$(wc -c < "$TMP/1.txt" | tr -d ' ')
echo " done (${SIZE} bytes)"

# ── 2. 256-Color Grid x50 ──
echo -n "  [2/5] Color grid data..."
{
  for rep in $(seq 1 50); do
    for i in $(seq 0 255); do
      printf "\033[48;5;%dm  " "$i"
      (( (i + 1) % 32 == 0 )) && printf "\033[0m\n"
    done
    printf "\033[0m"
  done
} > "$TMP/2.txt"
echo " done"

# ── 3. Full-Screen Animation (200 frames) ──
echo -n "  [3/5] Animation frames..."
FRAMES=200
CHARS=("█" "▓" "▒" "░" " ")
{
  for frame in $(seq 1 $FRAMES); do
    printf "\033[H"
    ci=$(( frame % 5 ))
    color=$(( (frame * 3) % 256 ))
    for row in $(seq 1 $ROWS); do
      printf "\033[38;5;%dm" "$color"
      printf '%0.s'"${CHARS[$ci]}" $(seq 1 $COLS)
    done
  done
  printf "\033[0m\033[H\033[2J"
} > "$TMP/3.txt"
echo " done (${FRAMES}f @ ${COLS}x${ROWS})"

# ── 4. Scrolling Stress (10k lines) ──
echo -n "  [4/5] Scroll data..."
{
  for i in $(seq 1 10000); do
    printf "\033[38;5;%dm%05d The quick brown fox jumps over the lazy dog — Lorem ipsum dolor sit amet\033[0m\n" "$(( i % 256 ))" "$i"
  done
} > "$TMP/4.txt"
echo " done"

# ── 5. Unicode/Emoji Stress (2k lines) ──
echo -n "  [5/5] Unicode data..."
{
  for i in $(seq 1 2000); do
    echo "│ 你好世界 🚀🔥💻 Ñoño café → ∑∏∫ ██▓▒░ ├──┤ ¡Hola! Ελληνικά العربية हिन्दी │"
  done
} > "$TMP/5.txt"
echo " done"

echo ""
echo -e "\033[1;35mRunning benchmarks...\033[0m"
echo ""
sleep 0.5

# ── Run ──

echo -e "\033[1;36m=== 1. Raw Throughput (10MB) ===\033[0m"
bench "$TMP/1.txt"
T1=$BENCH_MS
MB_S=$(safe_div $(( SIZE / 1024 * 1000 )) $T1)
MB_S=$(( MB_S / 1024 ))
echo -e "\n\033[1;32m→ ${T1}ms (~${MB_S} MB/s)\033[0m"
LABELS+=("Raw Throughput (10MB)")
TIMES+=("$T1")
EXTRAS+=("~${MB_S} MB/s")
sleep 0.3

echo -e "\n\033[1;36m=== 2. 256-Color Grid (x50) ===\033[0m"
bench "$TMP/2.txt"
T2=$BENCH_MS
echo -e "\033[1;32m→ ${T2}ms\033[0m"
LABELS+=("256-Color Grid (x50)")
TIMES+=("$T2")
EXTRAS+=("")
sleep 0.3

echo -e "\n\033[1;36m=== 3. Full-Screen Animation (${FRAMES}f) ===\033[0m"
bench "$TMP/3.txt"
T3=$BENCH_MS
FPS=$(safe_div $(( FRAMES * 1000 )) $T3)
echo -e "\033[1;32m→ ${T3}ms (~${FPS} fps)\033[0m"
LABELS+=("Animation (${FRAMES}f ${COLS}x${ROWS})")
TIMES+=("$T3")
EXTRAS+=("~${FPS} fps")
sleep 0.3

echo -e "\n\033[1;36m=== 4. Scrolling Stress (10k lines) ===\033[0m"
bench "$TMP/4.txt"
T4=$BENCH_MS
LPS=$(safe_div 10000000 $T4)
echo -e "\033[1;32m→ ${T4}ms (~${LPS} lines/s)\033[0m"
LABELS+=("Scrolling (10k lines)")
TIMES+=("$T4")
EXTRAS+=("~${LPS} lines/s")
sleep 0.3

echo -e "\n\033[1;36m=== 5. Unicode & Emoji (2k lines) ===\033[0m"
bench "$TMP/5.txt"
T5=$BENCH_MS
echo -e "\033[1;32m→ ${T5}ms\033[0m"
LABELS+=("Unicode/Emoji (2k lines)")
TIMES+=("$T5")
EXTRAS+=("")

# ══════════════════════════════════════════════════════════════════
# Summary
# ══════════════════════════════════════════════════════════════════
TOTAL=$(( T1 + T2 + T3 + T4 + T5 ))

W=62  # box inner width

echo ""
echo ""
printf "\033[1;35m╔"
printf '═%.0s' $(seq 1 $W)
printf "╗\n"
printf "║\033[1;37m%-${W}s\033[1;35m║\n" "  TERMINAL BENCHMARK RESULTS"
printf "╠"
printf '═%.0s' $(seq 1 $W)
printf "╣\033[0m\n"

for idx in "${!LABELS[@]}"; do
  label="${LABELS[$idx]}"
  time="${TIMES[$idx]}"
  extra="${EXTRAS[$idx]}"
  if [[ -n "$extra" ]]; then
    line=$(printf "  %-34s %7s ms   %s" "$label" "$time" "$extra")
  else
    line=$(printf "  %-34s %7s ms" "$label" "$time")
  fi
  printf "\033[1;35m║\033[0m%-${W}s\033[1;35m║\033[0m\n" "$line"
done

printf "\033[1;35m╠"
printf '═%.0s' $(seq 1 $W)
printf "╣\033[0m\n"

total_line=$(printf "  %-34s %7s ms" "TOTAL" "$TOTAL")
printf "\033[1;35m║\033[1;32m%-${W}s\033[1;35m║\033[0m\n" "$total_line"

size_line=$(printf "  Terminal: %dx%d" "$COLS" "$ROWS")
printf "\033[1;35m║\033[0m%-${W}s\033[1;35m║\033[0m\n" "$size_line"

printf "\033[1;35m╚"
printf '═%.0s' $(seq 1 $W)
printf "╝\033[0m\n"
