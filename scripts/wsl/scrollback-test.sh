#!/usr/bin/env bash
# scrollback 상한 확인용 — 기본 scrollback 5000줄을 초과하는 12000줄을 출력한다.
# (계획 v2 12장 "scrollback 제한: 기본 5,000줄", spike-plan 4.6 "scrollback 5000 고정")
#
# 사용법:
#   scripts/wsl/scrollback-test.sh
#
# 출력이 끝난 뒤 터미널을 맨 위로 스크롤한다. scrollback이 5000줄로 제대로
# 잘리고 있다면 1번 줄부터 7000번 줄까지는 이미 밀려나가 보이지 않고, 7001번
# 줄 부근부터 보여야 한다. 1번 줄이 그대로 보인다면 scrollback 제한이 적용되지
# 않은 것이다.

set -euo pipefail

echo "12000줄 출력 시작 (scrollback 5000 초과 확인용)..."
seq 1 12000
echo "12000줄 출력 완료. 맨 위로 스크롤해 1~7000번대 줄이 잘렸는지 확인하라."
