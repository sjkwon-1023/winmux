# MVP 11~12단계 실행 계획 — pane 분할 UI + pane 내부 탭 UI

계획 v2 17장 11단계(split tree UI + splitter resize)·12단계(pane 내부 탭)의 확정
실행계획. plan-drafter 초안 → plan-critic 반증(revise) → 메인 취합. ADR-0002 위에
얹는다 — 코어 변경은 "pure UI wiring" 예고에서의 의도적 이탈이며 증류 시 기록한다.

## 0. 확정 설계 결정

- **D1. Split 주소 = `SplitId(u64)`** — `SplitTree::Split`에 안정 id 필드. 경로
  인덱스는 트리 변이 후 다른 노드를 조용히 가리키는 silent misdirection이라 배제.
  단일 카운터(`AppState::next_id`) 발급이라 persistence(15단계) 자동 충족.
  `SplitTree::split()`은 allocator 접근이 없어 id 주입 파라미터로 시그니처 변경.
- **D2. splitter 드래그 = 로컬 CSS 프리뷰, pointerup에 `ResizeSplit` 1회.**
  검증은 모델 개구간(finite, 0<r<1) loud-fail(`CommandError::InvalidRatio`) + UI
  픽셀 클램프 분담. **드래그 활성 가드**: 드래그 중 도착하는 스냅샷(SessionExited
  등도 revision을 올린다)이 해당 split의 프리뷰 ratio를 밟지 않게 억제하고
  pointerup 후 재동기화.
- **D3. 비활성 터미널 탭 = keep-alive** — 탭별 `Map<TabId, TerminalView>`, 비활성은
  `display:none` + 채널 유지·계속 ack. (반증 과정에서 xterm 실소스로 확증: write
  콜백은 setTimeout 파싱 루프라 가시성 무관, 렌더만 IntersectionObserver로 중단·
  재표시 시 full refresh — "렌더링만 중단"은 xterm이 기본 제공.) xterm 인스턴스가
  탭 단위로 늘어나는 메모리 증가는 명시적으로 수용(scrollback 5000×탭 수, 백스톱 =
  계획 v2 12장 WebView 리셋 안전망). 수명 규칙 "alive 뷰 ⊆ 활성 워크스페이스의 탭"
  — 워크스페이스 이탈 시 전 뷰 dispose가 14단계 teardown의 훅.
- **D4. detach는 자동 치유여야 한다 (반증 major 반영).** F5는 dispose를 타지
  않으므로 미방문 탭 세션의 죽은 채널이 Delivered-무ack로 paused에 고착된다.
  두 갈래로 해결: (a) `PtySession::reset_flow()` 신설(flow reset + 리더 wake),
  `detach_terminal` = sink.detach() + reset_flow() — detach된 세션은 어떤 경로로든
  paused에 남지 않는다. (b) 프론트 부트 리컨실 때 attach하지 않는 모든 터미널
  세션에 `detach_terminal` 스윕(멱등).
- **D5. 분할 아이콘 = 원자 `SplitPane { pane, direction, tab: Option<NewTab> }`**
  (반증 판정 반영 — 컴포지션은 원자성 규율 위반 + 중간 스냅샷 1프레임 렌더).
  spawn-first 순서 재사용: spawn 실패 시 트리 불변. 출력은
  `PaneCreated { pane, split, tab: Option<TabId>, session: Option<SessionId> }` —
  생성된 안정 ID 전부 반환(CommandOutput 계약 의도).
- **D6. pane 정리 = CloseTab auto-collapse.** 마지막 탭이 닫혀 pane이 비면 collapse
  (ClosePane의 공유 헬퍼로 추출 — **active_pane fixup 포함**). 워크스페이스 마지막
  pane은 예외로 빈 pane + placeholder. 원자 SplitPane 채택으로 빈 pane 발생 경로가
  이 예외 하나로 줄어 규칙이 단순해진다. `ClosePane` 커맨드는 dev 훅·MCP용 존치.
- **D7. ResizeObserver는 pane당 1개**(PaneView 소유), TerminalView의 뷰당 observer
  제거. attach 말미 자동 `term.focus()` 제거 — **보상 경로 명시**: 부트 리컨실 후
  활성 pane의 뷰 1곳, CreateTab/SplitPane(tab 포함) 성공 직후 새 뷰,
  ActivateTab/FocusPane 클릭 직후 해당 뷰에 명시적 focus.

## 1. 청크

### A. 코어 계약 + detach 치유 (Rust + fixture + TS 미러 동기)

- model.rs: `SplitId` newtype, `Split { id, ... }`, `split(target, direction,
  new_pane, split_id)` 시그니처 변경, 불변식에 split id 유일 추가, 트리 테스트 갱신,
  스테일 주석 정리("resize는 12단계" 등 → 11~12단계 반영).
- command.rs: `ResizeSplit { split, ratio }`(+`InvalidRatio`), `SplitPane`에
  `tab: Option<NewTab>`(spawn-first 원자), `PaneCreated { pane, split, tab, session }`,
  CloseTab auto-collapse(공유 헬퍼 + fixup + rustdoc + 테스트: multi-pane 마지막 탭
  닫기 → collapse·세션 kill·fixup / 비활성 pane collapse / 단일 pane 예외 /
  ResizeSplit 성공·스테일 id·InvalidRatio(NaN·0·1)·실패 시 revision 불변).
- session.rs: `PtySession::reset_flow()` (rustdoc: detach 자동 치유 근거).
- apps/wmux 글루: `detach_terminal` = detach + reset_flow.
- fixtures: snapshot(split id 추가), commands(resizeSplit + splitPane tab 형태),
  outputs(PaneCreated 새 형태 + InvalidRatio), dispatcher.rs·types.ts·types.test.ts
  동기 (Rust/TS 같은 파일 잠금 유지).

### B. 11단계 UI — 분할 렌더·splitter·포커스·에러 표면화

- 순수 모듈(vitest는 DOM-free): `split-layout.ts`(structureKey/ratioFromPointer
  클램프/flexPair), `command-error.ts`(formatCommandError).
- `workspace-view.ts`: structureKey 동일하면 ratio만 in-place, 다르면 재구축하되
  pane 엘리먼트는 레지스트리 재사용(reparent — 현 렌더러 DOM이라 리스크 낮음,
  WebGL 활성화 시 재평가). split 컨테이너는 SplitId 키잉.
- `pane-view.ts`: 헤더 아이콘 4개(새 터미널 탭·브라우저 disabled·좌우/상하 분할 —
  분할은 D5 원자 커맨드), mousedown capture로 FocusPane, 활성 테두리. 이 청크에서
  pane 콘텐츠는 활성 탭 1개 dispose+reattach 임시 렌더(C에서 keep-alive 교체).
- `splitter.ts`: 4px 핸들, setPointerCapture, 로컬 flex-grow 프리뷰 + 드래그 가드
  (D2), pointerup에 dispatch 1회.
- `main.ts`: `dispatchUI` 래퍼 — CommandError를 상태 라인에 표시(one-shot 소거,
  폴링 금지). 드래그 중 fit은 rAF 코얼레싱, 잡음 시 pointerup 1회로 후퇴.

### C. 12단계 UI — 탭바·keep-alive·pane 정리 UX

- 순수 모듈: `tab-strip-model.ts`(탭 버튼 모델: title·active·exited 배지·알림 dot),
  `view-reconcile.ts`(planViewSync: alive/visible/dispose 판정 — D3 수명 규칙).
- `terminal-view.ts` 리팩터: observer·fitScheduled·자동 focus 제거, `setVisible()`·
  `scheduleFit()`·`focus()` 공개.
- `pane-view.ts`: 탭바(클릭 = 비활성 pane이면 FocusPane 후 ActivateTab, X =
  CloseTab), keep-alive 레지스트리(첫 가시화 때 lazy attach), pane당 observer 1개.
- 부트 리컨실: attach하지 않는 전 터미널 세션 `detach_terminal` 스윕(D4-b) + 활성
  pane 뷰 focus(D7).
- 빈 pane placeholder(마지막 pane 예외), Exited 배지.

### D. 문서

- WINDOWS-BUILD.md: 11~12단계 수동 체크리스트를 **새 절로 분리**(6장은 stage-10
  종료 기록으로 보존하되 5항의 "dispose 시 detach" 전제 문구를 keep-alive 이후
  실상으로 갱신). 신규 항목: 분할·중첩 분할, 드래그 + F5 후 ratio 생존, 탭
  생성/전환/닫기(전환 시 replay 플래시 없음 = keep-alive 증거), 숨은 탭 `yes` 자유
  실행 + get_stats paused:false, **리로드 후 미방문 탭 자유 실행**(D4 검증), 마지막
  탭 닫기 collapse / 마지막 pane placeholder, 2×2 리로드 생존, 스테일 id resize
  에러 표면화, 4-pane RAM 참고 측정.
- 검증 통과 후: ADR-0003 증류(SplitId·keep-alive·auto-collapse·detach 치유,
  "pure UI wiring" 이탈 기록) + 이 plan·stage10-plan 삭제.

## 2. 완료 기준

- 자동: 기존 게이트 전부 + 신규 vitest(클램프 경계·structureKey·reconcile·탭 모델·
  에러 포맷) green. 청크별 착지.
- 수동(사용자, Windows): 위 D절 체크리스트. 통과가 11~12단계 종료 조건.

## 3. 구현 라우팅

청크별 저비용 모델 워크플로(구현 → 통합 → 게이트 fresh) + 메인 게이트 재실행 +
change-critic 점검 — 10단계와 동일 패턴.
