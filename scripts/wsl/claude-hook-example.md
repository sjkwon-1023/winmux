# winmux OSC 규약 — Claude Code hook / 셸 프롬프트

계획 v2 9장 "에이전트 상태 및 알림"의 경로를 실제로 구현하는 **규약 문서**다. winmux 가
해석하는 OSC 시퀀스의 의미가 여기 정의돼 있고(18단계 확정), Claude Code hook 과 셸
프롬프트 쪽 절반을 예시로 보인다.

```
Claude Code hook (UserPromptSubmit / Notification / Stop)
  → 해석한 TTY(/dev/tty 또는 조상 프로세스의 pts)에 OSC 777 출력
  → Rust PTY 리더(winmux-core::osc::OscScanner)가 감지
  → 100ms flush 창으로 배치(글루 OscRouter)
  → 탭 unread dot / pane 배지 / 워크스페이스 사이드바 상태 갱신
```

v1은 IPC 서버·Named Pipe·Windows helper CLI를 쓰지 않고 PTY 출력 자체(OSC 시퀀스)를
전달 경로로 쓴다.

## OSC 의미 규약

winmux 가 해석하는 시퀀스는 네 종류다.

| 시퀀스 | 의미 | 상태(`agentStatus`) | unread dot |
|---|---|---|---|
| `OSC 777;notify;winmux:running;<body>` | 에이전트 작업 시작 | `running` | 없음 |
| `OSC 777;notify;winmux:needsInput;<body>` | 사용자 입력 대기 | `needsInput` | 있음 |
| `OSC 777;notify;winmux:idle;<body>` | 작업 종료 | `idle` | 있음 |
| 그 밖의 `OSC 777` / 모든 `OSC 9` | 상태 중립 알림 | **불변** | 있음 |
| `OSC 0`(및 별칭 `OSC 2`) | 탭 제목 | 불변 | 없음 |
| `OSC 7` `file://host/path` | 탭 cwd (재시작 시 재스폰 위치) | 불변 | 없음 |

세부 규칙:

- **상태 토큰은 title 필드 전체가 정확히 일치**해야 한다 (`winmux:running` /
  `winmux:needsInput` / `winmux:idle`). 하나라도 어긋나면 상태 중립 알림으로 떨어진다 —
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

## tty 해석 규율 — 직접 `/dev/tty` → 조상 pts fallback

Claude Code hook의 **stdout은 Claude Code 자신이 소비한다** (hook 실행 결과로 처리되어
UI/로그에 쓰이거나 버려지며, 그대로 터미널 화면에 바이트가 흘러가지 않는다). hook 스크립트가
그냥 `echo`나 `printf`로 OSC 시퀀스를 표준출력에 쓰면 그 바이트는 실제 PTY 스트림에
도달하지 못하고, Rust PTY 리더는 아무것도 감지하지 못한다.

따라서 hook 스크립트는 OSC 시퀀스를 **표준출력이 아니라 터미널 장치에 직접** 써야 한다.
문제는 그 장치를 어떻게 찾느냐인데, `> /dev/tty` 한 방으로는 부족하다.

**실측(Claude Code 2.1.226, 체크포인트 2):** hook 프로세스에는 **controlling TTY 가 없어**
`> /dev/tty` 가 `No such device or address`(ENXIO)로 실패한다. 반면 **Claude Code 본체
프로세스는 winmux 가 띄운 `/dev/pts/N` 에 그대로 붙어 있다** — 장치는 살아 있는데 hook 쪽에만
거기로 가는 손잡이가 없는 상태다. 그래서 아래 예시의 `winmux_emit` 은 tty 를 두 단계로 해석한다.

1. **직접 `/dev/tty`** — controlling TTY 가 있으면(손으로 돌려 보는 경우, hook 을 다른
   경로로 띄우는 경우, 이 전제가 바뀐 미래 버전) 이게 정답이고 한 번에 끝난다.
2. **조상 pts fallback** — 1이 실패하면 자기 자신부터 `/proc/<pid>/stat` 의 PPID 를 따라
   최대 8칸까지 거슬러 올라가며, 각 프로세스 fd 0/1/2 의 `readlink` 가 `/dev/pts/*` 를
   가리키면 거기에 쓴다. **가장 가까운 조상이 이긴다** — 터미널이 중첩돼 있어도 바깥
   터미널이 아니라 자기 pane 의 pts 를 집는다.

둘 다 실패하면(조상 체인에 pts 가 없는 경우 — 예: hook 이 init 으로 reparent 된 뒤 도는
경우) **조용히 포기하고 `exit 0` 한다.** 알림 하나를 놓치는 것보다 알림 전달 실패가 Claude
세션을 깨는 쪽이 나쁘다. 깊이 상한 8과 `/dev/pts/*` 화이트리스트는 이 탐색이 오래 끌거나
엉뚱한 대상(로그 파일·파이프)에 OSC 바이트를 흘리지 않게 막는 경계다.

(아래 셸 프롬프트 스니펫은 이 문제와 무관하다 — 셸은 자기 tty 를 가지고 있고 그 stdout이
곧 PTY라 리다이렉트도 fallback도 필요 없다.)

## hook 스크립트 예시

`~/.claude/hooks/winmux-notify.sh` (실행 권한 부여 필요: `chmod +x`):

```bash
#!/usr/bin/env bash
# Claude Code hook에서 호출되어 winmux 상태 토큰을 OSC 777로 실제 터미널 장치에 방출한다.
# 인자: $1 = 상태 토큰(winmux:running | winmux:needsInput | winmux:idle)
#       $2 = 본문(선택). Notification 이벤트는 stdin JSON의 .message를 우선한다.
set -euo pipefail

STATUS="${1:?usage: winmux-notify.sh <winmux:running|winmux:needsInput|winmux:idle> [body]}"
BODY="${2:-}"

# OSC 바이트를 실제 터미널 장치에 쓴다. 위 "tty 해석 규율"의 2단계를 그대로 구현한다.
#   1) /dev/tty — controlling TTY 가 있으면 이게 정답이다.
#   2) /proc 조상 체인 — Claude Code 2.1.226 의 hook 프로세스에는 controlling TTY 가
#      없어 1)이 ENXIO("No such device or address")로 실패한다. 이때는 자신부터 부모로
#      거슬러 올라가며 각 프로세스 fd 0/1/2 가 가리키는 /dev/pts/* 를 찾아 거기에 쓴다.
#      Claude Code 본체가 winmux 의 pts 에 붙어 있으므로 몇 칸 위에서 잡힌다.
# 어느 쪽도 안 되면 조용히 포기한다 — 알림 실패가 Claude 세션을 깨면 안 된다.
winmux_emit() {
  local payload="$1"

  if { printf '%s' "$payload" > /dev/tty; } 2>/dev/null; then
    return 0
  fi

  local pid=$$ depth=0 fd target stat ppid
  while [[ "$pid" -gt 1 && "$depth" -lt 8 ]]; do
    for fd in 0 1 2; do
      target="$(readlink "/proc/$pid/fd/$fd" 2>/dev/null || true)"
      [[ "$target" == /dev/pts/* ]] || continue
      if { printf '%s' "$payload" > "$target"; } 2>/dev/null; then
        return 0
      fi
    done
    # /proc/<pid>/stat 은 "<pid> (<comm>) <state> <ppid> ..." 형식이다. comm 에 공백·
    # 괄호가 들어갈 수 있어 마지막 ')' 뒤부터 잘라 state 다음의 ppid 를 읽는다.
    stat="$(cat "/proc/$pid/stat" 2>/dev/null || true)"
    [[ -n "$stat" ]] || break
    stat="${stat##*) }"
    ppid="${stat#* }"
    ppid="${ppid%% *}"
    [[ "$ppid" =~ ^[0-9]+$ ]] || break
    pid="$ppid"
    depth=$((depth + 1))
  done

  return 1
}

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
winmux_emit "$(printf '\033]777;notify;%s;%s\007' "$STATUS" "$BODY")" || true

# 방출에 실패해도 hook 은 성공으로 끝낸다(알림을 놓칠지언정 세션은 깨지 않는다).
exit 0
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
            "command": "~/.claude/hooks/winmux-notify.sh winmux:running"
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
            "command": "~/.claude/hooks/winmux-notify.sh winmux:needsInput 'needs input'"
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
            "command": "~/.claude/hooks/winmux-notify.sh winmux:idle done"
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
__winmux_osc() {
  # OSC 7: file://<host>/<path> — winmux는 host를 무시하고 경로만 쓴다(ST 종결).
  printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-wsl}" "$PWD"
  # OSC 0: 탭 제목 — 여기서는 디렉터리 이름(BEL 종결).
  printf '\033]0;%s\007' "${PWD##*/}"
}
PROMPT_COMMAND="__winmux_osc${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```

- winmux 는 OSC 7 경로를 percent-decode 한다. 경로에 `%` 가 들어 있으면 규약대로
  `%25` 로 인코딩해야 정확하다 (`%` 뒤가 16진수 2자리가 아니면 리터럴로 남긴다).
- 이 cwd 는 **재시작 후 재스폰 위치**로 쓰인다 — 마지막으로 있던 디렉터리에서 셸이
  다시 열린다.
- 제목을 에이전트 이름 등으로 바꾸고 싶으면 OSC 0 문자열만 갈아 끼우면 된다. ConPTY
  가 OSC 0 을 OSC 2 로 재인코딩해 흘려도 winmux 는 같은 의미로 받는다.

## 검증

1. `scripts/wsl/osc-test.sh` 로 "/dev/tty로 쓴 OSC 가 winmux 앱까지 도달하는가"를 먼저
   확인한다 (OSC 777 은 7·8·9번 케이스). 이 스크립트는 셸에서 직접 돌아 tty 를 가지므로
   1단계 경로만 탄다 — 전달 경로 자체가 살아 있는지 보는 용도다.
2. 그다음 Claude Code를 winmux 터미널 안에서 실행해 hook 3종이 실제로 탭 dot·pane
   배지·사이드바 상태/미리보기를 갱신하는지 확인한다
   (`docs/WINDOWS-BUILD.md` 10장 체크포인트 2).
3. hook 이 조용하면 fallback 이 어디서 끊겼는지부터 본다. winmux 터미널 안에서
   `setsid -w bash -c 'printf "" > /dev/tty' ; echo $?` 로 tty 없는 맥락을 재현하고,
   `for p in $(pgrep -f 'claude'); do readlink /proc/$p/fd/1; done` 로 Claude 본체가
   `/dev/pts/*` 에 붙어 있는지 확인한다. 조상 체인이 8칸 안에 있는지도 같이 본다.

## 참고

- BEL(`\007`)만 완료 신호로 의존하지 않는다(계획 v2 9장) — winmux의 `OscScanner`는 BEL과
  ST(`ESC \`) 둘 다 종결자로 인식한다.
- "hook 에 controlling TTY 가 없다"는 것은 Claude Code **2.1.226 에서 실측한 사실**이지
  보장된 계약이 아니다. 예시 스크립트가 1단계를 남겨 둔 이유가 이것이다 — 나중 버전이
  hook 에 tty 를 물려 주면 fallback 을 타지 않고 그대로 동작한다. 반대로 Claude Code 가
  pts 가 아닌 곳(파이프 전용 데몬 등)에서 돌게 되면 2단계도 대상을 못 찾으므로, 그때는
  파일/소켓 감시 대안으로 넘어가야 한다.
- ConPTY가 OSC 777을 잘라먹는 것으로 확인되면(spike-plan.md 6장 체크리스트 1번), 이
  hook 경로는 파일/소켓 감시 대안으로 교체해야 한다(계획 v2 2장 "단일 실패점" 참고).
  이 문서의 예시는 OSC passthrough가 살아있다는 전제 위에 있다. OSC 0/7 의 실전
  passthrough 는 아직 미검증이라, 실패해도 알림 경로(777/9)와는 독립이다.
