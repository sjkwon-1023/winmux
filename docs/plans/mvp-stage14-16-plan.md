# MVP 14~16단계 실행 계획 — 수명·영속 (teardown 마무리 · persistence · 자동 리셋)

plan-drafter 초안 → plan-critic 반증(revise) → 메인 취합 확정본. 원칙: 신규 로직의
테스트 가능 본체는 wmux-core, src-tauri 글루는 얇은 배선 (글루는 Windows 타깃
check/clippy만이 컴파일 게이트이므로).

## 0. 반증 반영·메인 판정

- **부팅 순서 재설계 (critic high)**: restore→manage가 아니라 **manage 먼저**.
  `Dispatcher::adopt(state)`(스폰 없이 복원 상태 채택) 후 즉시 manage, 그다음
  글루가 탭별로 `respawn_tab(tab)`을 호출(회당 lock 취득·publish_state) —
  재스폰된 세션의 on_exit가 manage 전이라 소실되는 창을 제거한다. 이 창에서
  `get_state`는 pty_session null인 Running 탭을 보여주는데, view-reconcile은
  세션 없는 탭을 attach하지 않으므로 무해하며 스냅샷 도착마다 점진 attach된다.
- **리셋 커맨드의 위치 (메인 판정 — 계획 v2 12장 자구에서 의도적 이탈)**: 코어
  `Command` bus는 구조 변이 전용(ADR-0002)이고 리셋은 상태 무변이·Tauri 의존
  동작이므로 코어 bus에 넣지 않는다. 글루 tauri command `reset_ui` + dev 훅
  (`window.__wmux.resetUi`)로 "UI 버튼 없음·디버깅/향후 MCP 전용" 취지를
  실현한다. ADR 증류 시 이 이탈을 기록한다.
- **활동 신호 (critic 판정 수용)**: write_stdin·send_raw·dispatch만으로는 순수
  열람(스크롤백 읽기)이 idle로 잡혀 "활성 사용 중 절대 금지" 위반. 프론트에
  **throttled 활동 핑**(wheel/mousedown/keydown capture, 10초당 1회
  `user_activity` invoke)을 추가한다. attach/resize/ack은 여전히 활동이 아니다
  (리셋 후 자동 동작의 자기루프 차단). **활동 신호는 idle과 hidden 타이머를 모두
  재무장한다** (Focused(false) 오인 시에도 타이핑 중 발화 불가). 프론트
  `document.visibilitychange`도 보조 신호로 같은 커맨드에 실어 보낸다
  (`user_activity { visible: bool | null }`).
- **persist 세부 (critic 반영)**: tmp는 `state.json`과 **같은 디렉터리**
  (`state.json.tmp-<pid>`) — Windows rename 원자성은 동일 볼륨 전제. Saver 저장
  실패는 loud stderr + 다음 schedule에서 자연 재시도. restore/sanitize 직후
  setup 말미에 `saver.schedule` 1회 (강등 결과가 디스크에 반영). 두 인스턴스
  동시 실행은 MVP 수용·명시: state.json은 마지막 종료 승리, 같은 user data
  folder의 WebView2 브라우저 프로세스 공유 시 워치독 자손 스캔이 0개가 될 수
  있다 — 0개면 loud 로그.
- **next_id 수리는 SplitId 포함** — split 노드 id도 같은 단일 카운터 발급
  (ADR-0003 결정 1)이므로 max 계산에 tree의 split id들을 포함한다.
- **복원 충실도 한계 명시**: 스크롤 위치는 복원하지 않는다(터미널은 replay
  재구성이라 구조적으로 불가, 뷰어 탭은 미구현 — 계획 v2 11장 대비 의도적 제외).
  `tab.cwd`는 생성 시점 값이라 셸 안에서 이동한 최종 cwd는 복원되지 않는다
  (OSC 7 추적은 18단계에서 자연 해소).
- windows-sys는 **직접 의존으로 추가** + features 명시
  (`Win32_System_Diagnostics_ToolHelp`, `Win32_System_ProcessStatus`,
  `Win32_Foundation`) — Cargo.lock의 전이 의존은 feature 활성을 뜻하지 않는다.

## 1. 청크 A — 14단계: replay 트림 + 전환 지연 tracer

- **A-1 replay.rs**: `evicted: bool` (최초 evict 시 true). `snapshot()`은 evicted일
  때만 앞 **4096B 내 첫 `\n`** 뒤부터 반환, 없으면 무트림. `len()`은 보관량 유지
  (스냅샷 길이와 다를 수 있음을 rustdoc 명시). 근거를 rustdoc에: `\n`은 행 경계
  휴리스틱(셸 출력 대상 — OSC/DCS 페이로드 내 `\n`은 이 레포 사용례에서 실질
  무해), 4096 = 행 길이 상한(초과 무개행 = TUI 프레임 → SIGWINCH nudge 수위로
  위임). 기존 escape-cut 허용 문구를 "완화됨(행 경계 시작 보장, TUI는 nudge)"로
  갱신. 프로토콜 무영향: reattach의 dedup은 end_offset 기준이라 head 축소와
  무관, 통합 테스트 (b)의 byte-exact tail assert도 replay.len() 기준이라 성립.
- **테스트 (critic: 우연 정합 방지 — 트림 자체를 잠글 것)**: evict+`\n` 존재 시
  `\n` 뒤 시작 / `\n` 없으면 무트림 / evict 전 무트림 / `\r\n` / `\n`이 끝
  직전일 때 empty 스냅샷 방지 / 4096 상한 초과 위치의 `\n` 무시.
- **A-2 switch-trace.ts** (순수 + vitest): begin(t0=switchWorkspace dispatch) →
  markSnapshot → markAttachStart(tab) → markReplayDone(tab, bytes) → 전체 정착 시
  report `{total, dispatchToSnapshot, perTab, replayBytes}` (완료점은 replay
  write 콜백 + rAF 1회 보정 — 페인트 근사임을 필드명/주석에 명시). 배선:
  main.ts(감지)·store.ts(스냅샷 시각)·workspace-view(ensureView)·terminal-view
  (replay 완료). report는 `console.debug` + `window.__wmux.lastSwitch`. 새 switch
  시작 시 미완 trace 폐기, 터미널 0개 워크스페이스는 렌더 직후 정착.

## 2. 청크 B — 15단계: persistence

- **B-1 core persist.rs**:
  `PersistedState { version: 1, state: AppState }` (camelCase envelope).
  `load(path) -> LoadOutcome { Restored(AppState) | Fresh(NoFile | Corrupt{backup,error} | UnsupportedVersion{found,backup}) }` —
  파싱→구조 검증(불변식의 Result판 신설: leaf 집합==panes 키, split id 유일,
  active 존재)→sanitize(전 terminal 탭 `pty_session := None` 무조건 소거 — 구
  u32 id가 새 레지스트리 id와 충돌하는 사고 차단; `next_id <= max(전 id, split
  포함)`면 max+1 수리+사유). 손상/미지원 버전은 `state.json.corrupt-<epoch>`로
  백업 rename 후 Fresh (loud). `save_atomic(path, state)` — 같은 디렉터리 tmp +
  rename. `Saver::spawn(path, debounce 500ms)` — mpsc worker, 최신값 coalesce,
  `flush()` 동기 기록, 저장 실패 loud+자연 재시도. 크래시 시 ≤500ms 유실 수용
  (rustdoc 명시).
- **B-2 restore 부팅**: core `Dispatcher::adopt(state, host)` +
  `respawn_tab(&mut self, tab: TabId) -> Result<SessionId, RestoreFailure>`
  (Running 터미널 탭 대상 — cwd=tab.cwd.or(ws.root_path), distro=ws.distro,
  80×24; 실패 시 그 탭 `Exited{None}` 강등·revision 증가). **Exited 저장 탭은
  재스폰하지 않는다** (상태 충실 복원 — 메인 결정; 재시작 후 빈 내용 + exited
  배지). 글루 main.rs: load → adopt → **manage** → 탭별 respawn 루프(회당 lock·
  publish) → 초기 saver.schedule → Fresh면 기존 dogfood. main.rs를
  `.build(ctx)` + `App::run(callback)` 구조로 변경 (RunEvent::Exit에서
  saver.flush). 저장 훅: `emit_state_changed`를 `publish_state`로 확장(emit +
  saver.schedule) — dispatch 성공과 sink on_exit 둘 다의 유일 경유점임은 검증됨.
- 테스트: persist round-trip/손상 백업/버전/sanitize/next_id(split 포함)/Saver
  debounce·coalesce·flush; restore — Running만 재스폰·Exited 보존·실패 강등·id
  연속성·adopt 후 respawn 전 상태의 불변식 (FakeHost).

## 3. 청크 C — 16단계: 자동 UI 리셋

- **C-1 core reset.rs** (순수, u64 ms 틱): `ResetConfig { idle_ms, hidden_ms,
  mem_limit_bytes (각 Option=off), mem_poll_ms, safe_idle_ms, cooldown_ms }`,
  `ResetPolicy { on_user_input(now) /* idle+hidden 재무장 */, on_focus(focused,
  now), on_visibility(visible, now), on_mem_sample(bytes, now),
  on_workspace_switch(now) -> Option<ResetTrigger>, next_deadline(now),
  poll(now) -> Option<ResetTrigger> }`. 의미론(계획 v2 12장 원문): idle은 발화
  후 disarm(재발화는 다음 실제 입력 후), hidden은 unfocused/invisible 연속,
  워치독은 pending → 다음 safe 순간(safe_idle 경과 또는 전환 직후), 공통
  cooldown(리셋 후에도 임계 초과 지속 시 cooldown에 막히면 loud 로그로 노출).
  테스트: 1회 발화·재무장 / 활동 중 미발화(핑 포함) / pending→safe 발화 /
  전환 자체 비트리거 / cooldown / 트리거별 off / hidden 재무장.
- **C-2 글루 reset_supervisor.rs**: env 6종(`WMUX_RESET_IDLE_SECS` 기본 1800,
  `WMUX_RESET_HIDDEN_SECS` 600, `WMUX_RESET_MEM_MB` 1536, `WMUX_RESET_MEM_POLL_SECS`
  60, `WMUX_RESET_SAFE_IDLE_SECS` 60, `WMUX_RESET_COOLDOWN_SECS` 300 — 0=off).
  supervisor 스레드(Mutex<ResetPolicy>+Condvar wait_timeout, 신호 시 notify).
  활동: write_stdin·send_raw·dispatch + 신규 `user_activity{visible}` 커맨드
  (프론트 throttled 핑·visibilitychange). `WindowEvent::Focused` → on_focus.
  메모리(#[cfg(windows)]): windows-sys 직접 의존, Toolhelp 자손 중
  msedgewebview2.exe만 PrivateUsage 합산(0개면 loud — 공유 브라우저 프로세스
  케이스), 비Windows 컴파일 배제+1회 로그. `perform_reset` =
  WebviewWindow::reload() + 트리거·수치 loud. `reset_ui` 커맨드(dev 훅 전용
  노출). SwitchWorkspace 성공 시 on_workspace_switch.
- **C-3 프론트**: user-activity 핑(순수 throttle 로직 vitest) +
  `window.__wmux.resetUi`. 문서: 체크포인트 1 체크리스트(WINDOWS-BUILD 신설 절),
  CLAUDE.md 현황.

## 4. 완료 기준

자동 게이트(기존 5종) green + 신규 테스트 전부. **체크포인트 1 (Windows, 사용자)**:
1. 2 워크스페이스·분할·탭·ratio 구성 → 앱 재시작 → 구조 완전 복원 + 터미널마다 새 셸.
2. Exited 탭은 재시작 후 Exited 유지(빈 내용), 앱 정상.
3. state.json 손상 후 재시작 → corrupt 백업 + 새 시작 + stderr 로그.
4. 4-pane 왕복 전환 → `window.__wmux.lastSwitch`로 100ms급 판독.
5. 컬러 출력 1MB+ 홍수 후 이탈→복귀 → replay 상단 깨진 시퀀스 없음.
6. `WMUX_RESET_IDLE_SECS=30` 방치 → 1회 발화·세션/텍스트 보존·비가시적, 재발화
   없음. **리셋 직후 자동 attach가 idle을 재무장하지 않는지**(추가 30초 방치에도
   재발화 1회뿐인지 — cooldown 끄고) 확인.
7. `WMUX_RESET_HIDDEN_SECS=30` 최소화/포커스아웃 → 발화. **포커스 둔 채 타이핑
   중에는 절대 발화하지 않는지**(Focused 오인 판별).
8. `WMUX_RESET_MEM_MB=100` → 타이핑/스크롤 중 미발화, 손 뗀 뒤(safe_idle) 또는
   전환 직후 발화.
9. **스크롤백 열람 중(입력 없이 wheel만) 리셋이 발화하지 않는지** (활동 핑 검증).
10. 작업관리자 강제 종료 → 재시작: 마지막 ≤500ms 변이 유실 외 정상 복원.

## 5. 라우팅

청크 A→B→C 순차(B·C가 main.rs 공유), 각각 워크플로(저비용 모델) + 메인 게이트
재실행 + change-critic. 체크포인트 1 통과 후 ADR 증류(리셋 커맨드 이탈 포함)와
plan 삭제.
