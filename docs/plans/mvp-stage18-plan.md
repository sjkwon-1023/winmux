# Stage 18 실행계획 — OSC 알림 라우팅 + 스냅샷 coalescing + keyed reconcile

> plan-drafter 초안 → plan-critic 반증 → 메인 취합 확정본 (2026-08-09).
> 근거: 터미널-계획-v2.md 9장(알림 라우팅 3층·Claude Code hook)·4장(모델 필드)·11장(재시작 후
> 알림 초기화), ADR-0002 결정 1("coalescing 은 18단계 착지 전 설계"), ADR-0003 결정 7
> (stringify 스킵 가드의 mid-click 스왈로 이력 — 동적 필드가 가드를 무력화하면 재발).

## 목표

OSC 777/9 알림과 OSC 0("2" 별칭 포함)/7 을 Rust 모델로 라우팅해 탭 unread dot → pane 배지 →
워크스페이스 사이드바 3층 표면을 실데이터로 돌리고, 그 전제인 스냅샷 coalescing(OSC 플러드 →
snapshot-per-mutation cliff 해소)과 프론트 keyed reconcile(전체 재조립 → mid-click 스왈로 재발
방지)을 함께 착지시킨다. Claude Code hook 규약을 실규약으로 확정한다.

범위 결정: OSC 0/2 제목 라우팅은 브리프 밖 확장이지만 **포함** — OSC 7 은 ADR-0004 가 18단계로
명시했고, 제목은 spike 부터 파서(`Osc0Title`)가 이미 있어 라우팅만 잇는 저비용 인접 작업이며,
탭바 동적 제목이 keyed reconcile 의 첫 실사례가 된다. Windows toast 는 **제외** (의존성·ARM64
검증 규모가 별도 작업 — flush 지점이 자연스러운 훅 포인트임을 주석으로 남긴다).

## hook / OSC 의미 규약 (canonical: scripts/wsl/claude-hook-example.md 갱신본)

- OSC 777 `notify;title;body` 에서 title 이 `winmux:running` | `winmux:needsInput` | `winmux:idle`
  이면 **상태 알림**: `Workspace.agent_status` = 해당 값, body 가 비어있지 않으면
  `last_agent_message` = body(500자 절단), unread 는 **needsInput·idle 만** 세팅 (running 은
  진행 신호 — dot 없음).
- 토큰 불일치 777·OSC 9 는 **상태 중립 알림**: unread + message 만, `agent_status` 불변
  (OSC 9 는 ConEmu 진행률 등 타 도구 잡음 가능성 — 상태를 주장하지 않는다). 백그라운드
  워크스페이스에서 순수 OSC 9 는 탭 dot + 사이드바 집계 dot(아래 B-6)으로 표면화된다 — 의도.
- Claude Code hook 매핑: `UserPromptSubmit`→`winmux:running`(body 없음) /
  `Notification`→`winmux:needsInput`+stdin `.message` / `Stop`→`winmux:idle`+"done"
  (transcript 마지막 메시지 추출은 선택 스니펫으로만 문서화 — jq 의존 최소화). 기존
  `> /dev/tty` 규율 유지.
- OSC 0(및 "2" — ConPTY 재인코딩 대비 별칭) → `Tab.title`. OSC 7 `file://host/path` →
  percent-decode 한 경로를 `TabKind::Terminal.cwd` 에 (respawn 이 탭 cwd 를 사용 —
  command.rs:376). 둘 다 unread 없음.

## core 계약 (crates/winmux-core)

- `osc.rs`: `parse_payload` 에 `"2"` → `Osc0Title` 별칭 (enum 불변 — spike `kind:"0"` 계약
  무영향).
- 신규 `notify.rs` (순수 — coalescing 자료구조 + 파서):
  ```rust
  pub struct OscBatch { entries: BTreeMap<SessionId, OscDelta> }   // Default
  pub struct OscDelta { title: Option<String>, cwd: Option<String>,
                        status: Option<AgentStatus>, message: Option<String>, unread: bool }
  impl OscBatch {
      pub fn merge(&mut self, session: SessionId, ev: &OscEvent);  // O(1): last-wins(title/cwd/
      pub fn is_empty(&self) -> bool;                              //   status), last-non-empty
      pub fn take(&mut self) -> OscBatch;                          //   (message), sticky unread
  }
  ```
  `winmux:` 토큰 파스, `file://` URI→경로(percent-decode), 500자 절단 포함. 메모리는 세션 수에
  상한 (큐가 아니라 cell). 창 내 cross-session 적용 순서는 세션 id 순이지만, 아래 needsInput
  우선 규칙이 load-bearing 케이스를 순서 무관하게 만들므로 수용 (critic 반영).
- `Dispatcher::apply_osc(&mut self, batch: OscBatch, now_ms: u64) -> bool` —
  `SessionExited` 와 동일한 세션→탭 선형 역매핑. 규칙 (critic 반영분 포함):
  - 미지 세션 no-op. **Exited 탭은 델타 통째 스킵** — 100ms 창 안에서 세션이 종료되면 즉시
    처리된 SessionExited 의 Idle 리셋 뒤에 지연 배치가 죽은 탭에 needsInput 을 재도장하는
    구멍을 막는다 (critic high).
  - 상태 반영은 **needsInput 우선**: 현재 `agent_status == NeedsInput` 이면 새 status 가
    NeedsInput 이거나 델타 탭 == `agent_status_source` 일 때만 갱신 (다른 탭의 running 이
    입력 대기를 가리지 않는다 — 계획 v2 9장 "사이드바만 훑어도 입력 대기가 보인다"). 사용자가
    응답하면 같은 source 의 UserPromptSubmit(running)이 자연 강등한다.
  - 필드 반영 + `Tab.last_activity_ms = now_ms` (이번 단계는 **데이터만 배선, UI 표면 없음** —
    ADR-0002 의 18단계 배선 약속 이행, 표면은 후속).
  - unread 는 **가시 탭(active workspace + 그 pane 의 active_tab) 억제**. 창 포커스와는 결합
    하지 않는다 (v1 결정): 활성 탭은 터미널 콘텐츠 자체가 보이고, 자리 비운 사용자에게는
    사이드바 agent_status 가 남는다.
  - 변경 시 revision **배치당 1회** 증가. 시간은 glue 주입 (코어 순수성).
- `Workspace.agent_status_source: Option<TabId>` 신규 —
  `#[serde(default, skip_serializing_if = "Option::is_none")]` (None 이면 JSON 불출력 → 기존
  golden fixture 무변경; critic 이 round-trip 성립 확인). status 기록 시 세팅.
- **source 리셋 헬퍼** (critic 반영 — ClosePane 누락 방지): `CloseTab`·`ClosePane`(제거되는
  각 탭)·`SessionExited` 에서 그 탭이 source 면 `agent_status = Idle`, `agent_status_source =
  None`. 공통 헬퍼 하나로 세 경로를 묶는다.
- unread 해제: `ActivateTab` 대상 탭, `SwitchWorkspace` 시 각 pane 의 `active_tab` —
  "가시화 = 읽음" (이미 활성인 탭은 활성화 이벤트가 다시 안 오므로 OSC 시점 억제가 필수 짝).
- `persist.rs` sanitize 확장: 로드 시 `agent_status = Idle`·`agent_status_source /
  last_agent_message = None`·전 탭 `notification = None`·`last_activity_ms = None` 무조건
  초기화 (`pty_session` 소거와 동급 — 죽은 세션의 needsInput 이 재시작을 넘지 않게, 계획 11장).

## glue 계약 (apps/winmux/src-tauri)

- 신규 `router.rs`:
  ```rust
  pub struct OscRouter { inner: Arc<RouterInner>, worker: Option<JoinHandle<()>> }
  struct RouterInner { pending: Mutex<RouterState>, cond: Condvar }
  struct RouterState { batch: OscBatch, closed: bool }
  ```
  - `push(session, &OscEvent)`: 리더 스레드에서 pending lock 아래 merge + notify 만 —
    **Dispatcher lock 을 핫패스에서 절대 잡지 않는다** (state.rs 잠금 규율).
  - worker: predicate loop(스퓨리어스 웨이크업 안전 — batch 비었고 !closed 인 동안 wait) →
    비면 아님이 확인되면 `WINMUX_OSC_FLUSH_MS`(기본 100, `WINMUX_RESET_*` knob 관례) 트레일링
    대기 → take(pending lock 해제 후) → dispatcher lock → `apply_osc(now)` → **변경 시에만**
    `publish_state`. pending lock 과 dispatcher lock 을 동시에 잡지 않는다 (데드락 불가 —
    critic 확인: Saver worker 는 dispatcher lock 을 안 잡음).
  - **수명 규율 (critic 반영 — Saver Drop 규율 답습)**: `Drop` 에서 closed 세팅 + notify +
    join. `flush_now()` 는 pending 을 동기 take→apply→publish — 앱 종료(RunEvent::Exit) 시
    **Saver flush 앞**에 호출해 cwd·상태 유실 창을 없앤다.
- `sink.rs on_osc`: `osc-event` emit 제거 → `router.push` 교체 (winmux 프론트에 osc-event
  리스너 없음 — spike 전용, spike 는 자체 글루라 무관).
- 구조 변이(dispatch·exit)는 지금처럼 즉시 emit 유지. 중복 emit 이중 방어: `apply_osc` false
  → emit 스킵 + 프론트 store revision 가드.

## 프론트 계약 (apps/winmux/src)

- `types.ts`: `Workspace.agentStatusSource?: TabId` (optional — 런타임 스냅샷에 실릴 수 있음,
  fixture 는 불변).
- 사이드바 (B-6): 카드 모델에 `preview`(lastAgentMessage) + **집계 unread dot**
  (= any(워크스페이스 내 탭 unread) — 상태 중립 알림도 백그라운드 워크스페이스에서 보이게,
  3층의 워크스페이스 층 완성). 렌더를 카드별 id-키잉 in-place 패치로: 멤버십·순서 동일 →
  텍스트/클래스만 갱신, 변화 → 재조립. diff 판정은 순수 함수(`reconcilePlan(prev, next)` 류)
  로 분리해 vitest.
- 탭바·pane 배지 (B-7): `renderTabStrip` 을 tab id 키 in-place(제목·dot·active 클래스 토글)
  로, 멤버십·순서 변화만 재조립. pane 헤더 배지 `●` = any(tab unread) (계획 9장 Pane 층).
- **노드 identity 보존 테스트** (critic 반영 — 순수 판정 테스트는 mid-click 스왈로의 본체인
  "눌린 엘리먼트 갈아치움"을 못 잡는다): `happy-dom` devDependency + 파일별
  `@vitest-environment` 로, unchanged 카드/탭의 DOM 노드가 스냅샷 갱신 후에도 동일 객체임을
  단언한다. 수동 체크포인트(제목 갱신 중 클릭)와 이중 잠금.

## 실행 청크

**청크 A — 코어 + 글루 (high risk: 동시성·계약).** 순서 의존 5단계:
1. `osc.rs` OSC 2 별칭 + 테스트.
2. `notify.rs` OscBatch/OscDelta/merge + 토큰·URI 파서 + 테스트 (last-wins·sticky·절단·상한).
3. `apply_osc` + needsInput 우선 + Exited 스킵 + source 리셋 헬퍼(CloseTab/ClosePane/
   SessionExited) + 해제 규칙(ActivateTab/SwitchWorkspace) + `agent_status_source` 필드 +
   테스트 (역매핑·가시 억제·우선 규칙·Exited 스킵·리셋 3경로·revision 1회·fixture round-trip
   무변경).
4. `persist.rs` sanitize 확장 + 테스트.
5. `router.rs` + `sink.rs` 교체 + main 배선(Exit 시 flush_now → Saver flush 순) +
   `claude-hook-example.md` 실규약 갱신 + docs 내 osc-event 참조 정리.

**주의 — A 착지 직후는 알려진 중간 상태다** (critic 반영): 동적 status/message 가 프론트
stringify 가드를 무력화해 ADR-0003 d7 스왈로 회귀가 B 착지까지 라이브다. A·B 는 커밋은 따로
하되 **수동 검증(체크포인트 2)은 반드시 B 이후**에만 돈다.

**청크 B — 프론트 표면 (med risk: 재조립 회귀).**
6. 사이드바 키잉 reconcile + preview + 집계 dot + 순수 판정 vitest + identity 테스트.
7. 탭바 in-place + pane 배지 + 순수 판정 vitest + identity 테스트.
8. WINDOWS-BUILD.md 체크포인트 2 항목 추가 + 게이트 전체 + change-critic 점검.

## 완료 기준

자동: CLAUDE.md 게이트 5종 green + 신규 테스트 전부. 수동(체크포인트 2, §10 에 추가):
1. `osc-test.sh` 777/9 — 백그라운드 탭 dot·pane 배지·사이드바 상태/미리보기/집계 dot, 표시 중
   탭 unread 미표시.
2. 실 Claude Code + hook 3종 — running/needsInput(+미리보기)/idle, 탭 활성화로 dot 해제.
3. 다른 탭 needsInput 중 딴 탭 running — 사이드바가 needsInput 을 유지 (우선 규칙).
4. 동적 탭 제목 갱신 중 탭 클릭 정상 (ADR-0003 d7 회귀 없음).
5. **[조건부]** `cd` 후 재시작 → 마지막 cwd 재스폰 — ConPTY OSC 7 passthrough 는 미검증
   전제다 (critic 반영): 실패해도 스테이지 블로커가 아니라 title/cwd 범위 축소 + 계획 2장
   파일/소켓 대안 재론으로 처리한다 (알림 경로와 독립).
6. OSC 플러드 — UI 응답성·Saver 주기 정상·RAM 안정.
7. needsInput 탭 닫기(CloseTab·ClosePane 각각) → 사이드바 idle 복귀.
8. 재시작 후 알림·상태 초기화 (sanitize).

## 리스크

- [high] ConPTY 의 OSC 0/2·7 실전 방출 형태 미검증 (777/9 만 spike 검증) — 완화: "2" 별칭 +
  조건부 체크포인트 5 + 실패 시 범위 축소 경로 확정.
- [high] on_osc 핫패스에서 Dispatcher lock 을 잡는 순간 coalescing 의 의미가 무너진다 —
  push 는 merge+notify 만이라는 경계를 리뷰에서 지킬 것.
- [med] A→B 사이 d7 회귀 창 — 수동 검증을 B 뒤로 고정해 봉쇄.
- [med] identity 테스트의 happy-dom 의존 추가 — 실패 시 수동 체크포인트 의존을 명시하고 진행.
