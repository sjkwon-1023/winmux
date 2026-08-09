# Stage 21 실행계획 — folderBrowser / textViewer / markdownViewer 뷰어 탭

> plan-drafter 초안 → plan-critic 반증 → 메인 취합 확정본 (2026-08-09). MVP 마지막
> 스테이지 — 착지 후 체크포인트 2 수동 검증.
> 근거: 터미널-계획-v2.md "탭 타입별 동작"(Rust 가 \\wsl.localhost UNC 로 접근; 9P 한계 —
> 마크다운 라이브 리로드는 활성 탭 한정 mtime 폴링; textViewer 청크 로딩·가상 스크롤 필수;
> **뷰어이지 에디터가 아니다**; 비활성 뷰어 탭은 상태만 남기고 DOM unmount), 4장(파일 열기
> 포함 모든 조작은 단일 dispatcher 경유), 5장(Windows→WSL 방향이라 잠근 배포판에서도 동작),
> "상태 저장" 장(재실행 시 뷰어 재로드). 14장 목록에서 **markdownViewer 만 "(옵션 기능)"**
> 표기 — folderBrowser·textViewer 는 이번에 완성, markdownViewer 는 절단 가능한 마지막 청크.

## 핵심 결정 (critic 반영)

- **distro 해석 (critic high)**: 인자(workspace.distro) → env `WINMUX_DISTRO` →
  **`wsl.exe -l -q` 기본 배포판 lazy 질의 + 프로세스 수명 캐시** → 전부 실패 시에만 loud
  에러. 터미널 스폰(host.rs — 없음 허용)과 정합: 가장 흔한 구성(둘 다 미설정)에서 뷰어가
  죽는 비대칭을 만들지 않는다. `wsl.exe -l -q` 출력은 UTF-16LE — 디코드 후 첫 비어있지
  않은 줄. 질의 실패 에러 메시지에 "set workspace distro or WINMUX_DISTRO" 안내 포함.
- **MarkdownViewer 는 타입부터 청크 D (critic high)**: 청크 A 의 `NewTab` 추가분은
  FolderBrowser·TextViewer 2종만. D 절단 시에도 dispatcher 표면에 미구현 경로가 남지
  않는다 (command.rs 의 "variant 생략 = 타입 수준 부재" 원칙).
- 경로 검증·UNC 매핑 순수 함수는 **winmux-core 신규 `wslpath.rs`** — 게이트가
  `cargo test -p winmux-core` 만 돌리므로 글루에 두면 어떤 게이트로도 테스트가 안 돈다
  (critic 정정: "src-tauri 는 Linux 컴파일 불가"가 아니라 이것이 정확한 근거).
- 디렉터리 탐색은 뷰 내부 상태가 아니라 **dispatcher 명령** (계획 4장 명문 + persist
  최신성). 파일 내용 읽기(fs_*)는 attach_terminal 류 콘텐츠 플레인 직접 invoke 전례와
  동형 — 4장 위반 아님.
- 뷰어 수명은 기존 `planViewSync`(터미널 keep-alive) **무변경**, 반대 시맨틱의 신규 순수
  함수 `planViewerSync` + 병렬 레지스트리로 공존.

## core 계약 (crates/winmux-core)

```rust
pub enum NewTab {
    Terminal { cwd: Option<String> },
    /// None → workspace root_path (root 도 None 이면 "/"). Terminal cwd 와 대칭.
    FolderBrowser { path: Option<String> },
    TextViewer { path: String },
    // MarkdownViewer { path: String }  ← 청크 D 에서 추가
}
pub enum Command {
    /// folderBrowser 경로 변경 — 탐색도 dispatcher 경유. Tab.title 도 basename 갱신.
    NavigateFolder { tab: TabId, path: String },
    /// 뷰어 스크롤 기록 (unmount 복원·persist). textViewer = 최상단 가시 행의 전역
    /// byte offset, markdownViewer(D) = 렌더 px — TabKind rustdoc 에 시맨틱 명문.
    /// FolderBrowser 대상은 KindMismatch (모델에 scroll_top 없음 — 기결정).
    SetViewerScroll { tab: TabId, scroll_top: f64 },
}
pub enum CommandError {
    KindMismatch { tab: TabId },          // Navigate→folderBrowser 만, SetScroll→뷰어만
    InvalidPath { message: String },      // wslpath::validate 공유
    InvalidScroll { value: f64 },         // finite·≥0 아님 (InvalidRatio 전례)
}
```

- 뷰어 탭 생성은 spawn 없는 순수 변이 — CreateTab/SplitPane/CreateWorkspace 3개 매치
  지점에 `viewer_tab()` 헬퍼 분기 (CreateTab 의 irrefutable let 은 match 로 전환).
  `TabCreated{session: None}` 기예약이라 출력 타입 무변경. CreateWorkspace 도 뷰어를
  받는다 (NewTab 공유 타입 일관성) — `WorkspaceCreated`/`PaneCreated` 의 "tab Some ⇔
  session Some" 불변식 주석·TS 주석을 "terminal 탭일 때만 session Some" 으로 갱신 +
  fixture 케이스 (critic low-med).
- 생성·Navigate 시 `wslpath::validate_linux_path` 형태 검증만 (코어 무 I/O). 실존 여부는
  뷰 로드 실패로 표면화.

**신규 `wslpath.rs`** (순수):
```rust
pub fn validate_linux_path(path: &str) -> Result<(), String>;
pub fn to_unc(distro: &str, linux_path: &str) -> Result<String, String>;
```
거부 규칙: 비절대, NUL, `\`(UNC 구분자 밀수), `.`/`..` 컴포넌트, **`:` 포함 컴포넌트**
(Windows ADS 해석), **후행 점·공백 컴포넌트**(Win32 절삭 alias — critic low). 빈 세그먼트
정규화. distro 도 검증('/'·'\'·NUL·빈 문자열 금지). ~247자 초과 UNC 경로의 실패 가능성은
알려진 한계로 rustdoc 에 기록 (verbatim `\\?\UNC` 미채택 — MVP). 심볼릭 링크는 9P 가
해석 — 읽기 전용 + 본인 머신이라 탈출 위협 모델이 아님을 rustdoc 에 명시.

## glue 계약 (apps/winmux/src-tauri) — 전부 spawn_blocking, `Result<T, String>` 관례

```rust
fs_list_dir(distro: Option<String>, path: String) -> Result<DirListing, String>
  // DirListing { entries: Vec<DirEntry{ name, is_dir, size: Option<u64> }>, truncated: bool }
  // 5,000 entry 상한 → truncated. 정렬은 프론트 순수 함수 (vitest).
fs_stat(distro: Option<String>, path: String) -> Result<FileStat, String>
  // { size: u64, mtime_ms: u64, is_dir: bool }
fs_read_chunk(distro: Option<String>, path: String, offset: u64, len: u32)
  -> Result<tauri::ipc::Response, String>   // raw bytes — attach_terminal 전례 (critic 확인:
                                            // base64 fallback 불필요). len > 4MiB loud 거부.
```
- distro 해석은 위 "핵심 결정" 순서. `#[cfg(not(windows))]` 는 Linux 경로 직사용 (Unix
  dev 실행 대칭).

## 프론트 계약 (apps/winmux/src)

- `planViewerSync(aliveViewerTabs, snapshot) -> { mount: VisibleViewer[], dispose: ViewerDispose[] }`
  — mount = 활성 워크스페이스 각 pane 의 active 뷰어 탭만, dispose = 그 외 전부.
  **dispose 항목에 탭 실존 여부 플래그** (critic med): 탭이 스냅샷에 남아 있는
  unmount 면 dispose 전 `flushScroll`, 탭 소멸(CloseTab 등)이면 flush 스킵 —
  UnknownTarget 잡음 방지.
- `ViewerView { root; update(kind); flushScroll(); focus(); dispose() }` — folder-view.ts /
  text-view.ts (D: markdown-view.ts). **textViewer 는 자체 ResizeObserver** 를 root 에
  설치 (critic med — pane 의 observer 는 터미널 fit 전용 유지): 리사이즈 시 슬라이스
  재계산.
- workspace-view: `viewerViews: Map<TabId, ViewerView>` 병렬 레지스트리 (기존 `views` 는
  TerminalView 전용 유지), render 에서 planViewerSync 집행, **focusTarget 을
  TerminalView | ViewerView 로 확장** (둘 다 focus() — 20단계 키보드 내비·D7 보상이
  뷰어 탭에서도 성립, critic med).
- pane-view: update 에 visibleViewer 인자 추가 — placeholder 는 **terminal 도 viewer 도
  없을 때만** (critic med: 동시 표시 방지). shown 시맨틱: shownTab = 표시 중 탭
  (terminal 이든 viewer 든) — send-mode resolveSend 의 기존 "비-terminal 대상 에러"
  경로는 그대로 성립 (뷰어 탭이 shown 이면 TerminalView 레지스트리 미스 → 에러).
  헤더에 폴더 버튼 신설 (`createTab{type:"folderBrowser",path:null}`) — 기존 ◎ browser
  버튼은 v2 예약이라 불변.
- folderBrowser: dirs-first·name asc 순수 정렬 + `..` 행(path≠"/") + 디렉터리 클릭 =
  `navigateFolder`, 파일 클릭 = `createTab`(textViewer; D 착지 후 .md/.markdown 만
  markdownViewer 로 라우팅). truncated 배너. 로드 실패는 인라인 에러 (탭 유지).
- textViewer: 512KiB 바이트 윈도우 `fs_read_chunk` 로드, 선두/말미 부분행·UTF-8 파단
  절삭, no-wrap 고정 행높이 실스크롤 + spacer + viewport±20행 슬라이스 (순수 계산 fn —
  vitest). 파일 > 윈도우: byte 범위 표시 + 처음/끝/이전/다음 버튼 (자동 이어읽기 없음 —
  메모리 상주 = 윈도우 1개 고정).
- **scroll 왕복 + 에코 가드 (초안 high 리스크)**: settle 500ms 디바운스로
  `setViewerScroll` dispatch, unmount(탭 존속) 시 flush. 스냅샷의 scrollTop 은 **마운트
  시 1회만 적용** — mounted && path 동일이면 재적용 금지 (없으면 매 dispatch 재렌더가
  사용자 스크롤과 싸운다). vitest 로 고정.

## 청크 D — markdownViewer (옵션 — 절단 가능)

`NewTab::MarkdownViewer` variant + TS + fixture + 확장자 라우팅을 **이 청크에서** 추가.
`marked` 의존 (pure JS·zero-dep — ARM64 무관). `fs_stat` > 2MiB 렌더 거부 + "open as
text" 안내. **raw HTML 전부 escape** (renderer override — IPC 를 쥔 WebView 라 파일발
HTML 주입 차단은 보안 필수, 옵션 아님). 링크 클릭 무동작, 이미지 placeholder. 라이브
리로드: 주입형 setTimeout 체인 2초 mtime 폴링 (레포 전례 — setInterval 전무),
`document.hidden` 정지, 폴링 수명 = 뷰 수명 (mount ⇒ active 라 별도 게이팅 불요).
scrollTop 은 px 시맨틱으로 C 인프라 재사용.

## 실행 청크 (순차)

- **A — 코어 계약**: NewTab 2종 + NavigateFolder/SetViewerScroll + CommandError 3종 +
  wslpath.rs + types.ts 동기화 + dispatcher 테스트(원자 생성·title 갱신·KindMismatch —
  FolderBrowser 에 SetScroll 포함·InvalidPath·InvalidScroll·persist 왕복) + fixture 갱신
  은 **commands/outputs/snapshot 3개** (critic 정정 — snapshot-empty 무관; 신규 케이스
  추가를 수용 기준에 명시). 착지 후 UI 는 기존 placeholder 경로로 무변화.
- **B — 글루**: fs_* 3종 + distro 해석(기본 배포판 질의+캐시) + cfg 분기 +
  backend.ts 래퍼 3종. Windows 실동작은 체크포인트 2 이월.
- **C1 — 수명 + folderBrowser**: planViewerSync + viewerViews 레지스트리 + focusTarget
  확장 + pane-view seam(placeholder·폴더 버튼) + folder-view.ts + 정렬 순수 fn + vitest.
- **C2 — textViewer**: text-view.ts (윈도우 로드·슬라이스·ResizeObserver·윈도우 이동
  버튼) + scroll 왕복·에코 가드 + vitest (슬라이스 계산·부분행 절삭·에코 가드·
  planViewerSync flush 판정).
- **D — markdownViewer**: 위 절 전부 + vitest (escape 설정·폴링 상태기계 — 타이머 주입).

각 청크 착지마다 게이트 5종 green. 커밋은 A+B(코어·글루) / C1+C2(프론트) / D 3회.

## 완료 기준

자동: 게이트 5종 + 신규 vitest·cargo 테스트. **수동 검증은 전부 Windows 전용(UNC·9P)이라
에이전트 실행 불가 — 체크포인트 2 사용자 체크리스트로 분리** (critic med):
1. 폴더 버튼 → workspace root folderBrowser, dirs-first 정렬.
2. 하위/`..` 탐색, 앱 재시작 후 경로 복원 (fresh 재목록).
3. 수백 MB 로그 → textViewer 즉시 열림, private working set +20MB 미만, 윈도우 이동 동작.
4. 중간 스크롤 → 탭 이탈/복귀(unmount/remount) 같은 지점, 재시작 후에도 같은 지점.
5. 비활성 뷰어 탭 DOM 부재 (devtools — unmount 계약).
6. 없는/삭제 파일 → 인라인 에러, 탭 유지.
7. [D] .md 렌더 + WSL 쪽 append 후 활성 탭 2~4초 내 재렌더, 비활성 시 폴링 0.
8. [D] raw HTML/script escape 표시, 링크 무동작.
9. 격리 배포판(automount/interop off)에서 뷰어 정상 (계획 5장 방향 검증).
10. 편집 affordance 전무. 11. 터미널↔뷰어 전환 시 keep-alive 유지 (re-replay 없음).
12. distro 미설정 구성에서 뷰어 동작 (기본 배포판 자동 해석).

## 리스크

- [high] scroll 에코 가드 부재 시 스크롤 클로버 — C2 에서 가드 + vitest 필수.
- [med] `wsl.exe -l -q` UTF-16 파싱·질의 실패 경로 — B 에서 실패 시 loud 에러로 한정,
  체크포인트 항목 12 로 실검증.
- [med] 글루는 구조적으로 유닛테스트 밖 (게이트가 core 테스트만) — 순수 로직의 core
  배치로 완화, 잔여는 체크포인트 2.
- [med] 9P 대형 디렉터리 지연 — spawn_blocking + 로딩 표시 + 5,000 상한.
- [low] 디바운스 창 내 종료 시 최종 스크롤 500ms 유실 — persist 전반의 기존 트레이드
  오프와 동일, 수용.
- [low] `\`·`:`·후행 점/공백 파일명 접근 불가 — 밀수 차단의 의도된 희생, 에러에 사유.
