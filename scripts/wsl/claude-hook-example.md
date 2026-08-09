# wmux OSC 규약 — Claude Code hook / 셸 프롬프트

계획 v2 9장 "에이전트 상태 및 알림"의 경로를 실제로 구현하는 **규약 문서**다. wmux 가
해석하는 OSC 시퀀스의 의미가 여기 정의돼 있고(18단계 확정), Claude Code hook 과 셸
프롬프트 쪽 절반을 예시로 보인다.

```
Claude Code hook (UserPromptSubmit / Notification / Stop)
  → 현재 TTY에 OSC 777 출력
  → Rust PTY 리더(wmux-core::osc::OscScanner)가 감지
  → 100ms flush 창으로 배치(글루 OscRouter)
  → 탭 unread dot / pane 배지 / 워크스페이스 사이드바 상태 갱신
```

v1은 IPC 서버·Named Pipe·Windows helper CLI를 쓰지 않고 PTY 출력 자체(OSC 시퀀스)를
전달 경로로 쓴다.

## OSC 의미 규약

wmux 가 해석하는 시퀀스는 네 종류다.

| 시퀀스 | 의미 | 상태(`agentStatus`) | unread dot |
|---|---|---|---|
| `OSC 777;notify;wmux:running;<body>` | 에이전트 작업 시작 | `running` | 없음 |
| `OSC 777;notify;wmux:needsInput;<body>` | 사용자 입력 대기 | `needsInput` | 있음 |
| `OSC 777;notify;wmux:idle;<body>` | 작업 종료 | `idle` | 있음 |
| 그 밖의 `OSC 777` / 모든 `OSC 9` | 상태 중립 알림 | **불변** | 있음 |
| `OSC 0`(및 별칭 `OSC 2`) | 탭 제목 | 불변 | 없음 |
| `OSC 7` `file://host/path` | 탭 cwd (재시작 시 재스폰 위치) | 불변 | 없음 |

세부 규칙:

- **상태 토큰은 title 필드 전체가 정확히 일치**해야 한다 (`wmux:running` /
  `wmux:needsInput` / `wmux:idle`). 하나라도 어긋나면 상태 중립 알림으로 떨어진다 —
  다른 도구가 쏘는 777, ConEmu 진행률 같은 OSC 9 가 에이전트 상태를 주장하지
  못하게 하는 경계다.
- `body` 가 비어 있지 않으면 사이드바 미리보기(`lastAgentMessage`)로 남는다. **빈
  body 는 앞서 온 메시지를 지우지 않는다** — `running` 을 body 없이 쏴도 직전
  needsInput 문구가 유지된다. 메시지는 500자에서 잘린다.
- `running` 은 진행 신호라 dot 을 만들지 않는다. `needsInput`·`idle` 과 상태 중립
  알림만 unread 를 세운다.
- **화면에 보이는 탭은 unread 를 세우지 않는다** (활성 워크스페이스 + 그 pane 의 활성
  탭). 내용이 이미 눈앞에 있기 때문이다.
- 입력 대기가 우선이다: 어떤 탭이 `needsInput` 인 동안 **다른 탭**의 `running`/`idle`
  은 워크스페이스 상태를 덮지 못한다. 같은 탭이 보낸 상태만 강등할 수 있다 (사용자가
  응답하면 그 탭의 `UserPromptSubmit` → `running` 이 자연 강등한다).
- 세미콜론(`;`)은 필드 구분자다. title/body 안에 그대로 들어가면 파서가 필드를
  오분할하므로 방출 측에서 치환한다.
- 재시작하면 알림·상태는 전부 초기화된다 (죽은 세션의 needsInput 이 재시작을 넘지
  않는다 — 계획 v2 11장).

## 왜 `> /dev/tty` 리다이렉트가 필요한가

Claude Code hook의 **stdout은 Claude Code 자신이 소비한다** (hook 실행 결과로 처리되어
UI/로그에 쓰이거나 버려지며, 그대로 터미널 화면에 바이트가 흘러가지 않는다). hook 스크립트가
그냥 `echo`나 `printf`로 OSC 시퀀스를 표준출력에 쓰면 그 바이트는 실제 PTY 스트림에
도달하지 못하고, Rust PTY 리더는 아무것도 감지하지 못한다.

따라서 hook 스크립트는 OSC 시퀀스를 **표준출력이 아니라 `/dev/tty`에 직접** 써야 한다.
`/dev/tty`는 그 프로세스가 붙어 있는 실제 터미널 장치 파일이므로, Claude Code의 stdout
캡처를 우회해 PTY 스트림에 바로 실려 wmux의 Rust PTY 리더까지 도달한다.

(아래 셸 프롬프트 스니펫은 반대다 — 셸의 stdout이 곧 PTY라 리다이렉트가 필요 없다.)

## hook 스크립트 예시

`~/.claude/hooks/wmux-notify.sh` (실행 권한 부여 필요: `chmod +x`):

```bash
#!/usr/bin/env bash
# Claude Code hook에서 호출되어 wmux 상태 토큰을 OSC 777로 /dev/tty에 방출한다.
# 인자: $1 = 상태 토큰(wmux:running | wmux:needsInput | wmux:idle)
#       $2 = 본문(선택). Notification 이벤트는 stdin JSON의 .message를 우선한다.
set -euo pipefail

STATUS="${1:?usage: wmux-notify.sh <wmux:running|wmux:needsInput|wmux:idle> [body]}"
BODY="${2:-}"

# Claude Code는 hook 실행 시 이벤트 정보를 stdin으로 JSON을 전달한다.
# Notification 이벤트는 .message 필드에 사람이 읽을 알림 문구가 들어 있다.
# jq가 없으면 인자로 받은 기본 본문으로 내려간다(hook 자체는 계속 동작한다).
if [[ ! -t 0 ]]; then
  INPUT_JSON="$(cat)"
  if command -v jq > /dev/null 2>&1; then
    FROM_JSON="$(printf '%s' "$INPUT_JSON" | jq -r '.message // empty' 2>/dev/null || true)"
    if [[ -n "$FROM_JSON" ]]; then
      BODY="$FROM_JSON"
    fi
  fi
fi

# 세미콜론(;)이 body 안에 그대로 들어가면 파서가 필드를 오분할하므로 치환한다.
BODY="${BODY//;/,}"

# OSC 777 형식: ESC ] 777 ; notify ; title ; body BEL
printf '\033]777;notify;%s;%s\007' "$STATUS" "$BODY" > /dev/tty
```

## settings.json 예시

세 이벤트를 상태 토큰 3종에 매핑한다:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/wmux-notify.sh wmux:running"
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
            "command": "~/.claude/hooks/wmux-notify.sh wmux:needsInput 'needs input'"
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/wmux-notify.sh wmux:idle done"
          }
        ]
      }
    ]
  }
}
```

- `UserPromptSubmit`: 사용자가 프롬프트를 보낸 직후 — "작업 시작"(`running`). 본문을
  주지 않으므로 직전 미리보기 문구가 그대로 남는다.
- `Notification`: 권한 확인 등 사용자 입력이 필요할 때 — stdin JSON의 `.message`에
  실제 문구(예: "Claude needs your permission to use Bash")가 들어온다.
- `Stop`: Claude Code가 응답을 끝내고 대기 상태로 돌아갈 때 — "작업 완료"(`idle`).

### (선택) Stop 에 마지막 응답 문구 싣기

`Stop` 의 stdin JSON에는 `.transcript_path`(JSONL)가 들어 있어, 마지막 assistant
메시지를 뽑아 미리보기로 쓸 수 있다. jq 의존이 커지므로 기본 예시에는 넣지 않았다 —
필요하면 위 스크립트의 `BODY` 결정부에 이어 붙인다:

```bash
TRANSCRIPT="$(printf '%s' "$INPUT_JSON" | jq -r '.transcript_path // empty')"
if [[ -n "$TRANSCRIPT" && -r "$TRANSCRIPT" ]]; then
  LAST="$(jq -rs '[.[] | select(.type == "assistant")] | last
                  | .message.content[]? | select(.type == "text") | .text' \
          "$TRANSCRIPT" 2>/dev/null | tail -n 1 || true)"
  if [[ -n "$LAST" ]]; then
    BODY="$LAST"
  fi
fi
```

## 셸 프롬프트에서 제목·cwd 방출 (OSC 0 / OSC 7)

탭 제목과 cwd 는 hook 이 아니라 셸이 프롬프트마다 방출한다. WSL 쪽 `~/.bashrc` 에:

```bash
# 프롬프트마다 현재 디렉터리(OSC 7)와 탭 제목(OSC 0)을 방출한다.
# 셸의 stdout이 곧 PTY라 /dev/tty 리다이렉트가 필요 없다.
__wmux_osc() {
  # OSC 7: file://<host>/<path> — wmux는 host를 무시하고 경로만 쓴다(ST 종결).
  printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-wsl}" "$PWD"
  # OSC 0: 탭 제목 — 여기서는 디렉터리 이름(BEL 종결).
  printf '\033]0;%s\007' "${PWD##*/}"
}
PROMPT_COMMAND="__wmux_osc${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```

- wmux 는 OSC 7 경로를 percent-decode 한다. 경로에 `%` 가 들어 있으면 규약대로
  `%25` 로 인코딩해야 정확하다 (`%` 뒤가 16진수 2자리가 아니면 리터럴로 남긴다).
- 이 cwd 는 **재시작 후 재스폰 위치**로 쓰인다 — 마지막으로 있던 디렉터리에서 셸이
  다시 열린다.
- 제목을 에이전트 이름 등으로 바꾸고 싶으면 OSC 0 문자열만 갈아 끼우면 된다. ConPTY
  가 OSC 0 을 OSC 2 로 재인코딩해 흘려도 wmux 는 같은 의미로 받는다.

## 검증

1. `scripts/wsl/osc-test.sh` 로 "/dev/tty로 쓴 OSC 가 wmux 앱까지 도달하는가"를 먼저
   확인한다 (OSC 777 은 7·8·9번 케이스).
2. 그다음 Claude Code를 wmux 터미널 안에서 실행해 hook 3종이 실제로 탭 dot·pane
   배지·사이드바 상태/미리보기를 갱신하는지 확인한다
   (`docs/WINDOWS-BUILD.md` 10장 체크포인트 2).

## 참고

- BEL(`\007`)만 완료 신호로 의존하지 않는다(계획 v2 9장) — wmux의 `OscScanner`는 BEL과
  ST(`ESC \`) 둘 다 종결자로 인식한다.
- ConPTY가 OSC 777을 잘라먹는 것으로 확인되면(spike-plan.md 6장 체크리스트 1번), 이
  hook 경로는 파일/소켓 감시 대안으로 교체해야 한다(계획 v2 2장 "단일 실패점" 참고).
  이 문서의 예시는 OSC passthrough가 살아있다는 전제 위에 있다. OSC 0/7 의 실전
  passthrough 는 아직 미검증이라, 실패해도 알림 경로(777/9)와는 독립이다.
