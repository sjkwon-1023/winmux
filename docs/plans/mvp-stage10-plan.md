# MVP 10단계 실행 계획 — 데이터 모델 + command dispatcher + 안정 ID

계획 v2 17장 10단계의 확정 실행계획. plan-drafter 초안 → plan-critic 반증(revise) →
메인 취합으로 확정됐다. 초안 뼈대(4청크·spike 동결·ID 체계·attach 프로토콜)는 채택,
반증 findings는 아래 "반영 사항"대로 반영.

## 0. 반증 반영 사항 (초안과 달라진 것)

1. **리더 루프 순서는 현행 유지** — `flow.on_sent`가 lock 안에서 `sink.on_output`보다
   먼저. sink가 `Dropped`를 반환하면 lock 재취득 후 `flow.on_acked(n)`로 **보상 롤백**.
   (초안의 재배열은 ack가 on_sent보다 먼저 도착하는 경합에서 pending 영구 누수 —
   saturating_sub로 ack가 소실되는 래칫이 생긴다.)
2. **WebView 리로드의 10단계 수용 수위 = "세션 생존 + 텍스트 보존"**. TUI 화면 무결
   재구성은 14단계(teardown/재구성) 게이트로 유예 — replay escape 절단(ADR-0001
   follow-up #3)은 그때 해결한다. 단 `attach_terminal` 직후 **resize nudge**(cols-1 →
   cols)로 SIGWINCH 재그리기 유도는 10단계에 포함.
3. **잠금 배치 확정**: `SessionManager`와 sink 채널 레지스트리는 **Dispatcher Mutex
   밖** 자체 동기화. 핫패스(write/ack/resize/attach/출력)는 Dispatcher lock을 절대
   타지 않는다. dispatch(구조 변이)만 Dispatcher lock을 쥐며, 그 안의 스폰까지 보유
   (수십 ms — 핫패스와 무간섭이므로 수용, 주석으로 명시).
4. **spike-plan.md는 삭제하지 않는다** — §4.5(동결 spike 글루 계약)·§6(측정 런북)이
   살아 있는 참조(ADR-0001 재현 하네스). 삭제는 spike 은퇴 시점에 재검토.
5. **미지 세션의 `SessionExited`는 무해한 no-op** — CloseTab이 탭을 먼저 제거한 뒤
   리더 스레드의 on_exit가 도착하는 정상 순서. 에러/패닉 금지 (상태 Mutex poison 방지).
6. **게이트·문서 갱신은 청크 C에서** — apps/wmux의 build+vitest를 게이트에 추가하고
   WINDOWS-BUILD.md에 apps/wmux 빌드·실행 절차를 더해야 C의 완료 기준이 성립.
7. **TS 미러 표류 방지 = golden JSON fixture 공유** — 같은 fixture 파일을 cargo test
   (serde round-trip)와 vitest(타입 파싱)가 함께 소비. Rust 단독 round-trip은 무효.
8. spike의 `ChannelSink`가 send 실패 시 `Dropped`로 바뀌며 "webview 소멸 후 paused
   휴면 → 읽고 버림"으로 동작이 달라진다 — 측정 재현성 각주로 인지.

## 1. 메인 결정 (반증이 판단을 요청한 항목)

- **SplitPane·ClosePane·CloseWorkspace를 모델 레벨에서 완전 구현** (tree 연산 포함).
  11단계는 그 위의 UI 배선으로 좁힌다. 근거: dispatcher 계약을 v2 MCP 노출 전에
  안정화하고, tree 연산은 UI 없이 단위 테스트 가능한 순수 로직이라 10단계 성격에 맞다.
- **Workspace 모델에 `git_branch`/`git_dirty` 필드 지금 포함** (값 채움은 19단계) —
  "타입 공간은 지금 확정" 기준의 일관성 (뷰어 TabKind와 동일 논리).
- **빈 pane 허용은 10단계 임시 상태** — 12단계 pane 정리(collapse) 규칙에서 재결정.
- **스냅샷 전체 emit은 10단계 수용** — 상태가 작고 revision 가드가 있다. 단
  `last_activity`·`agent_status`가 고빈도 변이되는 18단계 진입 전에 coalescing/스로틀
  재설계를 필수 선행 항목으로 기록 (ADR-0002에 명시).
- **프론트는 vanilla TS 유지** — 11~13단계 계획 시 프레임워크 재평가.

## 2. 아키텍처 요지

- 상태는 전부 Rust `wmux-core::model::AppState` 소유 (제약: WebView는 세션 손실 없이
  리로드 가능). 프론트는 뷰 — 부팅 시 `get_state`, 변이마다 `state-changed` 전체
  스냅샷(revision 포함, 낮으면 폐기).
- 안정 ID: `WorkspaceId`/`PaneId`/`TabId` = AppState 단일 카운터의 u64 newtype.
  휘발성 PTY `SessionId`(u32)와 분리 — persistence(15단계)·MCP(v2) 대상은 전자.
- 커맨드는 직렬화 가능한 `Command` enum 단일 bus (`#[serde(tag="type")]`) —
  키보드·마우스·(v2) MCP가 전부 같은 dispatch를 호출.
- 터미널 출력 핫패스: raw channel 유지 + **`[u64 LE offset][bytes]` 프레이밍**.
  `attach_terminal` = sink에 채널 장착 **후** `reattach()` → raw body
  `[u64 LE end_offset][replay bytes]` 반환. 채널 먼저·스냅샷 나중이 불변식 (유실 창
  차단), 겹침은 프론트가 offset < end_offset 폐기로 dedup, **폐기분 포함 전량 ack**
  (flow 계정 일치).

## 3. 청크 (각각 독립 착지 — 게이트 green 후 다음 진행)

### A. 코어 세션 API 재설계 (ADR-0001 follow-up 포함)

- `SessionSink::on_output(&self, offset: u64, bytes: &[u8]) -> Delivery` (Delivered |
  Dropped). Dropped면 리더가 flow 보상 롤백 (위 0-1).
- `PtySession::reattach() -> (u64, Vec<u8>)` — 한 lock 안에서 flow 리셋(+paused 해제,
  리더 깨움)과 (스냅샷 끝 오프셋, replay 스냅샷) 일관 반환.
- `SessionManager::create(spec, opts, make_sink: impl FnOnce(SessionId) -> Box<dyn SessionSink>)`
  — id 선발급. `PtySession::id`(AtomicU32) 삭제.
- spike 글루 이전: `state::Registry` 삭제 → 코어 SessionManager. `ChannelSink`는
  offset 무시, send 실패 시 Dropped (spike 프론트 무변경 — 프레이밍은 새 앱만).
- 테스트: 기존 통합 5종 갱신 + (a) sink factory id 일치 (b) flood→paused에서
  reattach 후 재개·오프셋 연속성 (c) Dropped 시 pending 불증가 + 보상 롤백 검증.

### B. `wmux-core::model` + `wmux-core::command`

- model.rs: AppState/Workspace(+git 필드)/SplitTree/Pane/Tab/TabKind 4종(뷰어 타입
  공간 확정, 생성 커맨드는 21단계)/AgentStatus/NotificationState. serde(camelCase).
- command.rs: `Command` enum (CreateWorkspace/SwitchWorkspace/CloseWorkspace/FocusPane/
  SplitPane/ClosePane/CreateTab(Terminal)/ActivateTab/CloseTab), `SessionHost` 포트
  (spawn_shell/kill — 모델을 PTY 없이 테스트), `Dispatcher`(dispatch → revision+1,
  apply_event(SessionExited — 미지 id no-op), snapshot).
- tree 연산(split/collapse/leaves/불변식) 구현·단위 테스트. wmux-core에 serde 의존
  추가 (ADR-0002에 기록).
- 테스트: FakeSessionHost dispatch 플로, golden JSON fixture round-trip.

### C. `apps/wmux` 신설 (spike는 측정 하네스로 동결 — 기능 동결, 컴파일 유지만)

- 글루: `TauriHost: SessionHost`, `Mutex<Dispatcher>`(구조 변이 전용), 커맨드:
  `dispatch`(spawn_blocking, 성공 시 state-changed emit) / `get_state` /
  `attach_terminal`(채널 먼저·reattach 나중, 직후 resize nudge) / write_stdin·send_raw·
  resize·ack_output·get_stats 이식 (잠금·spawn_blocking 규율 유지). `TerminalSink`:
  `Mutex<Option<Channel>>`, 프레이밍, 실패 시 채널 해제+Dropped. on_exit →
  apply_event+emit.
- 부팅: setup에서 dispatcher로 CreateWorkspace+CreateTab (bus dogfood — 프론트 attach
  전 프롬프트가 replay에 잡히는 게 attach의 자연 검증).
- 프론트(vanilla TS): types.ts(수기 미러 + golden fixture 검증), store.ts(revision
  가드), terminal-view.ts(attach 큐잉·dedup·전량 ack), ack-batcher 재사용. 렌더는
  활성 탭 1개 전면 (분할 렌더는 11단계). 커맨드 트리거는 dev 훅
  `window.__wmux.dispatch` (실 UI는 11~13단계, 외부 호출자는 v2 MCP).
- 게이트에 apps/wmux build+vitest 추가, WINDOWS-BUILD.md에 apps/wmux 절차 추가.
- vitest: store revision 가드, 프레임 파싱, dedup 전량 ack, golden fixture.

### D. 문서 증류

- ADR-0002: 상태 소유·스냅샷 프로토콜(+18단계 확장 절벽과 탈출구)·앱 분리(spike
  동결)·ID 체계·serde 도입·git 필드 선포함 사유.
- CLAUDE.md: Layout·Current state·Gates 현행화. spike-plan.md는 존치 (0-4).

## 4. 완료 기준

- **자동 게이트**: CLAUDE.md 게이트(+apps/wmux build·vitest) green, 청크별 신규
  테스트 green. 각 청크는 green 후에만 착지.
- **수동 검증 (사용자, Windows — C 착지 후)**: ① 부팅 시 터미널 탭 자동 생성·입력
  ② 마커 출력 후 WebView 리로드(F5/dev 훅) → 세션·텍스트 유지 ③ dev 훅으로
  CreateTab/CloseTab/SplitPane → 스냅샷 갱신·고아 프로세스 없음 ④ 리로드 전후
  Pane/Tab id 불변. 발견 문제는 후속 커밋으로 수정.

## 5. 구현 라우팅

청크별로 저비용 모델 워크플로(spike 때와 동일 패턴: 병렬 구현 → 통합 → 게이트 fresh
재검증) + 메인 직접 게이트 재실행 + change-critic 점검. A는 회귀 반경이 코어 전체라
단독 워크플로로 먼저 착지한다.
