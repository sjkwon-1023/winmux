# Stage 20 실행계획 — 키보드 3층 이동

> 메인 직접 계획 (2026-08-09). 근거: 터미널-계획-v2.md "키보드 모델" 장 (3층 구조에 맞춘
> 3종 이동) + "키보드 가로채기 목록" 장 (앱이 가로채는 단축키를 명시 목록으로 관리).
> 소규모 단일 표면이라 drafter/critic 왕복 없이 확정한다.

## 키 매핑 (= 가로채기 목록 추가분)

| 키 | 동작 | 대상 명령 |
|---|---|---|
| `Ctrl+1`~`Ctrl+9` | 워크스페이스 전환 (사이드바 순서 1-based) | `SwitchWorkspace` |
| `Alt+↑↓←→` | pane 포커스 이동 (기하학적 인접) | `FocusPane` |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | 활성 pane 내 탭 순환 (다음/이전, 순환) | `ActivateTab` |

계획의 "또는 Ctrl+↑↓" 는 채택하지 않는다 — TUI 앱의 Ctrl+방향키 사용과 충돌 위험이
있고 Ctrl+1~9 로 충분하다. 기존 가로채기(스파이크 확정분)와의 합집합이 전체 목록이다:
Ctrl+Shift+R(리로드), Ctrl+C/V·Ctrl+Insert/Shift+Insert(복사·붙여넣기 관례), Esc(send-mode
활성 중에만). **이 목록은 keys.ts 모듈 doc 주석을 canonical 로 유지한다** (계획 v2
"키보드 가로채기 목록" 장의 요구 — 코드와 목록이 한곳에).

## 설계

- **순수 모듈 `keys.ts`**: 판정을 DOM 무의존으로 분리해 vitest 로 잠근다.
  - `keyAction(spec) -> KeyAction | null` — spec 은 `{ key, ctrl, alt, shift, isComposing }`.
    IME 조합 중(`isComposing`)은 항상 null. KeyAction 은
    `{ type: "switchWorkspace"; ordinal }` | `{ type: "focusPane"; dir }` |
    `{ type: "cycleTab"; delta: 1 | -1 }`.
  - `paneInDirection(rects, from, dir) -> PaneId | null` — pane 기하 목록
    (`{ pane, x, y, w, h }`)에서 방향 반평면에 있는 pane 중 중심 거리 최소를 고른다.
    후보 없음·from 미존재는 null (no-op).
- **글루 (main.ts + workspace-view)**: window keydown **capture** 에 설치해 xterm 보다
  먼저 잡고, 매칭 시 `preventDefault`+`stopPropagation`. 해석은 최신 채택 스냅샷 기준:
  - switchWorkspace: `workspaces[ordinal-1]` 없으면 no-op, 이미 활성이면 no-op.
    dispatch 후 기존 사이드바 클릭 경로와 같은 focus 보상(activePane)을 태운다.
  - focusPane: workspace-view 가 `paneRects()` (pane root 의 boundingClientRect) 를
    노출하고, 활성 pane 기준 `paneInDirection` 결과를 `FocusPane` 으로 dispatch
    (기존 requestFocus pane 보상 재사용).
  - cycleTab: 활성 pane 의 tabs 에서 active_tab 의 이웃(순환)을 `ActivateTab` 으로.
    탭 0~1개면 no-op.
- **send-mode 와의 상호작용**: 가로채기 키는 send-mode 활성 중에도 동작한다 —
  워크스페이스 전환 시 기존 render 수명 가드가 자동 취소한다 (17단계 확정 동작).
  Esc 는 send-mode 리스너가 먼저 잡는다 (기존 capture 설치 순서 유지).

## 실행

1. `keys.ts` (순수 판정 + 기하) + vitest (매핑 전수·IME 가드·경계: ordinal 초과,
   후보 없음, 1-pane, 0~1-tab, 순환 wrap).
2. main.ts 배선 + workspace-view `paneRects()`/활성 pane·탭 조회 노출. 기존
   Ctrl+Shift+R 리스너·send-mode Esc 와의 순서 불변.
3. WINDOWS-BUILD.md §10 에 Stage 20 수동 항목: 3종 이동 각각 + "터미널로 새지 않음"
   (가로챈 키가 셸에 문자를 남기지 않는다) + Ctrl+Tab 이 WebView2 에서 페이지에
   도달하는지 (미검증 전제 — 실패 시 대체 키 선정이 필요함을 명시).

## 완료 기준

게이트 5종 green + keys.ts vitest. 수동은 체크포인트 2 로 배치.

## 리스크

- [med] WebView2 가 Ctrl+Tab 을 페이지로 안 넘길 가능성 — 체크포인트 2 에서 확인,
  실패 시 대체 키(예: Ctrl+PgUp/PgDn)로 교체하는 후속 (판정 모듈이라 교체 비용 소).
- [low] Alt+방향키를 쓰는 셸/TUI 와의 충돌 — 계획이 명시 배정한 키라 수용, 가로채기
  목록에 명시.
