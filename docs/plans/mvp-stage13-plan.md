# MVP 13단계 실행 계획 — 워크스페이스 사이드바

계획 v2 17장 13단계. ADR-0002/0003 아키텍처 위의 UI 스테이지라 plan 리뷰 왕복 없이
메인이 직접 확정 — 구현 후 change-critic 점검은 동일하게 적용한다.

## 0. 설계 결정

- **D1. `CreateWorkspace` 원자 확장** — `{ name, root_path, distro, tab: Option<NewTab> }`.
  tab이 Some이면 spawn-first(실패 시 워크스페이스 미생성·전량 불변), 출력
  `WorkspaceCreated { workspace, pane, tab: Option<TabId>, session: Option<SessionId> }`.
  근거는 ADR-0003 결정 2와 동일(컴포지션 = 원자성 규율 위반 + 중간 스냅샷 렌더).
  부트 dogfood도 단일 dispatch로 단순화된다. UI "새 워크스페이스"는 항상 tab을 싣는다.
- **D2. 워크스페이스 전환의 teardown은 이미 구현돼 있다** — view-reconcile의 "alive
  뷰 ⊆ 활성 워크스페이스 탭" 규칙이 이탈 시 dispose(detach→replay 기록), 복귀 시
  lazy attach를 수행한다 (ADR-0003 follow-up 명시). 13단계는 그 위의 UI만 얹는다.
- **D3. 사이드바 카드** (계획 v2 6장): 이름, 상태 아이콘(agentStatus — 18단계 전까지
  Idle 고정값 표시), 마지막 에이전트 메시지 미리보기(모델값 — null이면 생략),
  브랜치+dirty(`main*` — 19단계 전까지 null이라 생략됨), 경로 축약(`~/...` 스타일),
  pane·탭 수. 포트는 v2 자리 예약이므로 렌더하지 않는다. 활성 카드 하이라이트.
- **D4. 카드 상호작용** — 클릭 = SwitchWorkspace(이미 활성이면 no-op 스킵), X =
  CloseWorkspace(터미널 탭이 1개라도 있으면 `confirm()` — 세션이 전부 죽는 파괴적
  동작), "+ New workspace" = 사이드바 하단 인라인 폼(name 필수, rootPath·distro
  선택 — 절대 Linux 경로 안내 placeholder) → `CreateWorkspace { tab: Terminal }`.
  rename은 v1 nice-to-have로 이번 범위 제외.
- **D5. focus 보상** — switchWorkspace 성공 시 `{ kind: "activePane" }` 요청 추가
  (기존 close 계열과 동일 경로). 새 워크스페이스 생성은 WorkspaceCreated의 tab으로.
- **D6. 레이아웃** — 앱 셸을 좌측 고정폭 사이드바(200px대) + 우측 기존 뷰로. 접기
  토글은 후순위(키보드 모델 20단계와 함께).

## 1. 구현 범위

- 코어: command.rs `CreateWorkspace` 확장 + 테스트(원자성·spawn 실패 불변) +
  fixtures(commands·outputs) + TS 미러 동기 + 부트 dogfood 단일화(main.rs).
- 프론트: `sidebar-model.ts`(+vitest — 카드 사영·경로 축약·상태 매핑),
  `sidebar.ts`(DOM: 카드 리스트·클릭/X·인라인 폼), main.ts 셸 재배치 + compensateFocus
  확장, index.html/styles.css.
- 문서: WINDOWS-BUILD.md 8장 앞에 stage 13 체크리스트 절 추가(생성·전환·전환 시
  세션 유지(백그라운드 워크스페이스 자유 진행 — detach 스윕)·닫기 시 세션 정리·
  리로드 후 워크스페이스 목록/활성 유지·빈 상태), ARM64는 9장으로.

## 2. 완료 기준

자동 게이트 전부 green + 신규 vitest + change-critic 점검 → Windows 체크리스트
통과 시 종료(ADR 증류는 14단계와 묶어 판단).
