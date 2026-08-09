# MVP 17단계 실행 계획 — 패널 간 텍스트 전달

계획 v2 8장·17장 17단계. 아키텍처가 굳은 소형 UI 스테이지라 메인 직접 확정 —
구현 후 change-critic 점검은 동일 적용.

## 0. 설계 결정

- **D1. 전달은 대상 pane 의 xterm paste 경로 경유** (계획 v2 8장 구현 주의 그대로):
  `ESC[200~` 를 raw 로 PTY 에 쓰지 않는다 — 대상 뷰의 `term.paste(text)` 가
  bracketed paste 모드 추적을 담당하고, 결과는 기존 onData→write_stdin 으로 흐른다.
  Submit 은 paste 후 `write_stdin("\r")` 1회. **백엔드 신규 커맨드 없음** — API 3종
  분리(sendText/sendTextAndSubmit/sendRaw)는 프론트 API 층(terminal-view 공개
  메서드)에서 실현하고, v2 MCP 도구화 시점에 글루 커맨드로 승격한다.
- **D2. UI = 소스 pane 헤더의 전달 아이콘 2개 + 대상 클릭 모드** (마우스 우선):
  - `⤷` (Send selection) / `⤷⏎` (Send & run): 클릭 시 **현재 pane 의 표시 중
    터미널 선택 텍스트**를 캡처하고 "대상 선택 모드"에 진입. 선택이 없으면 상태
    라인에 에러 one-shot ("no selection to send").
  - 대상 선택 모드: 상태 라인에 "click a pane to send to (Esc cancels)" 프롬프트,
    다음 pane mousedown(capture, 주 버튼)이 대상 확정 → 전달(+submit). 소스 pane
    자신을 클릭하면 취소와 동일 처리(자기 전달은 무의미). Esc 는 모드 활성 중에만
    window capture 로 가로채 취소한다 — 계획 v2 3장 가로채기 목록에 "Esc (전달
    대상 선택 모드 중일 때만)" 를 추가하는 의도적 확장.
  - "전달"과 "전달 후 실행"이 아이콘부터 분리 — 실수 실행 방지 (계획 v2 8장).
- **D3. 대상은 활성 워크스페이스의 표시 중 터미널** (pane 의 shown 탭이 터미널일
  때) — keep-alive 로 항상 attach 돼 있어 paste 경로가 성립한다. 빈 pane·뷰어
  탭(21단계) 대상은 상태 라인 에러. 워크스페이스 간 전달은 v1 제외 (대상 뷰가
  lazy-attach 전이라 paste 경로가 없다 — MCP 시점에 재설계).
- **D4. 순수 로직 분리**: 대상 선택 모드 상태 머신(`send-mode.ts` — arm(text,
  submit)/cancel/resolve(target) + 프롬프트 문자열)을 DOM-free 로 두고 vitest.

## 1. 구현 범위

- terminal-view: `getSelection(): string` · `paste(text)` · `submit()`(=CR write) 공개.
- pane-view: 헤더 아이콘 `⤷`·`⤷⏎` (활성 탭이 터미널이고 선택 존재 시 동작),
  대상 클릭은 기존 mousedown capture 에 send-mode 분기 추가.
- main.ts / workspace-view: send-mode 상태 보유·상태 라인 프롬프트·Esc capture
  (모드 중에만)·대상 pane → 표시 뷰로 전달 실행.
- styles: 아이콘·모드 중 커서/하이라이트 최소 표시.
- vitest: send-mode 상태 머신, (기존 스위트 회귀).
- WINDOWS-BUILD.md: 체크포인트 2 절 신설 시작 — 17단계 항목(선택 전달·전달+실행
  분리·bracketed paste 안전성(vim 에 전달 시 자동 실행 안 됨)·무선택 에러·Esc 취소).

## 2. 완료 기준

자동 게이트 전부 green + change-critic → 체크포인트 2(21단계 후)에서 실기 검증.
