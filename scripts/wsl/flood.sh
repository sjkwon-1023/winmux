#!/usr/bin/env bash
# flow control / 대량 출력 부하 테스트 스크립트
# (계획 v2 3장 "부하"·"Flow control 검증", spike-plan 4.3 FlowControl 계약 검증용)
#
# `yes` 수준의 고속·반복 출력으로 FlowControl이 high water(기본 2MB)에서 Pause,
# low water(기본 512KB)로 내려오면 Resume하는지, stats 패널의 pending/paused가
# 기대대로 움직이는지, RAM이 폭주하지 않는지 확인하는 용도다.
#
# 사용법:
#   scripts/wsl/flood.sh [SECONDS] [--random-lines]
#
#   SECONDS         yes를 출력할 시간(초). 기본 10.
#   --random-lines  yes 출력이 끝난 뒤 base64(/dev/urandom) 대량 라인을 이어서
#                   출력한다. 반복 패턴(yes)이 아니라 고엔트로피 데이터에서도
#                   backpressure가 동일하게 버티는지 확인하는 용도.
#
# 예:
#   scripts/wsl/flood.sh                    # yes를 10초
#   scripts/wsl/flood.sh 30                 # yes를 30초
#   scripts/wsl/flood.sh 10 --random-lines  # yes 10초 + 랜덤 라인 대량 출력

set -euo pipefail

FLOOD_SECONDS="${1:-10}"
RANDOM_LINES=0
if [[ "${2:-}" == "--random-lines" ]]; then
  RANDOM_LINES=1
fi

echo "yes를 ${FLOOD_SECONDS}초 동안 출력한다 (Ctrl+C로 중단 가능)..."
# timeout으로 종료된 yes는 0이 아닌 종료 코드를 반환하므로 `|| true`로 흡수한다
# (그대로 두면 set -e 때문에 스크립트 전체가 죽는다).
timeout "${FLOOD_SECONDS}" yes || true

if [[ "$RANDOM_LINES" -eq 1 ]]; then
  echo "base64(/dev/urandom) 대량 라인 출력 시작..."
  # head가 300000줄을 다 받으면 먼저 종료해 파이프를 닫고, base64는 그 순간
  # SIGPIPE(exit 141)로 죽는다 — 의도된 정상 동작이다. 그런데 pipefail 하에서는
  # 파이프라인 전체의 종료 코드가 "오른쪽부터 훑어 처음 만나는 0이 아닌 상태"로
  # 정해지므로, head가 0으로 끝나도 base64의 141이 파이프라인 종료 코드로 채택되어
  # set -e가 스크립트를 여기서 중단시켜 버린다. `|| true`로 그 141을 흡수한다.
  base64 /dev/urandom | head -n 300000 || true
  echo "랜덤 라인 출력 완료."
fi

echo "flood 완료."
