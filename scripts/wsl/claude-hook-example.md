# Claude Code hook → OSC 777 알림 예시

계획 v2 9장 "에이전트 상태 및 알림"의 경로를 Claude Code hook으로 구현하는 예시다.

```
Claude Code hook (Stop / Notification)
  → 현재 TTY에 OSC 777 출력
  → Rust PTY 리더(wmux-core::osc::OscScanner)가 감지
  → 해당 탭/pane/워크스페이스 상태 갱신
```

v1은 IPC 서버·Named Pipe·Windows helper CLI를 쓰지 않고 PTY 출력 자체(OSC 시퀀스)를
전달 경로로 쓴다. 이 문서는 그 hook 쪽 절반 — Claude Code 설정과 OSC를 방출하는
스크립트 예시 — 을 보여준다.

## 왜 `> /dev/tty` 리다이렉트가 필요한가

Claude Code hook의 **stdout은 Claude Code 자신이 소비한다** (hook 실행 결과로 처리되어
UI/로그에 쓰이거나 버려지며, 그대로 터미널 화면에 바이트가 흘러가지 않는다). hook 스크립트가
그냥 `echo`나 `printf`로 OSC 시퀀스를 표준출력에 쓰면 그 바이트는 실제 PTY 스트림에
도달하지 못하고, Rust PTY 리더는 아무것도 감지하지 못한다.

따라서 hook 스크립트는 OSC 시퀀스를 **표준출력이 아니라 `/dev/tty`에 직접** 써야 한다.
`/dev/tty`는 그 프로세스가 붙어 있는 실제 터미널 장치 파일이므로, Claude Code의 stdout
캡처를 우회해 PTY 스트림에 바로 실려 wmux의 Rust PTY 리더까지 도달한다.

## hook 스크립트 예시

`~/.claude/hooks/osc777-notify.sh` (실행 권한 부여 필요: `chmod +x`):

```bash
#!/usr/bin/env bash
# Claude Code hook에서 호출되어 OSC 777로 알림을 /dev/tty에 방출한다.
# 인자: $1 = 제목(title). Notification 이벤트의 실제 메시지는 stdin의 JSON에서 읽는다.
set -euo pipefail

TITLE="${1:-Claude Code}"

# Claude Code는 hook 실행 시 이벤트 정보를 stdin으로 JSON을 전달한다.
# Notification 이벤트는 .message 필드에 사람이 읽을 알림 문구가 들어 있다.
# jq가 없거나 필드가 없으면 기본 문구로 대체한다.
INPUT_JSON="$(cat)"
BODY="$(printf '%s' "$INPUT_JSON" | jq -r '.message // empty' 2>/dev/null || true)"
if [[ -z "$BODY" ]]; then
  BODY="(no message)"
fi

# OSC 777 형식: ESC ] 777 ; notify ; title ; body BEL
# 세미콜론(;)이 title/body 안에 그대로 들어가면 파서가 필드를 오분할하므로 제거한다.
TITLE="${TITLE//;/,}"
BODY="${BODY//;/,}"

printf '\033]777;notify;%s;%s\007' "$TITLE" "$BODY" > /dev/tty
```

## settings.json 예시

`Stop`(작업 종료)과 `Notification`(권한 확인 등 입력 대기) 두 이벤트에 각각 다른
제목으로 위 스크립트를 연결한다:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/osc777-notify.sh 'Claude Code: done'"
          }
        ]
      }
    ],
    "Notification": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/osc777-notify.sh 'Claude Code: needs input'"
          }
        ]
      }
    ]
  }
}
```

- `Stop`: Claude Code가 응답을 끝내고 대기 상태로 돌아갈 때 발생 — "작업 완료"로 해석.
- `Notification`: 권한 확인 등 사용자 입력이 필요할 때 발생 — stdin JSON의 `.message`에
  실제 문구(예: "Claude needs your permission to use Bash")가 들어온다.

## 검증

이 예시가 실제로 동작하는지는 `scripts/wsl/osc-test.sh`의 OSC 777 케이스(7·8번)로
먼저 "/dev/tty로 쓴 OSC 777이 wmux 앱까지 도달하는가"를 검증한 뒤, Claude Code를
wmux 터미널 안에서 실행해 Stop/Notification 이벤트가 실제로 사이드바 상태·메시지
미리보기를 갱신하는지 확인한다 (spike-plan.md 6장 체크리스트 2번).

## 참고

- BEL(`\007`)만 완료 신호로 의존하지 않는다(계획 v2 9장) — wmux의 `OscScanner`는 BEL과
  ST(`ESC \`) 둘 다 종결자로 인식한다.
- ConPTY가 OSC 777을 잘라먹는 것으로 확인되면(6장 체크리스트 1번), 이 hook 경로는
  파일/소켓 감시 대안으로 교체해야 한다(계획 v2 2장 "단일 실패점" 참고). 이 문서의
  예시는 OSC passthrough가 살아있다는 전제 위에 있다.
