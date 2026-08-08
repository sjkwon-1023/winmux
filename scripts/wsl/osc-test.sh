#!/usr/bin/env bash
# OSC passthrough 수동 검증 스크립트 (계획 v2 3장 "OSC passthrough 검증", spike-plan 4.1)
#
# ConPTY가 WSL 안에서 방출한 OSC 0/7/9/777 시퀀스를 wmux 앱(Rust PTY 리더)까지
# 그대로 통과시키는지 눈으로 확인하기 위한 스크립트다. 종결자는 BEL(\x07)과
# ST(ESC \) 두 가지를 모두 시험한다 — 구현에 따라 둘 중 하나만 인식하는 경우가 있다.
# 마지막 케이스는 시퀀스를 두 번의 write로 쪼개 청크 경계에 걸쳐도 감지되는지 확인한다.
#
# 사용법:
#   scripts/wsl/osc-test.sh
#
# wmux 앱을 실행한 상태에서 이 스크립트가 붙어 있는 바로 그 터미널(WSL bash)에서
# 실행하고, 앱의 OSC 이벤트 로그에 케이스 1~9가 모두 예상대로 찍히는지 대조한다.
# 이 문서/스크립트가 방출하는 OSC 시퀀스의 실제 형식(예: OSC 777)은
# `\033]777;notify;제목;본문\007` 이다.

set -euo pipefail

ESC=$'\033'
BEL=$'\007'
ST="${ESC}\\"
TTY=/dev/tty

case_header() {
  echo
  echo "케이스 $1: $2"
}

emit() {
  # emit <osc-body> <terminator>
  # osc-body는 "]" 다음부터 종결자 앞까지의 내용이다.
  printf '%s%s%s' "$ESC" "$1" "$2" > "$TTY"
}

# --- OSC 0: 아이콘 이름 + 창 제목 ---

case_header 1 "OSC 0 title, BEL 종결"
emit "]0;wmux-osc-test-case-1" "$BEL"

case_header 2 "OSC 0 title, ST 종결"
emit "]0;wmux-osc-test-case-2" "$ST"

# --- OSC 7: cwd (file:// URI) ---

HOST="$(hostname)"
CWD="$(pwd)"

case_header 3 "OSC 7 cwd, BEL 종결"
emit "]7;file://${HOST}${CWD}" "$BEL"

case_header 4 "OSC 7 cwd, ST 종결"
emit "]7;file://${HOST}${CWD}" "$ST"

# --- OSC 9: 알림 (iTerm2 계열, message만) ---

case_header 5 "OSC 9 notify, BEL 종결"
emit "]9;wmux-osc-test-case-5" "$BEL"

case_header 6 "OSC 9 notify, ST 종결"
emit "]9;wmux-osc-test-case-6" "$ST"

# --- OSC 777: 알림 (urxvt 계열, notify;title;body) ---
# 형식: \033]777;notify;제목;본문\007  (BEL) 또는 \033]777;notify;제목;본문\033\  (ST)

case_header 7 "OSC 777 notify, BEL 종결"
emit "]777;notify;wmux-osc-test-case-7;OSC 777 BEL body" "$BEL"

case_header 8 "OSC 777 notify, ST 종결"
emit "]777;notify;wmux-osc-test-case-8;OSC 777 ST body" "$ST"

# --- 분할 테스트: 한 시퀀스를 두 번의 write로 쪼개 청크 경계 처리 확인 ---
# PTY 리더가 feed()를 여러 번 호출받아도(청크 경계에 걸려도) 시퀀스를 이어 붙여
# 인식하는지 확인한다. 두 write 사이에 0.2초를 둔다.

case_header 9 "OSC 777, 두 번의 write로 분할 (0.2s 간격)"
printf '%s]777;notify;wmux-osc-test-case-9;split-' "$ESC" > "$TTY"
sleep 0.2
printf 'payload%s' "$BEL" > "$TTY"

echo
echo "완료. wmux 앱의 OSC 이벤트 로그에서 케이스 1~9가 모두 보이는지 확인하라."
echo "케이스 9(분할)가 감지되지 않으면 OscScanner의 청크 경계 처리를 재점검한다."
