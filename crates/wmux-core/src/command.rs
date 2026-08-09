//! Command dispatcher — 모든 구조 변이의 단일 진입점 (계획 v2 4장, 10단계 계획 3-B).
//!
//! 키보드·마우스·(v2) MCP 가 전부 같은 [`Command`] bus 로 [`Dispatcher::dispatch`] 를
//! 호출한다. PTY 부수효과는 [`SessionHost`] 포트로 격리해 모델을 PTY·Tauri 없이
//! 테스트한다 — 실제 앱 글루가 `SessionManager` 위에 이 포트를 구현한다.
//!
//! # 의미론 요약
//!
//! - 대상 id(pane/tab/workspace) 탐색은 **전 워크스페이스 범위** — 안정 ID 는 단일
//!   카운터 발급이라 전역 유일하다. 미지 id 는 [`CommandError::UnknownTarget`].
//! - 성공한 dispatch 마다 `revision += 1`. 실패 시 상태·revision 불변 (원자성 —
//!   각 핸들러는 검증을 전부 마친 뒤에만 변이하고, spawn 은 탭 추가 전에 수행).
//! - [`Dispatcher::apply_event`] 의 미지 session 은 무해한 no-op (10단계 계획 0-5 —
//!   CloseTab 이 탭을 먼저 제거한 뒤 리더 스레드의 exit 통지가 도착하는 정상 순서).
//!   상태가 실제로 바뀔 때만 revision 이 증가한다.
//! - [`Dispatcher::apply_osc`] 는 flush 창 하나에 모인 OSC 델타를 통째로 반영하고
//!   revision 을 **배치당 최대 1회** 올린다 (18단계 계획 core 계약 — OSC 플러드가
//!   스냅샷 발행을 이벤트 수만큼 유발하지 않게 하는 coalescing 의 착지점).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{
    AgentStatus, AppState, NotificationState, Pane, PaneId, SplitDirection, SplitId, SplitTree,
    Tab, TabId, TabKind, TerminalStatus, Workspace, WorkspaceId,
};
use crate::notify::{OscBatch, OscDelta};
use crate::session::SessionId;

/// 직렬화 가능한 command bus 의 명령 집합.
///
/// JSON 은 internal tag: `{"type": "createWorkspace", "name": ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    /// 워크스페이스 + pane 1개(Leaf)를 만들고 active 로 지정한다. `tab` 이
    /// Some 이면 초기 pane 에 그 탭까지 **원자적으로** 생성한다 (계획 13-D1 —
    /// CreateTab 과 동일한 spawn-first 순서라 스폰 실패 시 워크스페이스·pane·
    /// next_id 전부 불변이고, 빈 워크스페이스 중간 상태가 스냅샷에 노출되지
    /// 않는다). None 이면 기존처럼 빈 pane 을 만든다 — 필드 누락 JSON 도
    /// None 으로 파싱된다 (13단계 이전 클라이언트 하위호환).
    CreateWorkspace {
        name: String,
        root_path: Option<String>,
        distro: Option<String>,
        tab: Option<NewTab>,
    },
    SwitchWorkspace {
        workspace: WorkspaceId,
    },
    /// 소속 terminal 세션을 전부 kill 하고 제거한다. 마지막 워크스페이스도 닫을 수
    /// 있다 (active_workspace 가 None 이 된다).
    CloseWorkspace {
        workspace: WorkspaceId,
    },
    /// 소속 워크스페이스의 active_pane 을 바꾼다. active_workspace 는 바꾸지
    /// 않는다 — 워크스페이스 전환은 SwitchWorkspace 로 명시한다 (명령 직교성).
    FocusPane {
        pane: PaneId,
    },
    /// `pane` 의 leaf 를 분할해 새 pane 을 second(우/하)로 만들고 포커스를 새
    /// pane 으로 옮긴다. `tab` 이 Some 이면 새 pane 에 그 탭까지 **원자적으로**
    /// 생성한다 (계획 D5 — CreateTab 과 동일한 spawn-first 순서라 스폰 실패 시
    /// 트리·panes 불변이고, 분할만 된 중간 상태가 스냅샷에 노출되지 않는다).
    /// None 이면 기존처럼 빈 pane 을 만든다 (dev 훅·MCP 용).
    SplitPane {
        pane: PaneId,
        direction: SplitDirection,
        tab: Option<NewTab>,
    },
    /// `split` 노드의 ratio 를 갱신한다. ratio 는 finite 하고 개구간 (0, 1) 안이
    /// 어야 한다 — 아니면 [`CommandError::InvalidRatio`] (검증은 모델이 loud 하게,
    /// 픽셀 클램프는 UI 가 분담 — 계획 D2). 스테일 split id 는 UnknownTarget.
    ResizeSplit {
        split: SplitId,
        ratio: f64,
    },
    /// 소속 세션 kill + tree collapse. 워크스페이스의 마지막 pane 은 닫을 수 없다
    /// ([`CommandError::LastPane`]). 12단계 UI 의 pane 정리는 CloseTab
    /// auto-collapse 가 담당하고, 이 커맨드는 dev 훅·MCP 용으로 존치한다 (계획 D6).
    ClosePane {
        pane: PaneId,
    },
    CreateTab {
        pane: PaneId,
        tab: NewTab,
    },
    ActivateTab {
        tab: TabId,
    },
    /// terminal 탭이면 세션 kill. active_tab 이었다면 직전 탭으로 조정 (첫 탭이었
    /// 으면 다음 탭).
    ///
    /// **auto-collapse 규칙 (계획 D6)**: 마지막 탭이 닫혀 pane 이 비면, 그 pane
    /// 이 워크스페이스의 마지막 pane 이 아닌 한 pane 자체를 collapse 한다 (tree
    /// remove + panes remove + active_pane fixup — ClosePane 과 공유 헬퍼).
    /// 워크스페이스의 마지막 pane 은 예외로 빈 pane(active_tab = None)으로 남는다.
    CloseTab {
        tab: TabId,
    },
    /// folderBrowser 탭의 경로를 바꾼다 — 디렉터리 탐색도 뷰 내부 상태가 아니라
    /// dispatcher 를 경유한다 (계획 v2 4장 + persist 최신성, 21단계). 대상이
    /// folderBrowser 가 아니면 [`CommandError::KindMismatch`], 경로 형태가
    /// 불량하면 [`CommandError::InvalidPath`] (실존 여부는 검사하지 않는다 —
    /// 코어 무 I/O). 성공 시 `Tab.title` 도 새 경로의 basename 으로 갱신된다.
    NavigateFolder {
        tab: TabId,
        path: String,
    },
    /// 뷰어 스크롤 위치를 기록한다 — unmount 복원·persist 용 (21단계). 값 시맨틱은
    /// 탭 종류별로 다르다 ([`TabKind`] rustdoc 참조): textViewer 는 최상단 가시
    /// 행의 전역 byte offset, markdownViewer 는 렌더 컨테이너의 **px** 다. 코어는
    /// 둘을 구분하지 않고 f64 를 그대로 보관한다 — 해석은 그 종류의 뷰 몫이다.
    /// finite·0 이상이어야 하고 (아니면 [`CommandError::InvalidScroll`]), 대상은
    /// 스크롤 위치를 모델에 가진 뷰어 탭(textViewer·markdownViewer)이어야 한다 —
    /// folderBrowser 는 그 필드가 없으므로 [`CommandError::KindMismatch`]
    /// (기결정).
    SetViewerScroll {
        tab: TabId,
        scroll_top: f64,
    },
}

/// 탭 생성 명세 — CreateTab·SplitPane·CreateWorkspace 가 공유한다. 21단계 뷰어
/// 3종이 모두 착지해 [`TabKind`] 와 종류가 일대일이다 (terminal 은 스폰을
/// 동반하고, 뷰어 3종은 순수 변이다).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NewTab {
    Terminal {
        /// None 이면 워크스페이스 root_path 를 기본 cwd 로 쓴다 (계획 v2 4장).
        cwd: Option<String>,
    },
    /// 디렉터리 탐색 탭. spawn 이 없는 순수 변이로 생성된다.
    FolderBrowser {
        /// None 이면 워크스페이스 root_path, 그것도 None 이면 `"/"` (Terminal
        /// cwd 와 대칭 — 계획 21단계 core 계약).
        path: Option<String>,
    },
    /// 텍스트 파일 뷰어 탭 (에디터가 아니다 — 읽기 전용). 경로는 필수다.
    TextViewer {
        path: String,
    },
    /// 마크다운 렌더 뷰어 탭 (21단계 청크 D). TextViewer 와 같은 파일을 다른
    /// 방식으로 볼 뿐이라 계약은 동일하다 — 읽기 전용, 경로 필수. 스크롤
    /// 시맨틱만 다르다 (렌더된 px — [`TabKind`] rustdoc).
    MarkdownViewer {
        path: String,
    },
}

/// dispatch 성공 결과. 생성된 안정 ID 를 돌려줘 dev 훅·MCP 가 후속 조작에 쓴다.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CommandOutput {
    /// CreateWorkspace 결과 — 생성된 안정 ID 전부를 돌려준다 (계획 13-D1).
    /// `tab` 은 `CreateWorkspace.tab` 이 Some 이었을 때만 Some 이고, `session`
    /// 은 그 탭이 **terminal 일 때만** Some 이다 (뷰어 탭은 스폰이 없다 — 21단계).
    WorkspaceCreated {
        workspace: WorkspaceId,
        pane: PaneId,
        tab: Option<TabId>,
        session: Option<SessionId>,
    },
    /// SplitPane 결과 — 생성된 안정 ID 전부를 돌려준다 (계획 D5). `tab` 은
    /// `SplitPane.tab` 이 Some 이었을 때만 Some 이고, `session` 은 그 탭이
    /// **terminal 일 때만** Some 이다 (뷰어 탭은 스폰이 없다 — 21단계).
    PaneCreated {
        pane: PaneId,
        split: SplitId,
        tab: Option<TabId>,
        session: Option<SessionId>,
    },
    TabCreated {
        tab: TabId,
        /// terminal 탭이면 스폰된 PTY 세션 id, 뷰어 탭이면 None (스폰 없는 순수
        /// 변이 — 21단계).
        session: Option<SessionId>,
    },
    /// id 를 새로 만들지 않는 명령의 성공.
    Done,
}

/// dispatch 실패. 상태는 변하지 않았음이 보장된다.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CommandError {
    /// 대상 id 가 전 워크스페이스 어디에도 없다.
    UnknownTarget { target: String },
    /// 워크스페이스의 마지막 pane 은 닫을 수 없다.
    LastPane,
    /// 셸 스폰 실패 — 탭은 추가되지 않았다 (spawn 이 탭 추가보다 먼저).
    SpawnFailed { message: String },
    /// ResizeSplit 의 ratio 가 유효 범위 밖 — finite 하고 개구간 (0, 1) 안이어야
    /// 한다 (계획 D2 — 모델은 loud-fail, 픽셀 클램프는 UI 분담).
    InvalidRatio { ratio: f64 },
    /// 대상 탭의 종류가 이 명령을 받을 수 없다 — NavigateFolder 는
    /// folderBrowser 만, SetViewerScroll 은 스크롤 위치를 모델에 가진 뷰어만
    /// 받는다 (21단계).
    KindMismatch { tab: TabId },
    /// 뷰어 경로의 형태가 불량하다 — 사유는 `wslpath::validate_linux_path` 의
    /// 문자열을 그대로 싣는다 (21단계). 실존 여부와는 무관하다 (코어 무 I/O).
    InvalidPath { message: String },
    /// SetViewerScroll 의 scroll_top 이 finite·0 이상이 아니다 (InvalidRatio 와
    /// 같은 loud-fail 방침).
    InvalidScroll { value: f64 },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::UnknownTarget { target } => {
                write!(f, "unknown target: {target}")
            }
            CommandError::LastPane => {
                write!(f, "cannot close the last pane of a workspace")
            }
            CommandError::SpawnFailed { message } => {
                write!(f, "shell spawn failed: {message}")
            }
            CommandError::InvalidRatio { ratio } => {
                write!(
                    f,
                    "invalid split ratio {ratio}: must be finite and in (0, 1)"
                )
            }
            CommandError::KindMismatch { tab } => {
                write!(f, "tab {} has the wrong kind for this command", tab.0)
            }
            CommandError::InvalidPath { message } => {
                write!(f, "invalid path: {message}")
            }
            CommandError::InvalidScroll { value } => {
                write!(
                    f,
                    "invalid scroll offset {value}: must be finite and >= 0"
                )
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// PTY 쪽에서 dispatcher 로 흘러오는 세션 이벤트.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    SessionExited {
        session: SessionId,
        code: Option<u32>,
    },
}

/// 셸 스폰 요청. cols/rows 는 기본 80×24 — 실측 resize 는 attach 후 프론트가
/// 수행한다 (10단계 계획 2장 attach 프로토콜).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellSpawnReq {
    pub cwd: Option<String>,
    pub distro: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// 이 셸이 쓸 **탭별 명령 history** 의 키 — 이 세션이 실릴 터미널 탭의 안정
    /// ID 다 (체크포인트 2 UX 요청). 안정 ID 는 재시작을 넘어 유지되므로 글루가
    /// 탭마다 다른 HISTFILE 을 물려 주면 재시작 후에도 같은 탭의 history 만
    /// 복원된다. 코어는 값을 만들어 넘기기만 하고 파일 배치는 글루 몫이다.
    /// None 이면 셸 기본 history 파일 (히스토리 분리 없음).
    pub history_tab: Option<u64>,
}

impl Default for ShellSpawnReq {
    fn default() -> Self {
        Self {
            cwd: None,
            distro: None,
            cols: 80,
            rows: 24,
            history_tab: None,
        }
    }
}

/// PTY 부수효과 포트. 실제 구현은 앱 글루(`SessionManager` 래핑), 테스트는 fake.
pub trait SessionHost: Send {
    /// 셸 세션을 스폰하고 휘발성 세션 id 를 돌려준다.
    fn spawn_shell(&self, req: ShellSpawnReq) -> anyhow::Result<SessionId>;

    /// 세션 종료. 미지·이미 종료된 id 에도 무해해야 한다 (멱등 —
    /// `PtySession::kill` 과 동일 계약).
    fn kill(&self, id: SessionId);
}

/// revision 을 곁들인 상태 직렬화 뷰 — `state-changed` emit·`get_state` 응답 형태.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot<'a> {
    pub revision: u64,
    pub state: &'a AppState,
}

/// 상태 소유자. 모든 구조 변이는 [`Dispatcher::dispatch`] 를 경유한다.
pub struct Dispatcher {
    state: AppState,
    host: Box<dyn SessionHost>,
}

impl Dispatcher {
    pub fn new(host: Box<dyn SessionHost>) -> Self {
        Self {
            state: AppState::new(),
            host,
        }
    }

    /// 복원(sanitize 완료)된 상태를 **스폰 없이** 채택한다 — manage-first 부팅
    /// (계획 15단계 B-2)의 코어 절반. 글루는 이 dispatcher 를 즉시 manage 한 뒤
    /// [`Self::running_terminal_tabs`] 로 대상을 뽑아 탭별로 [`Self::respawn_tab`]
    /// 을 호출해 재스폰한다. adopt 시점에는 살아 있는 PTY 가 없다 — persist
    /// sanitize 가 전 터미널 탭의 `pty_session` 을 소거한 상태를 전제한다.
    pub fn adopt(state: AppState, host: Box<dyn SessionHost>) -> Self {
        // persist::load 가 릴리즈에서도 validate 를 마쳤지만, 다른 호출자(테스트
        // 등)의 실수는 debug 에서 즉시 드러낸다.
        for ws in &state.workspaces {
            ws.debug_assert_invariants();
        }
        Self { state, host }
    }

    /// 테스트·글루용 상태 접근자.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn snapshot(&self) -> StateSnapshot<'_> {
        StateSnapshot {
            revision: self.state.revision,
            state: &self.state,
        }
    }

    /// 명령 실행. 성공 시에만 `revision += 1`, 실패 시 상태 불변.
    pub fn dispatch(&mut self, cmd: Command) -> Result<CommandOutput, CommandError> {
        let out = self.execute(cmd)?;
        self.state.revision += 1;
        for ws in &self.state.workspaces {
            ws.debug_assert_invariants();
        }
        Ok(out)
    }

    /// 세션 이벤트 반영. 상태가 실제로 바뀔 때만 `revision += 1`.
    pub fn apply_event(&mut self, ev: SessionEvent) {
        match ev {
            SessionEvent::SessionExited { session, code } => {
                let mut changed = false;
                for ws in &mut self.state.workspaces {
                    let mut exited = Vec::new();
                    for pane in ws.panes.values_mut() {
                        for tab in &mut pane.tabs {
                            if let TabKind::Terminal {
                                pty_session: Some(s),
                                status,
                                ..
                            } = &mut tab.kind
                            {
                                if *s == session {
                                    // pty_session 은 유지한다 (Exited 탭 표시용).
                                    let next = TerminalStatus::Exited { code };
                                    if *status != next {
                                        *status = next;
                                        changed = true;
                                    }
                                    exited.push(tab.id);
                                }
                            }
                        }
                    }
                    // 죽은 세션의 탭이 워크스페이스 상태의 출처였으면 되돌린다 —
                    // 탭 소멸 3경로(CloseTab·ClosePane·SessionExited)가 공유하는 규칙.
                    for tab in exited {
                        changed |= reset_agent_source(ws, tab);
                    }
                }
                // 미지 session 이면 changed == false — 무해한 no-op (모듈 doc 참조).
                if changed {
                    self.state.revision += 1;
                }
            }
        }
    }

    /// flush 창 하나에 모인 OSC 델타를 모델에 반영한다 (18단계 계획 core 계약).
    /// 반환값은 "상태가 실제로 바뀌었는가" — 글루는 true 일 때만 스냅샷을 발행한다.
    ///
    /// - 세션→탭 역매핑은 [`Self::apply_event`] 와 같은 선형 탐색이고, **미지 세션
    ///   은 무해한 no-op** 이다 (창이 열려 있는 동안 탭이 닫히는 정상 순서).
    /// - **Exited 탭은 델타를 통째로 스킵**한다: 100ms 창 안에서 세션이 끝나면 즉시
    ///   처리된 `SessionExited` 의 Idle 리셋 뒤에 지연된 배치가 죽은 탭에 알림을
    ///   다시 도장하는 구멍이 생긴다.
    /// - `now_ms` 는 글루가 주입한다 (코어는 시계를 읽지 않는다).
    /// - revision 은 변경이 하나라도 있을 때 **배치당 1회** 오른다.
    pub fn apply_osc(&mut self, batch: OscBatch, now_ms: u64) -> bool {
        let mut changed = false;
        for (session, delta) in &batch.entries {
            let Some((wi, pane, ti)) = self.locate_session(*session) else {
                continue;
            };
            changed |= self.apply_delta(wi, pane, ti, delta, now_ms);
        }
        if changed {
            self.state.revision += 1;
        }
        changed
    }

    /// 델타 하나를 이미 역매핑된 탭에 반영한다. 반환값은 이 델타가 상태를 바꿨는가.
    fn apply_delta(
        &mut self,
        wi: usize,
        pane: PaneId,
        ti: usize,
        delta: &OscDelta,
        now_ms: u64,
    ) -> bool {
        // 가시 탭 = active 워크스페이스에 속하고 그 pane 의 active_tab 인 탭. pane 은
        // 전부 화면에 보이므로 active_pane 여부는 따지지 않는다. 창 포커스와도
        // 결합하지 않는다 (v1 결정 — 자리를 비운 사용자에게는 사이드바 상태가 남는다).
        let ws_ref = &self.state.workspaces[wi];
        let tab_id = ws_ref.panes[&pane].tabs[ti].id;
        let visible = self.state.active_workspace == Some(ws_ref.id)
            && ws_ref.panes[&pane].active_tab == Some(tab_id);

        let ws = &mut self.state.workspaces[wi];
        let tab = &mut ws
            .panes
            .get_mut(&pane)
            .expect("locate_session 이 존재를 보장")
            .tabs[ti];
        if matches!(
            tab.kind,
            TabKind::Terminal {
                status: TerminalStatus::Exited { .. },
                ..
            }
        ) {
            return false;
        }

        let mut changed = false;
        if let Some(title) = &delta.title {
            if tab.title != *title {
                tab.title.clone_from(title);
                changed = true;
            }
        }
        if let Some(next_cwd) = &delta.cwd {
            // respawn 이 탭 cwd 를 쓰므로(재시작 후 마지막 디렉터리 복원) OSC 7 은
            // 탭에 기록된 cwd 를 갱신한다.
            if let TabKind::Terminal { cwd, .. } = &mut tab.kind {
                if cwd.as_deref() != Some(next_cwd.as_str()) {
                    *cwd = Some(next_cwd.clone());
                    changed = true;
                }
            }
        }
        // 가시 탭의 unread 는 억제한다 — 내용이 이미 눈앞에 있으므로 dot 이 의미가
        // 없고, 이미 활성인 탭에는 ActivateTab 해제가 다시 오지 않는다.
        if delta.unread && !visible && tab.notification != NotificationState::Unread {
            tab.notification = NotificationState::Unread;
            changed = true;
        }
        if tab.last_activity_ms != Some(now_ms) {
            tab.last_activity_ms = Some(now_ms);
            changed = true;
        }

        if let Some(status) = delta.status {
            // needsInput 우선: 입력 대기 중인 워크스페이스는 **다른 탭**의 상태
            // 알림으로 덮이지 않는다 (사이드바만 훑어도 입력 대기가 보여야 한다 —
            // 계획 v2 9장). 사용자가 응답하면 같은 출처 탭의 running 이 강등한다.
            let allowed = ws.agent_status != AgentStatus::NeedsInput
                || status == AgentStatus::NeedsInput
                || ws.agent_status_source == Some(tab_id);
            if allowed {
                if ws.agent_status != status {
                    ws.agent_status = status;
                    changed = true;
                }
                if ws.agent_status_source != Some(tab_id) {
                    ws.agent_status_source = Some(tab_id);
                    changed = true;
                }
            }
        }
        // 미리보기 메시지는 상태 우선 규칙과 독립이다 — 우선 규칙의 대상은
        // agent_status 뿐이고, 메시지는 마지막으로 도착한 알림 본문을 보여준다.
        if let Some(message) = &delta.message {
            if ws.last_agent_message.as_deref() != Some(message.as_str()) {
                ws.last_agent_message = Some(message.clone());
                changed = true;
            }
        }
        changed
    }

    /// respawn 대상 열거 — **Running 터미널 탭 중 `pty_session` 이 None** 인 탭
    /// (= adopt 직후의 재스폰 대상). Exited 로 저장된 탭은 재스폰하지 않는 상태
    /// 충실 복원 결정(계획 B-2 — 재시작 후 빈 내용 + exited 배지)에 따라 열거에서
    /// 도 제외된다.
    pub fn running_terminal_tabs(&self) -> Vec<TabId> {
        let mut tabs = Vec::new();
        for ws in &self.state.workspaces {
            for pane in ws.panes.values() {
                for tab in &pane.tabs {
                    if matches!(
                        tab.kind,
                        TabKind::Terminal {
                            pty_session: None,
                            status: TerminalStatus::Running,
                            ..
                        }
                    ) {
                        tabs.push(tab.id);
                    }
                }
            }
        }
        tabs
    }

    /// adopt 된 상태의 터미널 탭 하나에 새 셸을 재스폰한다 — manage-first 부팅의
    /// 탭별 단계 (글루가 회당 lock 으로 호출, 계획 0장).
    ///
    /// - 대상은 **Running 터미널 탭이면서 `pty_session` 이 None** 인 탭뿐이다.
    ///   그 외(미지 id·Exited·이미 세션 있음)는 [`CommandError::UnknownTarget`] —
    ///   글루가 [`Self::running_terminal_tabs`] 스냅샷에서 대상을 뽑으므로 부적합
    ///   호출은 프로그램 결함이고, 조용한 no-op 으로 가리지 않는다 (이 경우
    ///   상태·revision 불변).
    /// - 스폰은 cwd = 탭 cwd(없으면 워크스페이스 root_path), distro = 워크스페이스
    ///   기본값, 80×24, history_tab = 이 탭의 id — [`Command::CreateTab`] 과 같은
    ///   스폰 경로를 공유한다. 탭에 기록된 cwd 는 바꾸지 않는다 (생성 시점 값
    ///   보존 — 계획 0장 충실도 한계 명시).
    /// - 성공 시 `pty_session` 에 새 id 를 채우고, **스폰 실패 시 그 탭을
    ///   `Exited { code: None }` 으로 강등한다** — dispatch 의 "실패 = 상태 불변"
    ///   계약과 달리 강등 자체가 설계된 결과 상태다. 성공·강등 어느 쪽이든
    ///   `revision += 1` 로 스냅샷에 전파된다.
    pub fn respawn_tab(&mut self, tab: TabId) -> Result<SessionId, CommandError> {
        let (wi, pane, ti) = self.locate_tab(tab)?;
        // 적격성 검사 — 통과 못 하면 상태·revision 불변으로 에러.
        let tab_cwd = match &self.state.workspaces[wi].panes[&pane].tabs[ti].kind {
            TabKind::Terminal {
                pty_session: None,
                status: TerminalStatus::Running,
                cwd,
            } => cwd.clone(),
            _ => return Err(unknown("respawnable tab", tab.0)),
        };
        // 재스폰은 탭 id 가 이미 있으므로 peek 없이 그대로 넘긴다 — 같은 탭이면
        // 재시작 전후로 같은 HISTFILE 을 다시 물게 되는 것이 이 기능의 요점이다.
        let spawned = self.spawn_terminal(wi, tab_cwd, tab.0);
        let pane_ref = self.state.workspaces[wi]
            .panes
            .get_mut(&pane)
            .expect("locate_tab 이 존재를 보장");
        let TabKind::Terminal {
            pty_session,
            status,
            ..
        } = &mut pane_ref.tabs[ti].kind
        else {
            unreachable!("적격성 검사를 통과한 terminal 탭");
        };
        let result = match spawned {
            Ok((session, _effective_cwd)) => {
                *pty_session = Some(session);
                Ok(session)
            }
            Err(err) => {
                // 스폰 실패 강등 — pty_session 은 None 그대로 (배지만 Exited).
                *status = TerminalStatus::Exited { code: None };
                Err(err)
            }
        };
        self.state.revision += 1;
        for ws in &self.state.workspaces {
            ws.debug_assert_invariants();
        }
        result
    }

    fn execute(&mut self, cmd: Command) -> Result<CommandOutput, CommandError> {
        match cmd {
            Command::CreateWorkspace {
                name,
                root_path,
                distro,
                tab,
            } => {
                // 원자성: 탭 동반 생성은 준비 단계(terminal = spawn, 뷰어 = 경로
                // 검증)를 **모든 상태 변이보다 먼저** 수행한다 (CreateTab 과 동일
                // 한 순서) — 실패 시 워크스페이스·pane·next_id·revision 전부
                // 불변이다. 워크스페이스가 아직 상태에 없으므로 기본값
                // (root_path·distro)을 직접 넘긴다.
                //
                // history_tab: 스폰이 id 발급보다 먼저이므로 이 핸들러의 할당 순서
                // (workspace → pane → tab)를 반영해 탭이 받게 될 id 를 peek 한다.
                let history_tab = self.state.peek_id(2);
                let prepared = match tab {
                    Some(spec) => {
                        Some(self.prepare_tab_with(&root_path, &distro, spec, history_tab)?)
                    }
                    None => None,
                };
                let workspace = WorkspaceId(self.state.alloc_id());
                let pane = PaneId(self.state.alloc_id());
                let tab_id = prepared.as_ref().map(|_| TabId(self.state.alloc_id()));
                if let Some(tab_id) = tab_id {
                    // 할당 순서가 바뀌면 peek 이 어긋난다 — 여기서 즉시 터뜨려
                    // 커플링을 드러낸다 (peek_id rustdoc).
                    debug_assert_eq!(tab_id.0, history_tab, "peek 한 탭 id 와 실제 발급 불일치");
                }
                let session = prepared.as_ref().and_then(PreparedTab::session);
                let mut initial = empty_pane(pane);
                if let (Some(tab_id), Some(prepared)) = (tab_id, prepared) {
                    initial.tabs.push(prepared.into_tab(tab_id));
                    initial.active_tab = Some(tab_id);
                }
                self.state.workspaces.push(Workspace {
                    id: workspace,
                    name,
                    root_path,
                    distro,
                    git_branch: None,
                    git_dirty: None,
                    layout: SplitTree::Leaf { pane },
                    panes: [(pane, initial)].into(),
                    active_pane: pane,
                    agent_status: AgentStatus::Idle,
                    last_agent_message: None,
                    agent_status_source: None,
                });
                self.state.active_workspace = Some(workspace);
                Ok(CommandOutput::WorkspaceCreated {
                    workspace,
                    pane,
                    tab: tab_id,
                    session,
                })
            }

            Command::SwitchWorkspace { workspace } => {
                let ws = self
                    .state
                    .workspace_mut(workspace)
                    .ok_or_else(|| unknown("workspace", workspace.0))?;
                // "가시화 = 읽음": 전환하면 각 pane 의 active_tab 이 곧바로 화면에
                // 드러나므로 그 탭들의 unread 를 내린다 (비활성 탭은 그대로).
                clear_visible_unread(ws);
                self.state.active_workspace = Some(workspace);
                Ok(CommandOutput::Done)
            }

            Command::CloseWorkspace { workspace } => {
                let wi = self
                    .state
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == workspace)
                    .ok_or_else(|| unknown("workspace", workspace.0))?;
                let removed = self.state.workspaces.remove(wi);
                for pane in removed.panes.values() {
                    for tab in &pane.tabs {
                        self.kill_if_terminal(tab);
                    }
                }
                if self.state.active_workspace == Some(workspace) {
                    // 닫은 워크스페이스가 active 였으면 남은 것 중 첫 번째로,
                    // 없으면 None (마지막 워크스페이스 닫기 허용). fallback 으로
                    // 드러나는 워크스페이스에는 SwitchWorkspace 와 같은
                    // "가시화 = 읽음" 규칙을 적용한다 (18단계 리뷰 finding).
                    self.state.active_workspace = self.state.workspaces.first().map(|ws| ws.id);
                    if let Some(shown) = self.state.workspaces.first_mut() {
                        clear_visible_unread(shown);
                    }
                }
                Ok(CommandOutput::Done)
            }

            Command::FocusPane { pane } => {
                let wi = self.ws_index_of_pane(pane)?;
                self.state.workspaces[wi].active_pane = pane;
                Ok(CommandOutput::Done)
            }

            Command::SplitPane {
                pane,
                direction,
                tab,
            } => {
                let wi = self.ws_index_of_pane(pane)?;
                // 원자성: 탭 동반 분할은 준비 단계(terminal = spawn, 뷰어 = 경로
                // 검증)를 **모든 상태 변이보다 먼저** 수행한다 (CreateTab 과 동일
                // 한 순서) — 실패 시 트리·panes·next_id 가 전부 불변이라 분할만
                // 된 중간 상태가 없다.
                //
                // history_tab: 이 핸들러의 할당 순서(pane → split → tab)를 반영해
                // 탭이 받게 될 id 를 peek 한다 (CreateWorkspace 와 같은 방식).
                let history_tab = self.state.peek_id(2);
                let prepared = match tab {
                    Some(spec) => Some(self.prepare_tab(wi, spec, history_tab)?),
                    None => None,
                };
                let new_pane = PaneId(self.state.alloc_id());
                let split_id = SplitId(self.state.alloc_id());
                let tab_id = prepared.as_ref().map(|_| TabId(self.state.alloc_id()));
                if let Some(tab_id) = tab_id {
                    debug_assert_eq!(tab_id.0, history_tab, "peek 한 탭 id 와 실제 발급 불일치");
                }
                let session = prepared.as_ref().and_then(PreparedTab::session);
                let ws = &mut self.state.workspaces[wi];
                let split_ok = ws.layout.split(pane, direction, new_pane, split_id);
                debug_assert!(split_ok, "불변식: panes 의 pane 은 layout leaf 로 존재");
                let mut created = empty_pane(new_pane);
                if let (Some(tab_id), Some(prepared)) = (tab_id, prepared) {
                    created.tabs.push(prepared.into_tab(tab_id));
                    created.active_tab = Some(tab_id);
                }
                ws.panes.insert(new_pane, created);
                ws.active_pane = new_pane;
                Ok(CommandOutput::PaneCreated {
                    pane: new_pane,
                    split: split_id,
                    tab: tab_id,
                    session,
                })
            }

            Command::ResizeSplit { split, ratio } => {
                if !(ratio.is_finite() && 0.0 < ratio && ratio < 1.0) {
                    return Err(CommandError::InvalidRatio { ratio });
                }
                // split id 도 전 워크스페이스 범위 탐색 (안정 ID 전역 유일).
                let found = self
                    .state
                    .workspaces
                    .iter_mut()
                    .any(|ws| ws.layout.set_ratio(split, ratio));
                if !found {
                    return Err(unknown("split", split.0));
                }
                Ok(CommandOutput::Done)
            }

            Command::ClosePane { pane } => {
                let wi = self.ws_index_of_pane(pane)?;
                if self.state.workspaces[wi].panes.len() <= 1 {
                    return Err(CommandError::LastPane);
                }
                let removed = collapse_pane(&mut self.state.workspaces[wi], pane);
                for tab in &removed.tabs {
                    // 사라지는 탭이 워크스페이스 상태의 출처면 Idle 로 되돌린다
                    // (pane 하나에 여러 탭이 있을 수 있어 제거되는 탭 전부 확인).
                    reset_agent_source(&mut self.state.workspaces[wi], tab.id);
                    self.kill_if_terminal(tab);
                }
                Ok(CommandOutput::Done)
            }

            Command::CreateTab { pane, tab } => {
                let wi = self.ws_index_of_pane(pane)?;
                // 원자성: 준비 단계(spawn·경로 검증) 실패 시 상태(탭·next_id)가
                // 변하지 않도록 준비를 먼저 마친다.
                //
                // history_tab: 이 핸들러는 탭 id 만 발급하므로 peek offset 0.
                let history_tab = self.state.peek_id(0);
                let prepared = self.prepare_tab(wi, tab, history_tab)?;
                let session = prepared.session();
                let tab_id = TabId(self.state.alloc_id());
                debug_assert_eq!(tab_id.0, history_tab, "peek 한 탭 id 와 실제 발급 불일치");
                let pane_ref = self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("ws_index_of_pane 이 존재를 보장");
                pane_ref.tabs.push(prepared.into_tab(tab_id));
                pane_ref.active_tab = Some(tab_id);
                Ok(CommandOutput::TabCreated {
                    tab: tab_id,
                    session,
                })
            }

            Command::ActivateTab { tab } => {
                let (wi, pane, ti) = self.locate_tab(tab)?;
                let pane_ref = self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("locate_tab 이 존재를 보장");
                pane_ref.active_tab = Some(tab);
                // "가시화 = 읽음" — 활성화된 탭의 unread 는 여기서 내려간다.
                pane_ref.tabs[ti].notification = NotificationState::None;
                Ok(CommandOutput::Done)
            }

            Command::CloseTab { tab } => {
                let (wi, pane, ti) = self.locate_tab(tab)?;
                let ws = &mut self.state.workspaces[wi];
                let pane_ref = ws.panes.get_mut(&pane).expect("locate_tab 이 존재를 보장");
                let removed = pane_ref.tabs.remove(ti);
                if pane_ref.active_tab == Some(tab) {
                    // 직전 탭으로 조정. 첫 탭이었으면 (제거 후 index 0 에 온) 다음
                    // 탭, 마지막 남은 탭이었으면 None.
                    pane_ref.active_tab = if pane_ref.tabs.is_empty() {
                        None
                    } else {
                        Some(pane_ref.tabs[ti.saturating_sub(1)].id)
                    };
                    // 승격도 가시화다 (18단계 리뷰 finding): 이 워크스페이스가
                    // 보이는 중이면 화면에 드러난 승격 탭의 unread 를 내린다.
                    if self.state.active_workspace == Some(ws.id) {
                        if let Some(promoted) = pane_ref.active_tab {
                            if let Some(t) = pane_ref.tabs.iter_mut().find(|t| t.id == promoted) {
                                t.notification = NotificationState::None;
                            }
                        }
                    }
                }
                // auto-collapse (계획 D6): 마지막 탭이 닫혀 pane 이 비면 pane 자체
                // 를 collapse 한다 — 단 워크스페이스의 마지막 pane 은 예외로 빈
                // pane 으로 남긴다 (variant rustdoc 의 규칙 명세 참조).
                if pane_ref.tabs.is_empty() && ws.panes.len() > 1 {
                    let collapsed = collapse_pane(ws, pane);
                    debug_assert!(collapsed.tabs.is_empty(), "빈 pane 만 collapse 대상");
                }
                // 닫힌 탭이 워크스페이스 상태의 출처면 Idle 로 되돌린다.
                reset_agent_source(ws, tab);
                self.kill_if_terminal(&removed);
                Ok(CommandOutput::Done)
            }

            Command::NavigateFolder { tab, path } => {
                // 값 검증을 대상 탐색보다 먼저 (ResizeSplit 과 같은 순서).
                validate_viewer_path(&path)?;
                let title = path_title(&path);
                let (wi, pane, ti) = self.locate_tab(tab)?;
                let tab_ref = &mut self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("locate_tab 이 존재를 보장")
                    .tabs[ti];
                let TabKind::FolderBrowser { path: current } = &mut tab_ref.kind else {
                    return Err(CommandError::KindMismatch { tab });
                };
                *current = path;
                // 제목도 새 경로의 basename 으로 따라간다 (탭 스트립 표시).
                tab_ref.title = title;
                Ok(CommandOutput::Done)
            }

            Command::SetViewerScroll { tab, scroll_top } => {
                if !(scroll_top.is_finite() && scroll_top >= 0.0) {
                    return Err(CommandError::InvalidScroll { value: scroll_top });
                }
                let (wi, pane, ti) = self.locate_tab(tab)?;
                let tab_ref = &mut self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("locate_tab 이 존재를 보장")
                    .tabs[ti];
                // folderBrowser·terminal 은 모델에 스크롤 위치가 없다 — 조용한
                // no-op 대신 KindMismatch 로 드러낸다. 값의 단위는 종류마다
                // 다르지만(byte offset vs px) 코어는 f64 를 그대로 보관한다.
                let (TabKind::TextViewer {
                    scroll_top: current,
                    ..
                }
                | TabKind::MarkdownViewer {
                    scroll_top: current,
                    ..
                }) = &mut tab_ref.kind
                else {
                    return Err(CommandError::KindMismatch { tab });
                };
                *current = scroll_top;
                Ok(CommandOutput::Done)
            }
        }
    }

    /// 워크스페이스 기본값(root_path·distro)을 적용한 [`Self::prepare_tab_with`]
    /// — 이미 상태에 있는 워크스페이스(CreateTab·SplitPane)용.
    fn prepare_tab(
        &self,
        wi: usize,
        spec: NewTab,
        history_tab: u64,
    ) -> Result<PreparedTab, CommandError> {
        let ws = &self.state.workspaces[wi];
        self.prepare_tab_with(&ws.root_path, &ws.distro, spec, history_tab)
    }

    /// 탭 생성의 **앞단** — terminal 은 셸 스폰, 뷰어는 경로 기본값 해석 + 형태
    /// 검증. 세 생성 지점(CreateTab·SplitPane·CreateWorkspace)이 공유하며,
    /// 호출자는 **모든 상태 변이 전에** 이걸 마쳐 실패 시 상태 불변을 보장한다
    /// (뷰어는 스폰이 없어도 InvalidPath 로 실패할 수 있다 — 계획 21단계).
    ///
    /// `history_tab` 은 이 탭이 **받게 될** 안정 ID 다 — 스폰이 발급보다 먼저라
    /// 호출자가 [`AppState::peek_id`] 로 미리 읽어 넘긴다 (뷰어 탭은 스폰이 없어
    /// 쓰지 않는다).
    fn prepare_tab_with(
        &self,
        root_path: &Option<String>,
        distro: &Option<String>,
        spec: NewTab,
        history_tab: u64,
    ) -> Result<PreparedTab, CommandError> {
        match spec {
            NewTab::Terminal { cwd } => {
                let (session, cwd) =
                    self.spawn_terminal_with(root_path, distro, cwd, history_tab)?;
                Ok(PreparedTab::Terminal { session, cwd })
            }
            NewTab::FolderBrowser { path } => {
                // 이중 기본값: 탭 path → 워크스페이스 root_path → "/".
                let path = path
                    .or_else(|| root_path.clone())
                    .unwrap_or_else(|| "/".to_owned());
                validate_viewer_path(&path)?;
                Ok(PreparedTab::Viewer {
                    title: path_title(&path),
                    kind: TabKind::FolderBrowser { path },
                })
            }
            NewTab::TextViewer { path } => {
                validate_viewer_path(&path)?;
                Ok(PreparedTab::Viewer {
                    title: path_title(&path),
                    // 새 탭은 항상 파일 선두에서 시작한다 (복원은 persist 몫).
                    kind: TabKind::TextViewer {
                        path,
                        scroll_top: 0.0,
                    },
                })
            }
            NewTab::MarkdownViewer { path } => {
                validate_viewer_path(&path)?;
                Ok(PreparedTab::Viewer {
                    title: path_title(&path),
                    // TextViewer 와 같이 문서 선두(px 0)에서 시작한다.
                    kind: TabKind::MarkdownViewer {
                        path,
                        scroll_top: 0.0,
                    },
                })
            }
        }
    }

    /// 워크스페이스 기본값(cwd·distro)을 적용해 터미널 셸을 스폰한다 — CreateTab·
    /// SplitPane(tab 포함)이 공유하는 spawn-first 원자성의 앞단: **모든 상태 변이
    /// 전에** 호출해 실패 시 상태 불변을 보장한다. 탭 cwd 미지정 시 워크스페이스
    /// root_path 가 기본 (계획 v2 4장). 반환: (세션 id, 탭에 기록할 실제 cwd).
    fn spawn_terminal(
        &self,
        wi: usize,
        cwd: Option<String>,
        history_tab: u64,
    ) -> Result<(SessionId, Option<String>), CommandError> {
        let ws = &self.state.workspaces[wi];
        self.spawn_terminal_with(&ws.root_path, &ws.distro, cwd, history_tab)
    }

    /// [`Self::spawn_terminal`] 의 기본값 명시 버전 — CreateWorkspace(tab 포함)는
    /// 워크스페이스가 아직 상태에 없어 인덱스 대신 만들려는 값의 기본값을 직접
    /// 넘긴다 (spawn-first 계약은 동일).
    fn spawn_terminal_with(
        &self,
        root_path: &Option<String>,
        distro: &Option<String>,
        cwd: Option<String>,
        history_tab: u64,
    ) -> Result<(SessionId, Option<String>), CommandError> {
        let cwd = cwd.or_else(|| root_path.clone());
        let req = ShellSpawnReq {
            cwd: cwd.clone(),
            distro: distro.clone(),
            history_tab: Some(history_tab),
            ..ShellSpawnReq::default()
        };
        let session = self
            .host
            .spawn_shell(req)
            .map_err(|e| CommandError::SpawnFailed {
                message: e.to_string(),
            })?;
        Ok((session, cwd))
    }

    /// terminal 탭이면 소속 세션을 kill 한다. status 와 무관하게 kill 하며(이미
    /// Exited 여도 무해 — SessionHost::kill 은 멱등 계약), 뷰어 탭은 no-op.
    fn kill_if_terminal(&self, tab: &Tab) {
        if let TabKind::Terminal {
            pty_session: Some(s),
            ..
        } = tab.kind
        {
            self.host.kill(s);
        }
    }

    /// `pane` 을 소유한 워크스페이스의 인덱스 (전 워크스페이스 범위 탐색).
    fn ws_index_of_pane(&self, pane: PaneId) -> Result<usize, CommandError> {
        self.state
            .workspaces
            .iter()
            .position(|ws| ws.panes.contains_key(&pane))
            .ok_or_else(|| unknown("pane", pane.0))
    }

    /// `session` 이 실린 탭의 (워크스페이스 인덱스, pane id, tab 인덱스).
    /// 세션 id 는 탭 하나에만 실리므로 첫 일치에서 멈춘다. 미지 세션은 None —
    /// 호출자(OSC 반영)가 no-op 으로 처리한다.
    fn locate_session(&self, session: SessionId) -> Option<(usize, PaneId, usize)> {
        for (wi, ws) in self.state.workspaces.iter().enumerate() {
            for (pid, pane) in &ws.panes {
                let found = pane.tabs.iter().position(|t| {
                    matches!(
                        t.kind,
                        TabKind::Terminal {
                            pty_session: Some(s),
                            ..
                        } if s == session
                    )
                });
                if let Some(ti) = found {
                    return Some((wi, *pid, ti));
                }
            }
        }
        None
    }

    /// `tab` 을 소유한 (워크스페이스 인덱스, pane id, tab 인덱스).
    fn locate_tab(&self, tab: TabId) -> Result<(usize, PaneId, usize), CommandError> {
        for (wi, ws) in self.state.workspaces.iter().enumerate() {
            for (pid, pane) in &ws.panes {
                if let Some(ti) = pane.tabs.iter().position(|t| t.id == tab) {
                    return Ok((wi, *pid, ti));
                }
            }
        }
        Err(unknown("tab", tab.0))
    }
}

/// 탭 생성 앞단([`Dispatcher::prepare_tab_with`])의 결과 — terminal 은 스폰된
/// 세션과 실제 cwd, 뷰어는 검증을 마친 제목·종류. 어느 쪽이든 상태 변이 전에
/// 만들어지므로 이 값이 손에 들어온 시점에는 "실패할 일이 남아 있지 않다".
enum PreparedTab {
    Terminal {
        session: SessionId,
        cwd: Option<String>,
    },
    Viewer {
        title: String,
        kind: TabKind,
    },
}

impl PreparedTab {
    /// 발급된 안정 ID 로 탭 값을 완성한다.
    fn into_tab(self, id: TabId) -> Tab {
        match self {
            PreparedTab::Terminal { session, cwd } => terminal_tab(id, session, cwd),
            PreparedTab::Viewer { title, kind } => viewer_tab(id, title, kind),
        }
    }

    /// 출력(`TabCreated` 등)에 실을 세션 id — 뷰어 탭은 None.
    fn session(&self) -> Option<SessionId> {
        match self {
            PreparedTab::Terminal { session, .. } => Some(*session),
            PreparedTab::Viewer { .. } => None,
        }
    }
}

fn empty_pane(id: PaneId) -> Pane {
    Pane {
        id,
        tabs: Vec::new(),
        active_tab: None,
    }
}

/// 갓 스폰된 터미널 세션의 탭 값 — CreateTab·SplitPane·CreateWorkspace(tab
/// 포함)가 공유한다.
fn terminal_tab(id: TabId, session: SessionId, cwd: Option<String>) -> Tab {
    Tab {
        id,
        title: "Terminal".to_owned(),
        kind: TabKind::Terminal {
            pty_session: Some(session),
            status: TerminalStatus::Running,
            cwd,
        },
        notification: NotificationState::None,
        last_activity_ms: None,
    }
}

/// 갓 만들어진 뷰어 탭의 값 — [`terminal_tab`] 의 뷰어 짝 (CreateTab·SplitPane·
/// CreateWorkspace 공유). 스폰이 없으므로 순수 변이다.
fn viewer_tab(id: TabId, title: String, kind: TabKind) -> Tab {
    Tab {
        id,
        title,
        kind,
        notification: NotificationState::None,
        last_activity_ms: None,
    }
}

/// 뷰어 경로의 형태 검증 — 사유 문자열을 그대로 [`CommandError::InvalidPath`] 에
/// 싣는다 (생성·NavigateFolder 공유). 실존 여부는 검사하지 않는다 (코어 무 I/O —
/// 없는 경로는 뷰 로드 실패로 표면화).
fn validate_viewer_path(path: &str) -> Result<(), CommandError> {
    crate::wslpath::validate_linux_path(path)
        .map_err(|message| CommandError::InvalidPath { message })
}

/// 경로에서 탭 제목으로 쓸 basename 을 뽑는다. 빈 세그먼트(`//`·후행 `/`)는
/// 건너뛰고, 남는 컴포넌트가 없으면(루트) `"/"` 를 쓴다.
fn path_title(path: &str) -> String {
    path.rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or("/")
        .to_owned()
}

/// pane 을 워크스페이스에서 제거한다: panes remove + tree collapse(형제 승격) +
/// active_pane fixup(닫힌 pane 이 포커스였으면 collapse 후 leaf 순서상 첫 pane
/// 으로). ClosePane 과 CloseTab auto-collapse 가 공유한다 (계획 D6).
///
/// # 호출자 계약
///
/// - `pane` 은 `ws.panes` 에 존재해야 하고, 워크스페이스의 마지막 pane 이면 안
///   된다 (루트 단일 leaf 는 collapse 불가 — ClosePane 은 `LastPane` 으로, CloseTab
///   은 빈 pane 예외로 사전에 거른다).
/// - 반환된 Pane 소속 탭들의 세션 kill 은 호출자 책임이다.
fn collapse_pane(ws: &mut Workspace, pane: PaneId) -> Pane {
    let removed = ws.panes.remove(&pane).expect("호출자가 pane 존재를 보장");
    let collapse_ok = ws.layout.remove(pane);
    debug_assert!(collapse_ok, "불변식: panes 의 pane 은 layout leaf 로 존재");
    if ws.active_pane == pane {
        ws.active_pane = ws.layout.leaves()[0];
    }
    removed
}

/// 사라지는 탭이 워크스페이스 `agent_status` 의 출처였으면 상태를 Idle 로 되돌린다
/// (18단계 계획 core 계약). 죽은 탭의 needsInput 이 사이드바에 남아 영원히 입력
/// 대기로 보이는 것을 막는 규칙이라, 탭이 사라지는 세 경로(`CloseTab`·`ClosePane`
/// 의 제거 탭 각각·`SessionExited`)가 전부 이 헬퍼를 거친다.
/// 반환값은 상태가 바뀌었는가 (revision 판정용).
fn reset_agent_source(ws: &mut Workspace, tab: TabId) -> bool {
    if ws.agent_status_source != Some(tab) {
        return false;
    }
    ws.agent_status_source = None;
    ws.agent_status = AgentStatus::Idle;
    true
}

/// "가시화 = 읽음" 의 워크스페이스 단위 적용 — 이 워크스페이스가 화면에 드러나는
/// 순간(SwitchWorkspace·CloseWorkspace fallback) 각 pane 의 active_tab unread 를
/// 내린다 (비활성 탭은 그대로). 탭 단위 짝은 ActivateTab·CloseTab 승격이다.
fn clear_visible_unread(ws: &mut Workspace) {
    for pane in ws.panes.values_mut() {
        let Some(active) = pane.active_tab else {
            continue;
        };
        if let Some(tab) = pane.tabs.iter_mut().find(|t| t.id == active) {
            tab.notification = NotificationState::None;
        }
    }
}

fn unknown(kind: &str, id: u64) -> CommandError {
    CommandError::UnknownTarget {
        target: format!("{kind} {id}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::osc::OscEvent;

    /// 테스트용 fake 호스트 — 스폰 id 순차 발급(1부터), kill·스폰 요청 기록,
    /// 스폰 실패 주입.
    #[derive(Default)]
    struct FakeHostInner {
        next_session: AtomicU32,
        kills: Mutex<Vec<SessionId>>,
        spawns: Mutex<Vec<ShellSpawnReq>>,
        fail_spawn: AtomicBool,
    }

    #[derive(Clone, Default)]
    struct FakeSessionHost(Arc<FakeHostInner>);

    impl FakeSessionHost {
        fn kills(&self) -> Vec<SessionId> {
            self.0.kills.lock().unwrap().clone()
        }

        fn spawns(&self) -> Vec<ShellSpawnReq> {
            self.0.spawns.lock().unwrap().clone()
        }

        fn set_fail_spawn(&self, fail: bool) {
            self.0.fail_spawn.store(fail, Ordering::SeqCst);
        }
    }

    impl SessionHost for FakeSessionHost {
        fn spawn_shell(&self, req: ShellSpawnReq) -> anyhow::Result<SessionId> {
            if self.0.fail_spawn.load(Ordering::SeqCst) {
                anyhow::bail!("injected spawn failure");
            }
            self.0.spawns.lock().unwrap().push(req);
            Ok(self.0.next_session.fetch_add(1, Ordering::SeqCst) + 1)
        }

        fn kill(&self, id: SessionId) {
            self.0.kills.lock().unwrap().push(id);
        }
    }

    fn dispatcher() -> (Dispatcher, FakeSessionHost) {
        let host = FakeSessionHost::default();
        (Dispatcher::new(Box::new(host.clone())), host)
    }

    /// 워크스페이스 1개(tab 없음)를 만들고 (workspace, 초기 pane) id 를
    /// 돌려주는 헬퍼.
    fn create_ws(d: &mut Dispatcher, name: &str) -> (WorkspaceId, PaneId) {
        match d
            .dispatch(Command::CreateWorkspace {
                name: name.into(),
                root_path: None,
                distro: None,
                tab: None,
            })
            .unwrap()
        {
            CommandOutput::WorkspaceCreated {
                workspace,
                pane,
                tab: None,
                session: None,
            } => (workspace, pane),
            other => panic!("unexpected output: {other:?}"),
        }
    }

    fn create_terminal_tab(d: &mut Dispatcher, pane: PaneId) -> (TabId, SessionId) {
        match d
            .dispatch(Command::CreateTab {
                pane,
                tab: NewTab::Terminal { cwd: None },
            })
            .unwrap()
        {
            CommandOutput::TabCreated {
                tab,
                session: Some(s),
            } => (tab, s),
            other => panic!("unexpected output: {other:?}"),
        }
    }

    /// 탭 없는 분할 헬퍼 — (새 pane, 새 split 노드) id 를 돌려준다.
    fn split_empty(
        d: &mut Dispatcher,
        pane: PaneId,
        direction: SplitDirection,
    ) -> (PaneId, SplitId) {
        match d
            .dispatch(Command::SplitPane {
                pane,
                direction,
                tab: None,
            })
            .unwrap()
        {
            CommandOutput::PaneCreated {
                pane,
                split,
                tab: None,
                session: None,
            } => (pane, split),
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn create_workspace_makes_empty_pane_and_activates() {
        let (mut d, _host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
                tab: None,
            })
            .unwrap();
        assert_eq!(
            out,
            CommandOutput::WorkspaceCreated {
                workspace: WorkspaceId(1),
                pane: PaneId(2),
                tab: None,
                session: None,
            }
        );
        let ws = d.state().workspace(WorkspaceId(1)).unwrap();
        assert_eq!(ws.layout, SplitTree::Leaf { pane: PaneId(2) });
        assert_eq!(ws.active_pane, PaneId(2));
        assert_eq!(ws.agent_status, AgentStatus::Idle);
        assert!(ws.panes[&PaneId(2)].tabs.is_empty());
        assert_eq!(ws.panes[&PaneId(2)].active_tab, None);
        assert_eq!(d.state().active_workspace, Some(WorkspaceId(1)));
        assert_eq!(d.state().revision, 1);
        assert_eq!(d.state().next_id, 3);
    }

    #[test]
    fn create_workspace_with_tab_creates_tab_atomically() {
        let (mut d, host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        // 생성된 안정 ID 전부 반환 (계획 13-D1) — 발급 순서는 workspace →
        // pane → tab.
        let CommandOutput::WorkspaceCreated {
            workspace,
            pane,
            tab: Some(tab),
            session: Some(session),
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        assert!(workspace.0 < pane.0 && pane.0 < tab.0);

        let w = d.state().workspace(workspace).unwrap();
        assert_eq!(w.layout, SplitTree::Leaf { pane });
        assert_eq!(w.active_pane, pane);
        let p = &w.panes[&pane];
        assert_eq!(p.tabs.len(), 1);
        assert_eq!(p.tabs[0].id, tab);
        assert_eq!(p.active_tab, Some(tab));
        let TabKind::Terminal {
            pty_session,
            status,
            cwd,
        } = &p.tabs[0].kind
        else {
            panic!("terminal 탭이 아님");
        };
        assert_eq!(*pty_session, Some(session));
        assert_eq!(*status, TerminalStatus::Running);
        // cwd 미지정 → 워크스페이스 root_path 상속, distro 도 워크스페이스
        // 기본값 — CreateTab·SplitPane 과 공유하는 스폰 경로.
        assert_eq!(cwd.as_deref(), Some("/proj"));
        assert_eq!(host.spawns()[0].cwd.as_deref(), Some("/proj"));
        assert_eq!(host.spawns()[0].distro.as_deref(), Some("Ubuntu"));
        assert_eq!(d.state().active_workspace, Some(workspace));
        assert_eq!(d.state().revision, 1);
    }

    #[test]
    fn create_workspace_with_tab_spawn_failure_leaves_state_untouched() {
        let (mut d, host) = dispatcher();
        // 기존 워크스페이스를 하나 두어 active_workspace 불변까지 함께 잠근다.
        create_ws(&mut d, "existing");
        host.set_fail_spawn(true);
        let before = serde_json::to_value(d.state()).unwrap();

        let err = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: None,
                distro: None,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::SpawnFailed { .. }));
        // 워크스페이스·pane·next_id·revision 전부 불변 (spawn-first 원자성).
        let after = serde_json::to_value(d.state()).unwrap();
        assert_eq!(before, after, "spawn 실패가 상태를 바꿈 (원자성 위반)");
    }

    #[test]
    fn create_workspace_tab_field_missing_deserializes_to_none() {
        // 하위호환 (계획 13-D1): 13단계 이전 클라이언트의 tab 필드 없는 JSON 은
        // tab: None 으로 파싱된다 (fixture 쪽 잠금은 dispatcher.rs 참조).
        let cmd: Command = serde_json::from_str(
            r#"{ "type": "createWorkspace", "name": "ws", "rootPath": null, "distro": null }"#,
        )
        .unwrap();
        assert_eq!(
            cmd,
            Command::CreateWorkspace {
                name: "ws".into(),
                root_path: None,
                distro: None,
                tab: None,
            }
        );
    }

    #[test]
    fn full_flow_kills_sessions_and_collapses() {
        let (mut d, host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");

        // 분할(탭 없음) → 새 빈 pane 이 second 로 생기고 포커스 이동.
        let (pane2, split_id) = split_empty(&mut d, pane1, SplitDirection::Vertical);
        {
            let w = d.state().workspace(ws).unwrap();
            assert_eq!(w.active_pane, pane2);
            assert_eq!(w.layout.leaves(), vec![pane1, pane2]);
            assert_eq!(w.layout.split_ids(), vec![split_id]);
        }

        // 탭 3개: pane1 에 1개, pane2 에 2개.
        let (_tab1, s1) = create_terminal_tab(&mut d, pane1);
        let (tab2, s2) = create_terminal_tab(&mut d, pane2);
        let (tab3, s3) = create_terminal_tab(&mut d, pane2);
        assert_eq!((s1, s2, s3), (1, 2, 3));
        assert_eq!(
            d.state().workspace(ws).unwrap().panes[&pane2].active_tab,
            Some(tab3)
        );

        // 탭 닫기 → 그 세션만 kill, active_tab 은 직전 탭으로.
        d.dispatch(Command::CloseTab { tab: tab3 }).unwrap();
        assert_eq!(host.kills(), vec![s3]);
        assert_eq!(
            d.state().workspace(ws).unwrap().panes[&pane2].active_tab,
            Some(tab2)
        );

        // pane 닫기 → 남은 세션 kill + tree collapse + 포커스 회귀.
        d.dispatch(Command::ClosePane { pane: pane2 }).unwrap();
        assert_eq!(host.kills(), vec![s3, s2]);
        {
            let w = d.state().workspace(ws).unwrap();
            assert_eq!(w.layout, SplitTree::Leaf { pane: pane1 });
            assert_eq!(w.active_pane, pane1);
        }

        // 워크스페이스 닫기 → 남은 세션 전부 kill, active 는 None.
        d.dispatch(Command::CloseWorkspace { workspace: ws })
            .unwrap();
        assert_eq!(host.kills(), vec![s3, s2, s1]);
        assert!(d.state().workspaces.is_empty());
        assert_eq!(d.state().active_workspace, None);

        // revision 은 dispatch 성공 횟수(8)와 일치 — 단조 증가.
        assert_eq!(d.state().revision, 8);
    }

    #[test]
    fn exited_tab_close_still_kills_session() {
        // "kill 은 status 무관" 의미론 고정 — 자연 종료(SessionExited 반영)된
        // 탭을 닫아도 host.kill 이 호출된다 (kill 은 멱등 계약이라 늦은 호출 무해).
        let (mut d, host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (tab, session) = create_terminal_tab(&mut d, pane);
        d.apply_event(SessionEvent::SessionExited {
            session,
            code: Some(0),
        });
        d.dispatch(Command::CloseTab { tab }).unwrap();
        assert_eq!(host.kills(), vec![session]);
    }

    #[test]
    fn close_last_pane_is_rejected() {
        let (mut d, host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let rev = d.state().revision;
        let err = d.dispatch(Command::ClosePane { pane }).unwrap_err();
        assert_eq!(err, CommandError::LastPane);
        assert_eq!(d.state().revision, rev);
        assert!(host.kills().is_empty());
        assert!(d.state().workspaces[0].panes.contains_key(&pane));
    }

    #[test]
    fn unknown_targets_error_without_state_change() {
        let (mut d, _host) = dispatcher();
        create_ws(&mut d, "ws");
        let rev = d.state().revision;
        let cases = [
            Command::SwitchWorkspace {
                workspace: WorkspaceId(99),
            },
            Command::CloseWorkspace {
                workspace: WorkspaceId(99),
            },
            Command::FocusPane { pane: PaneId(99) },
            Command::SplitPane {
                pane: PaneId(99),
                direction: SplitDirection::Horizontal,
                tab: None,
            },
            Command::ResizeSplit {
                split: SplitId(99),
                ratio: 0.5,
            },
            Command::ClosePane { pane: PaneId(99) },
            Command::CreateTab {
                pane: PaneId(99),
                tab: NewTab::Terminal { cwd: None },
            },
            Command::ActivateTab { tab: TabId(99) },
            Command::CloseTab { tab: TabId(99) },
            Command::NavigateFolder {
                tab: TabId(99),
                path: "/proj".into(),
            },
            Command::SetViewerScroll {
                tab: TabId(99),
                scroll_top: 0.0,
            },
        ];
        for cmd in cases {
            let err = d.dispatch(cmd.clone()).unwrap_err();
            assert!(
                matches!(err, CommandError::UnknownTarget { .. }),
                "{cmd:?} → {err:?}"
            );
            assert_eq!(d.state().revision, rev, "{cmd:?} 가 상태를 바꿈");
        }
    }

    #[test]
    fn spawn_failure_leaves_state_untouched() {
        let (mut d, host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        host.set_fail_spawn(true);
        let before = serde_json::to_value(d.state()).unwrap();
        let err = d
            .dispatch(Command::CreateTab {
                pane,
                tab: NewTab::Terminal { cwd: None },
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::SpawnFailed { .. }));
        let after = serde_json::to_value(d.state()).unwrap();
        assert_eq!(before, after, "spawn 실패가 상태를 바꿈 (원자성 위반)");
    }

    #[test]
    fn resize_split_updates_ratio() {
        let (mut d, _host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");
        let (_pane2, split_id) = split_empty(&mut d, pane1, SplitDirection::Horizontal);
        let rev = d.state().revision;

        let out = d
            .dispatch(Command::ResizeSplit {
                split: split_id,
                ratio: 0.25,
            })
            .unwrap();
        assert_eq!(out, CommandOutput::Done);
        assert_eq!(d.state().revision, rev + 1);
        let SplitTree::Split { id, ratio, .. } = &d.state().workspace(ws).unwrap().layout else {
            panic!("split 이어야 함");
        };
        assert_eq!(*id, split_id);
        assert_eq!(*ratio, 0.25);
    }

    #[test]
    fn resize_split_reaches_inactive_workspace() {
        // split id 탐색은 전 워크스페이스 범위 — 비활성 워크스페이스의 split 도
        // id 로 조준된다 (안정 ID 전역 유일).
        let (mut d, _host) = dispatcher();
        let (ws1, pane1) = create_ws(&mut d, "one");
        let (_pane2, split_id) = split_empty(&mut d, pane1, SplitDirection::Vertical);
        let (ws2, _) = create_ws(&mut d, "two");
        assert_eq!(d.state().active_workspace, Some(ws2));

        d.dispatch(Command::ResizeSplit {
            split: split_id,
            ratio: 0.7,
        })
        .unwrap();
        let SplitTree::Split { ratio, .. } = &d.state().workspace(ws1).unwrap().layout else {
            panic!("split 이어야 함");
        };
        assert_eq!(*ratio, 0.7);
    }

    #[test]
    fn resize_split_stale_id_is_unknown_target() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane1) = create_ws(&mut d, "ws");
        let (pane2, split_id) = split_empty(&mut d, pane1, SplitDirection::Horizontal);
        // collapse 로 split 노드가 사라진 뒤 옛 id 로 resize — 스테일 주소.
        d.dispatch(Command::ClosePane { pane: pane2 }).unwrap();
        let rev = d.state().revision;

        let err = d
            .dispatch(Command::ResizeSplit {
                split: split_id,
                ratio: 0.5,
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::UnknownTarget { .. }), "{err:?}");
        assert_eq!(d.state().revision, rev);
    }

    #[test]
    fn resize_split_invalid_ratio_rejected_without_state_change() {
        let (mut d, _host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");
        let (_pane2, split_id) = split_empty(&mut d, pane1, SplitDirection::Horizontal);
        let rev = d.state().revision;

        // 개구간 (0, 1) 밖·비유한 값 전부 InvalidRatio — 경계 0.0·1.0 포함.
        for ratio in [f64::NAN, 0.0, 1.0, -0.25, 1.5, f64::INFINITY] {
            let err = d
                .dispatch(Command::ResizeSplit {
                    split: split_id,
                    ratio,
                })
                .unwrap_err();
            assert!(
                matches!(err, CommandError::InvalidRatio { .. }),
                "ratio {ratio} → {err:?}"
            );
            assert_eq!(d.state().revision, rev, "ratio {ratio} 가 상태를 바꿈");
        }
        let SplitTree::Split { ratio, .. } = &d.state().workspace(ws).unwrap().layout else {
            panic!("split 이어야 함");
        };
        assert_eq!(*ratio, 0.5, "실패한 resize 가 ratio 를 바꿈");
    }

    #[test]
    fn split_pane_with_tab_creates_pane_and_tab_atomically() {
        let (mut d, host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
                tab: None,
            })
            .unwrap();
        let CommandOutput::WorkspaceCreated {
            workspace: ws,
            pane: pane1,
            ..
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };

        let out = d
            .dispatch(Command::SplitPane {
                pane: pane1,
                direction: SplitDirection::Vertical,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        // 생성된 안정 ID 전부 반환 (계획 D5) — 발급 순서는 pane → split → tab.
        let CommandOutput::PaneCreated {
            pane: pane2,
            split,
            tab: Some(tab),
            session: Some(session),
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        assert!(pane2.0 < split.0 && split.0 < tab.0);

        let w = d.state().workspace(ws).unwrap();
        assert_eq!(w.active_pane, pane2);
        assert_eq!(w.layout.leaves(), vec![pane1, pane2]);
        assert_eq!(w.layout.split_ids(), vec![split]);
        let p2 = &w.panes[&pane2];
        assert_eq!(p2.tabs.len(), 1);
        assert_eq!(p2.active_tab, Some(tab));
        let TabKind::Terminal {
            pty_session,
            status,
            cwd,
        } = &p2.tabs[0].kind
        else {
            panic!("terminal 탭이 아님");
        };
        assert_eq!(*pty_session, Some(session));
        assert_eq!(*status, TerminalStatus::Running);
        // 워크스페이스 기본값(cwd·distro) 적용 — CreateTab 과 공유하는 스폰 경로.
        assert_eq!(cwd.as_deref(), Some("/proj"));
        assert_eq!(host.spawns()[0].cwd.as_deref(), Some("/proj"));
        assert_eq!(host.spawns()[0].distro.as_deref(), Some("Ubuntu"));
    }

    #[test]
    fn split_pane_with_tab_spawn_failure_leaves_state_untouched() {
        let (mut d, host) = dispatcher();
        let (_ws, pane1) = create_ws(&mut d, "ws");
        host.set_fail_spawn(true);
        let before = serde_json::to_value(d.state()).unwrap();

        let err = d
            .dispatch(Command::SplitPane {
                pane: pane1,
                direction: SplitDirection::Horizontal,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::SpawnFailed { .. }));
        // 트리·panes·next_id·revision 전부 불변 (spawn-first 원자성).
        let after = serde_json::to_value(d.state()).unwrap();
        assert_eq!(before, after, "spawn 실패가 상태를 바꿈 (원자성 위반)");
    }

    #[test]
    fn close_last_tab_collapses_pane_and_fixes_focus() {
        // multi-pane 워크스페이스에서 pane 의 마지막 탭 닫기 → 세션 kill +
        // collapse + active_pane fixup (계획 D6).
        let (mut d, host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");
        let out = d
            .dispatch(Command::SplitPane {
                pane: pane1,
                direction: SplitDirection::Horizontal,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        let CommandOutput::PaneCreated {
            pane: pane2,
            tab: Some(tab2),
            session: Some(s2),
            ..
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        assert_eq!(d.state().workspace(ws).unwrap().active_pane, pane2);

        d.dispatch(Command::CloseTab { tab: tab2 }).unwrap();
        assert_eq!(host.kills(), vec![s2]);
        let w = d.state().workspace(ws).unwrap();
        assert_eq!(w.layout, SplitTree::Leaf { pane: pane1 });
        assert!(!w.panes.contains_key(&pane2));
        // 닫힌 pane 이 포커스였으므로 leaf 순서상 첫 pane 으로 fixup.
        assert_eq!(w.active_pane, pane1);
    }

    #[test]
    fn close_last_tab_of_inactive_pane_collapses_and_keeps_focus() {
        let (mut d, host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");
        let out = d
            .dispatch(Command::SplitPane {
                pane: pane1,
                direction: SplitDirection::Vertical,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        let CommandOutput::PaneCreated {
            pane: pane2,
            tab: Some(tab2),
            session: Some(s2),
            ..
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        // 포커스를 pane1 로 되돌린 뒤 비활성 pane2 의 마지막 탭을 닫는다.
        d.dispatch(Command::FocusPane { pane: pane1 }).unwrap();

        d.dispatch(Command::CloseTab { tab: tab2 }).unwrap();
        assert_eq!(host.kills(), vec![s2]);
        let w = d.state().workspace(ws).unwrap();
        assert_eq!(w.layout, SplitTree::Leaf { pane: pane1 });
        assert!(!w.panes.contains_key(&pane2));
        // 포커스는 원래부터 pane1 — fixup 없이 그대로.
        assert_eq!(w.active_pane, pane1);
    }

    #[test]
    fn create_tab_uses_workspace_defaults_for_spawn() {
        let (mut d, host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
                tab: None,
            })
            .unwrap();
        let CommandOutput::WorkspaceCreated { pane, .. } = out else {
            panic!("unexpected output: {out:?}");
        };

        // cwd 미지정 → root_path 상속, cols/rows 는 기본 80×24.
        create_terminal_tab(&mut d, pane);
        // cwd 명시 → 그대로 사용.
        d.dispatch(Command::CreateTab {
            pane,
            tab: NewTab::Terminal {
                cwd: Some("/elsewhere".into()),
            },
        })
        .unwrap();

        let spawns = host.spawns();
        assert_eq!(
            spawns[0],
            ShellSpawnReq {
                cwd: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
                cols: 80,
                rows: 24,
                // 워크스페이스(1)·pane(2) 다음 발급이므로 첫 탭은 3.
                history_tab: Some(3),
            }
        );
        assert_eq!(spawns[1].cwd, Some("/elsewhere".into()));

        // 탭에도 실제 적용된 cwd 가 기록된다.
        let ws = &d.state().workspaces[0];
        let TabKind::Terminal { cwd, .. } = &ws.panes[&pane].tabs[0].kind else {
            panic!("terminal 탭이 아님");
        };
        assert_eq!(cwd.as_deref(), Some("/proj"));
    }

    /// 탭별 명령 history 계약 (체크포인트 2 UX): 스폰 3경로 전부에서 호스트가
    /// 받은 `history_tab` 이 **그 스폰으로 생긴 탭의 안정 ID** 와 같아야 한다.
    /// 스폰이 id 발급보다 먼저라 peek 으로 계산하는 값이므로, 할당 순서가 바뀌면
    /// 핸들러의 debug_assert 와 이 테스트가 함께 터진다.
    #[test]
    fn spawn_carries_history_tab_of_the_created_tab() {
        let (mut d, host) = dispatcher();
        // 1) CreateWorkspace(tab 동반) — 발급 순서 workspace → pane → tab.
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: None,
                distro: None,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        let CommandOutput::WorkspaceCreated {
            pane,
            tab: Some(ws_tab),
            ..
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        // 2) CreateTab — 탭 id 만 발급.
        let (created_tab, _session) = create_terminal_tab(&mut d, pane);
        // 3) SplitPane(tab 동반) — 발급 순서 pane → split → tab.
        let out = d
            .dispatch(Command::SplitPane {
                pane,
                direction: SplitDirection::Vertical,
                tab: Some(NewTab::Terminal { cwd: None }),
            })
            .unwrap();
        let CommandOutput::PaneCreated {
            tab: Some(split_tab),
            ..
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };

        let history: Vec<Option<u64>> = host.spawns().iter().map(|r| r.history_tab).collect();
        assert_eq!(
            history,
            vec![Some(ws_tab.0), Some(created_tab.0), Some(split_tab.0)]
        );
    }

    #[test]
    fn close_tab_adjusts_active_to_previous() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, _) = create_terminal_tab(&mut d, pane);
        let (tab2, _) = create_terminal_tab(&mut d, pane);
        let (tab3, _) = create_terminal_tab(&mut d, pane);

        // active(=tab3) 닫기 → 직전 탭 tab2.
        d.dispatch(Command::CloseTab { tab: tab3 }).unwrap();
        let active = |d: &Dispatcher| d.state().workspace(ws).unwrap().panes[&pane].active_tab;
        assert_eq!(active(&d), Some(tab2));

        // active 가 아닌 첫 탭 닫기 → active 유지.
        d.dispatch(Command::CloseTab { tab: tab1 }).unwrap();
        assert_eq!(active(&d), Some(tab2));

        // 마지막 탭 닫기 → 워크스페이스의 마지막 pane 이므로 collapse 예외:
        // 빈 pane (active_tab = None)으로 남는다 (계획 D6).
        d.dispatch(Command::CloseTab { tab: tab2 }).unwrap();
        assert_eq!(active(&d), None);
        let w = d.state().workspace(ws).unwrap();
        assert!(w.panes[&pane].tabs.is_empty());
        assert_eq!(w.layout, SplitTree::Leaf { pane });
        assert_eq!(w.active_pane, pane);
    }

    #[test]
    fn close_first_tab_while_active_falls_to_next() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, _) = create_terminal_tab(&mut d, pane);
        let (tab2, _) = create_terminal_tab(&mut d, pane);
        d.dispatch(Command::ActivateTab { tab: tab1 }).unwrap();

        // 첫 탭이 active 인 채로 닫기 → 다음 탭(tab2)으로.
        d.dispatch(Command::CloseTab { tab: tab1 }).unwrap();
        assert_eq!(
            d.state().workspace(ws).unwrap().panes[&pane].active_tab,
            Some(tab2)
        );
    }

    #[test]
    fn focus_pane_does_not_switch_workspace() {
        let (mut d, _host) = dispatcher();
        let (ws1, _pane1) = create_ws(&mut d, "one");
        let (ws2, pane2) = create_ws(&mut d, "two");
        let (pane3, _split) = split_empty(&mut d, pane2, SplitDirection::Horizontal);

        d.dispatch(Command::SwitchWorkspace { workspace: ws1 })
            .unwrap();
        assert_eq!(d.state().active_workspace, Some(ws1));
        assert_eq!(d.state().workspace(ws2).unwrap().active_pane, pane3);

        // 비활성 워크스페이스의 pane 포커스 — 그 워크스페이스의 active_pane 만
        // 바뀌고 active_workspace 는 그대로 (명령 직교성).
        d.dispatch(Command::FocusPane { pane: pane2 }).unwrap();
        assert_eq!(d.state().active_workspace, Some(ws1));
        assert_eq!(d.state().workspace(ws2).unwrap().active_pane, pane2);
    }

    #[test]
    fn close_active_workspace_falls_back_to_first_remaining() {
        let (mut d, _host) = dispatcher();
        let (ws1, _) = create_ws(&mut d, "one");
        let (ws2, _) = create_ws(&mut d, "two");
        assert_eq!(d.state().active_workspace, Some(ws2));
        d.dispatch(Command::CloseWorkspace { workspace: ws2 })
            .unwrap();
        assert_eq!(d.state().active_workspace, Some(ws1));

        // 비활성 워크스페이스를 닫으면 active 는 그대로.
        let (ws3, _) = create_ws(&mut d, "three");
        d.dispatch(Command::SwitchWorkspace { workspace: ws1 })
            .unwrap();
        d.dispatch(Command::CloseWorkspace { workspace: ws3 })
            .unwrap();
        assert_eq!(d.state().active_workspace, Some(ws1));
    }

    #[test]
    fn session_exited_marks_tab_and_keeps_session_id() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (_tab, session) = create_terminal_tab(&mut d, pane);
        let rev = d.state().revision;

        d.apply_event(SessionEvent::SessionExited {
            session,
            code: Some(0),
        });
        assert_eq!(d.state().revision, rev + 1);
        let TabKind::Terminal {
            pty_session,
            status,
            ..
        } = &d.state().workspace(ws).unwrap().panes[&pane].tabs[0].kind
        else {
            panic!("terminal 탭이 아님");
        };
        assert_eq!(*pty_session, Some(session), "pty_session 은 유지");
        assert_eq!(*status, TerminalStatus::Exited { code: Some(0) });

        // 동일 이벤트 재도착 → 상태 변화 없음, revision 불변.
        d.apply_event(SessionEvent::SessionExited {
            session,
            code: Some(0),
        });
        assert_eq!(d.state().revision, rev + 1);
    }

    #[test]
    fn session_exited_unknown_session_is_noop() {
        let (mut d, _host) = dispatcher();
        create_ws(&mut d, "ws");
        let before = serde_json::to_value(d.state()).unwrap();
        // CloseTab 선행 후 exit 통지가 도착하는 정상 순서 — 패닉·변이 없어야 한다.
        d.apply_event(SessionEvent::SessionExited {
            session: 999,
            code: None,
        });
        assert_eq!(serde_json::to_value(d.state()).unwrap(), before);
    }

    #[test]
    fn revision_increases_by_one_per_successful_dispatch() {
        let (mut d, _host) = dispatcher();
        assert_eq!(d.state().revision, 0);
        let (_ws, pane) = create_ws(&mut d, "ws");
        assert_eq!(d.state().revision, 1);
        create_terminal_tab(&mut d, pane);
        assert_eq!(d.state().revision, 2);
        assert_eq!(d.snapshot().revision, 2);
    }

    // ---- 뷰어 탭 (21단계 계획 청크 A) ----

    /// root_path 를 지정해 워크스페이스를 만드는 헬퍼 (뷰어 기본값 검증용).
    fn create_ws_rooted(
        d: &mut Dispatcher,
        name: &str,
        root_path: Option<&str>,
    ) -> (WorkspaceId, PaneId) {
        match d
            .dispatch(Command::CreateWorkspace {
                name: name.into(),
                root_path: root_path.map(String::from),
                distro: None,
                tab: None,
            })
            .unwrap()
        {
            CommandOutput::WorkspaceCreated {
                workspace,
                pane,
                tab: None,
                session: None,
            } => (workspace, pane),
            other => panic!("unexpected output: {other:?}"),
        }
    }

    /// 뷰어 탭 생성 헬퍼 — 스폰이 없으므로 출력의 session 은 항상 None 이다.
    fn create_viewer_tab(d: &mut Dispatcher, pane: PaneId, spec: NewTab) -> TabId {
        match d.dispatch(Command::CreateTab { pane, tab: spec }).unwrap() {
            CommandOutput::TabCreated {
                tab,
                session: None,
            } => tab,
            other => panic!("unexpected output: {other:?}"),
        }
    }

    #[test]
    fn create_folder_browser_inherits_root_path_without_spawning() {
        let (mut d, host) = dispatcher();
        let (ws, pane) = create_ws_rooted(&mut d, "ws", Some("/proj/app"));
        let tab = create_viewer_tab(&mut d, pane, NewTab::FolderBrowser { path: None });

        assert!(host.spawns().is_empty(), "뷰어 탭 생성은 스폰이 없다");
        let t = tab_view(&d, tab);
        // path 미지정 → 워크스페이스 root_path 상속, 제목은 basename.
        assert_eq!(
            t.kind,
            TabKind::FolderBrowser {
                path: "/proj/app".into()
            }
        );
        assert_eq!(t.title, "app");
        assert_eq!(
            d.state().workspace(ws).unwrap().panes[&pane].active_tab,
            Some(tab)
        );
    }

    #[test]
    fn folder_browser_falls_back_to_root_when_both_paths_are_none() {
        // 탭 path 도 워크스페이스 root_path 도 없으면 "/" (계획 21단계 core 계약).
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let tab = create_viewer_tab(&mut d, pane, NewTab::FolderBrowser { path: None });
        let t = tab_view(&d, tab);
        assert_eq!(t.kind, TabKind::FolderBrowser { path: "/".into() });
        assert_eq!(t.title, "/");
    }

    #[test]
    fn create_text_viewer_starts_at_offset_zero() {
        let (mut d, host) = dispatcher();
        let (_ws, pane) = create_ws_rooted(&mut d, "ws", Some("/proj"));
        let tab = create_viewer_tab(
            &mut d,
            pane,
            NewTab::TextViewer {
                path: "/proj/notes.txt".into(),
            },
        );
        assert!(host.spawns().is_empty());
        let t = tab_view(&d, tab);
        assert_eq!(
            t.kind,
            TabKind::TextViewer {
                path: "/proj/notes.txt".into(),
                scroll_top: 0.0,
            }
        );
        assert_eq!(t.title, "notes.txt");
    }

    #[test]
    fn create_markdown_viewer_starts_at_pixel_zero() {
        // markdownViewer 는 TextViewer 와 같은 생성 계약(스폰 없음·basename 제목)
        // 이고 scroll_top 만 px 시맨틱이다 (21단계 청크 D).
        let (mut d, host) = dispatcher();
        let (_ws, pane) = create_ws_rooted(&mut d, "ws", Some("/proj"));
        let tab = create_viewer_tab(
            &mut d,
            pane,
            NewTab::MarkdownViewer {
                path: "/proj/README.md".into(),
            },
        );
        assert!(host.spawns().is_empty(), "뷰어 탭 생성은 스폰이 없다");
        let t = tab_view(&d, tab);
        assert_eq!(
            t.kind,
            TabKind::MarkdownViewer {
                path: "/proj/README.md".into(),
                scroll_top: 0.0,
            }
        );
        assert_eq!(t.title, "README.md");
    }

    #[test]
    fn create_workspace_and_split_pane_accept_viewer_tabs_atomically() {
        let (mut d, host) = dispatcher();
        // 워크스페이스 + 뷰어 탭 원자 생성 — session 만 None 이고 tab 은 Some.
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: None,
                tab: Some(NewTab::FolderBrowser { path: None }),
            })
            .unwrap();
        let CommandOutput::WorkspaceCreated {
            workspace,
            pane,
            tab: Some(tab),
            session: None,
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        assert!(workspace.0 < pane.0 && pane.0 < tab.0);
        let p = &d.state().workspace(workspace).unwrap().panes[&pane];
        assert_eq!(p.tabs.len(), 1);
        assert_eq!(p.active_tab, Some(tab));
        assert_eq!(
            p.tabs[0].kind,
            TabKind::FolderBrowser {
                path: "/proj".into()
            }
        );

        // 분할 + 뷰어 탭 원자 생성.
        let out = d
            .dispatch(Command::SplitPane {
                pane,
                direction: SplitDirection::Vertical,
                tab: Some(NewTab::TextViewer {
                    path: "/proj/a.txt".into(),
                }),
            })
            .unwrap();
        let CommandOutput::PaneCreated {
            pane: pane2,
            split,
            tab: Some(tab2),
            session: None,
        } = out
        else {
            panic!("unexpected output: {out:?}");
        };
        assert!(pane2.0 < split.0 && split.0 < tab2.0);
        let w = d.state().workspace(workspace).unwrap();
        assert_eq!(w.active_pane, pane2);
        assert_eq!(w.panes[&pane2].active_tab, Some(tab2));
        assert_eq!(tab_view(&d, tab2).title, "a.txt");
        assert!(host.spawns().is_empty(), "뷰어 탭 생성은 스폰이 없다");
    }

    #[test]
    fn navigate_folder_updates_path_and_title() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let tab = create_viewer_tab(
            &mut d,
            pane,
            NewTab::FolderBrowser {
                path: Some("/proj".into()),
            },
        );

        let out = d
            .dispatch(Command::NavigateFolder {
                tab,
                path: "/proj/src/model".into(),
            })
            .unwrap();
        assert_eq!(out, CommandOutput::Done);
        let t = tab_view(&d, tab);
        assert_eq!(
            t.kind,
            TabKind::FolderBrowser {
                path: "/proj/src/model".into()
            }
        );
        assert_eq!(t.title, "model");

        // 루트로 올라가면 제목도 "/" (basename 이 없는 경로).
        d.dispatch(Command::NavigateFolder {
            tab,
            path: "/".into(),
        })
        .unwrap();
        let t = tab_view(&d, tab);
        assert_eq!(t.kind, TabKind::FolderBrowser { path: "/".into() });
        assert_eq!(t.title, "/");
    }

    #[test]
    fn viewer_commands_reject_wrong_tab_kinds_without_state_change() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (terminal, _s) = create_terminal_tab(&mut d, pane);
        let folder = create_viewer_tab(
            &mut d,
            pane,
            NewTab::FolderBrowser {
                path: Some("/proj".into()),
            },
        );
        let text = create_viewer_tab(
            &mut d,
            pane,
            NewTab::TextViewer {
                path: "/proj/a.txt".into(),
            },
        );
        let markdown = create_viewer_tab(
            &mut d,
            pane,
            NewTab::MarkdownViewer {
                path: "/proj/a.md".into(),
            },
        );
        let before = serde_json::to_value(d.state()).unwrap();

        let cases = [
            // NavigateFolder 는 folderBrowser 만 받는다.
            (
                Command::NavigateFolder {
                    tab: terminal,
                    path: "/x".into(),
                },
                terminal,
            ),
            (
                Command::NavigateFolder {
                    tab: text,
                    path: "/x".into(),
                },
                text,
            ),
            (
                Command::NavigateFolder {
                    tab: markdown,
                    path: "/x".into(),
                },
                markdown,
            ),
            // SetViewerScroll 은 스크롤 위치를 모델에 가진 뷰어만 받는다 —
            // folderBrowser 는 그 필드가 없어 KindMismatch (기결정).
            (
                Command::SetViewerScroll {
                    tab: folder,
                    scroll_top: 10.0,
                },
                folder,
            ),
            (
                Command::SetViewerScroll {
                    tab: terminal,
                    scroll_top: 10.0,
                },
                terminal,
            ),
        ];
        for (cmd, target) in cases {
            let err = d.dispatch(cmd.clone()).unwrap_err();
            assert_eq!(err, CommandError::KindMismatch { tab: target }, "{cmd:?}");
            assert_eq!(
                serde_json::to_value(d.state()).unwrap(),
                before,
                "{cmd:?} 가 상태를 바꿈"
            );
        }
    }

    #[test]
    fn viewer_paths_are_validated_on_create_and_navigate() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let folder = create_viewer_tab(
            &mut d,
            pane,
            NewTab::FolderBrowser {
                path: Some("/proj".into()),
            },
        );
        let before = serde_json::to_value(d.state()).unwrap();

        // wslpath 거부 규칙별 대표 1개씩 — 사유는 wslpath.rs 테스트가 잠근다.
        for path in [
            "relative/path",
            "/proj/../etc",
            r"/proj/a\b",
            "/proj/a:stream",
            "/proj/trailing.",
            "",
        ] {
            for cmd in [
                Command::CreateTab {
                    pane,
                    tab: NewTab::TextViewer { path: path.into() },
                },
                Command::CreateTab {
                    pane,
                    tab: NewTab::MarkdownViewer { path: path.into() },
                },
                Command::CreateTab {
                    pane,
                    tab: NewTab::FolderBrowser {
                        path: Some(path.into()),
                    },
                },
                Command::NavigateFolder {
                    tab: folder,
                    path: path.into(),
                },
            ] {
                let err = d.dispatch(cmd.clone()).unwrap_err();
                assert!(
                    matches!(err, CommandError::InvalidPath { .. }),
                    "{cmd:?} → {err:?}"
                );
                // next_id 까지 불변 (검증이 id 발급보다 먼저).
                assert_eq!(
                    serde_json::to_value(d.state()).unwrap(),
                    before,
                    "{cmd:?} 가 상태를 바꿈"
                );
            }
        }
    }

    #[test]
    fn set_viewer_scroll_records_offset_and_rejects_bad_values() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let tab = create_viewer_tab(
            &mut d,
            pane,
            NewTab::TextViewer {
                path: "/proj/a.txt".into(),
            },
        );

        let out = d
            .dispatch(Command::SetViewerScroll {
                tab,
                scroll_top: 4096.0,
            })
            .unwrap();
        assert_eq!(out, CommandOutput::Done);
        assert_eq!(
            tab_view(&d, tab).kind,
            TabKind::TextViewer {
                path: "/proj/a.txt".into(),
                scroll_top: 4096.0,
            }
        );

        // finite·0 이상이 아니면 InvalidScroll, 상태 불변 (0.0 은 유효 경계).
        let before = serde_json::to_value(d.state()).unwrap();
        for value in [f64::NAN, -1.0, f64::INFINITY, f64::NEG_INFINITY] {
            let err = d
                .dispatch(Command::SetViewerScroll {
                    tab,
                    scroll_top: value,
                })
                .unwrap_err();
            assert!(
                matches!(err, CommandError::InvalidScroll { .. }),
                "{value} → {err:?}"
            );
            assert_eq!(
                serde_json::to_value(d.state()).unwrap(),
                before,
                "{value} 가 상태를 바꿈"
            );
        }
        d.dispatch(Command::SetViewerScroll {
            tab,
            scroll_top: 0.0,
        })
        .unwrap();
    }

    #[test]
    fn set_viewer_scroll_records_pixel_offset_for_markdown_viewer() {
        // 같은 명령이 markdownViewer 도 받는다 — 값은 렌더 px 지만 코어는 단위를
        // 해석하지 않고 f64 를 그대로 보관한다 (21단계 청크 D).
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let tab = create_viewer_tab(
            &mut d,
            pane,
            NewTab::MarkdownViewer {
                path: "/proj/README.md".into(),
            },
        );

        let out = d
            .dispatch(Command::SetViewerScroll {
                tab,
                scroll_top: 120.5,
            })
            .unwrap();
        assert_eq!(out, CommandOutput::Done);
        assert_eq!(
            tab_view(&d, tab).kind,
            TabKind::MarkdownViewer {
                path: "/proj/README.md".into(),
                scroll_top: 120.5,
            }
        );

        let before = serde_json::to_value(d.state()).unwrap();
        let err = d
            .dispatch(Command::SetViewerScroll {
                tab,
                scroll_top: -1.0,
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::InvalidScroll { .. }), "{err:?}");
        assert_eq!(serde_json::to_value(d.state()).unwrap(), before);
    }

    #[test]
    fn viewer_tabs_survive_persist_round_trip() {
        // 뷰어 탭은 sanitize 대상이 아니다 — 경로·스크롤이 재시작을 넘어 남아야
        // 뷰어 재로드(계획 v2 "상태 저장")가 성립한다.
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws_rooted(&mut d, "ws", Some("/proj"));
        let (terminal, _s) = create_terminal_tab(&mut d, pane);
        let folder = create_viewer_tab(&mut d, pane, NewTab::FolderBrowser { path: None });
        let text = create_viewer_tab(
            &mut d,
            pane,
            NewTab::TextViewer {
                path: "/proj/notes.txt".into(),
            },
        );
        let markdown = create_viewer_tab(
            &mut d,
            pane,
            NewTab::MarkdownViewer {
                path: "/proj/README.md".into(),
            },
        );
        d.dispatch(Command::SetViewerScroll {
            tab: text,
            scroll_top: 4096.0,
        })
        .unwrap();
        d.dispatch(Command::SetViewerScroll {
            tab: markdown,
            scroll_top: 120.5,
        })
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        crate::persist::save_atomic(&path, d.state()).unwrap();
        let crate::persist::LoadOutcome::Restored { state, repairs } =
            crate::persist::load(&path)
        else {
            panic!("Restored 여야 함");
        };
        assert!(repairs.is_empty(), "정상 상태에 수리 사유: {repairs:?}");

        let kind_of = |state: &AppState, id: TabId| {
            state
                .workspaces
                .iter()
                .flat_map(|ws| ws.panes.values())
                .flat_map(|p| p.tabs.iter())
                .find(|t| t.id == id)
                .expect("탭이 존재해야 함")
                .kind
                .clone()
        };
        assert_eq!(
            kind_of(&state, folder),
            TabKind::FolderBrowser {
                path: "/proj".into()
            }
        );
        assert_eq!(
            kind_of(&state, text),
            TabKind::TextViewer {
                path: "/proj/notes.txt".into(),
                scroll_top: 4096.0,
            },
            "sanitize 가 뷰어 스크롤을 건드리면 안 된다"
        );
        assert_eq!(
            kind_of(&state, markdown),
            TabKind::MarkdownViewer {
                path: "/proj/README.md".into(),
                scroll_top: 120.5,
            }
        );

        // 재스폰 대상은 terminal 탭뿐 — 뷰어 탭은 열거에 끼지 않는다.
        let adopted = Dispatcher::adopt(state, Box::new(FakeSessionHost::default()));
        assert_eq!(adopted.running_terminal_tabs(), vec![terminal]);
    }

    // ---- OSC 델타 반영 (18단계 계획 core 계약) ----

    fn status_notify(token: &str, body: &str) -> OscEvent {
        OscEvent::Osc777Notify {
            title: token.into(),
            body: body.into(),
        }
    }

    /// (세션, 이벤트) 목록을 한 배치로 병합한다 — 글루의 flush 창 한 번에 해당.
    fn batch(events: &[(SessionId, OscEvent)]) -> OscBatch {
        let mut b = OscBatch::default();
        for (session, ev) in events {
            b.merge(*session, ev);
        }
        b
    }

    /// 워크스페이스의 에이전트 상태 3종 (상태, 출처 탭, 미리보기 메시지).
    fn agent(d: &Dispatcher, ws: WorkspaceId) -> (AgentStatus, Option<TabId>, Option<String>) {
        let w = d.state().workspace(ws).unwrap();
        (
            w.agent_status,
            w.agent_status_source,
            w.last_agent_message.clone(),
        )
    }

    /// 탭 값 관측 헬퍼 — 전 워크스페이스 범위 탐색.
    fn tab_view(d: &Dispatcher, tab: TabId) -> &Tab {
        d.state()
            .workspaces
            .iter()
            .flat_map(|ws| ws.panes.values())
            .flat_map(|pane| pane.tabs.iter())
            .find(|t| t.id == tab)
            .expect("탭이 존재해야 함")
    }

    fn tab_cwd(d: &Dispatcher, tab: TabId) -> Option<String> {
        let TabKind::Terminal { cwd, .. } = &tab_view(d, tab).kind else {
            panic!("terminal 탭이어야 함");
        };
        cwd.clone()
    }

    #[test]
    fn apply_osc_routes_delta_to_the_tab_owning_the_session() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        // 나중에 만든 tab2 가 active — tab1 은 뒤에 숨는다.
        let (tab2, _s2) = create_terminal_tab(&mut d, pane);
        let rev = d.state().revision;

        let changed = d.apply_osc(
            batch(&[
                (s1, OscEvent::Osc0Title("agent".into())),
                (s1, OscEvent::Osc7Cwd("file://h/home/u/my%20proj".into())),
                (s1, status_notify("wmux:needsInput", "approve?")),
            ]),
            1_000,
        );
        assert!(changed);
        assert_eq!(d.state().revision, rev + 1);

        // 역매핑된 탭에만 필드가 반영된다.
        let t1 = tab_view(&d, tab1);
        assert_eq!(t1.title, "agent");
        assert_eq!(t1.notification, NotificationState::Unread);
        assert_eq!(t1.last_activity_ms, Some(1_000));
        assert_eq!(tab_cwd(&d, tab1).as_deref(), Some("/home/u/my proj"));

        // 같은 pane 의 다른 탭은 그대로.
        let t2 = tab_view(&d, tab2);
        assert_eq!(t2.title, "Terminal");
        assert_eq!(t2.notification, NotificationState::None);
        assert_eq!(t2.last_activity_ms, None);
        assert_eq!(tab_cwd(&d, tab2), None);

        // 워크스페이스 레벨 상태 + 출처 탭 기록.
        assert_eq!(
            agent(&d, ws),
            (
                AgentStatus::NeedsInput,
                Some(tab1),
                Some("approve?".to_owned())
            )
        );
    }

    #[test]
    fn apply_osc_unknown_session_is_noop() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        create_terminal_tab(&mut d, pane);
        let before = serde_json::to_value(d.state()).unwrap();

        // 창이 열려 있는 동안 탭이 닫히는 정상 순서 — 패닉·변이 없이 false.
        assert!(!d.apply_osc(batch(&[(999, status_notify("wmux:needsInput", "x"))]), 1_000));
        assert_eq!(serde_json::to_value(d.state()).unwrap(), before);
    }

    #[test]
    fn apply_osc_skips_whole_delta_for_exited_tab() {
        // 100ms 창 안에서 세션이 끝나면 즉시 처리된 SessionExited 의 Idle 리셋 뒤에
        // 지연 배치가 도착한다 — 죽은 탭에 알림·상태를 다시 도장하면 안 된다.
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (_tab, session) = create_terminal_tab(&mut d, pane);
        d.apply_osc(
            batch(&[(session, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );
        d.apply_event(SessionEvent::SessionExited {
            session,
            code: Some(0),
        });
        assert_eq!(agent(&d, ws).0, AgentStatus::Idle);
        let before = serde_json::to_value(d.state()).unwrap();

        // 제목·활동 시각까지 통째로 스킵 — 상태 변화 없음.
        assert!(!d.apply_osc(
            batch(&[
                (session, OscEvent::Osc0Title("late".into())),
                (session, status_notify("wmux:needsInput", "still?")),
            ]),
            2_000,
        ));
        assert_eq!(serde_json::to_value(d.state()).unwrap(), before);
    }

    #[test]
    fn apply_osc_keeps_needs_input_against_other_tabs() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        let (tab2, s2) = create_terminal_tab(&mut d, pane);
        d.apply_osc(
            batch(&[(s1, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );
        assert_eq!(agent(&d, ws).1, Some(tab1));

        // 다른 탭의 running·idle 은 입력 대기를 가리지 못한다 (출처도 유지).
        for token in ["wmux:running", "wmux:idle"] {
            d.apply_osc(batch(&[(s2, status_notify(token, ""))]), 2_000);
            assert_eq!(agent(&d, ws).0, AgentStatus::NeedsInput, "{token}");
            assert_eq!(agent(&d, ws).1, Some(tab1), "{token}");
        }

        // 단 다른 탭의 needsInput 은 반영되고 출처가 그 탭으로 옮겨간다.
        d.apply_osc(
            batch(&[(s2, status_notify("wmux:needsInput", "second"))]),
            3_000,
        );
        assert_eq!(agent(&d, ws).0, AgentStatus::NeedsInput);
        assert_eq!(agent(&d, ws).1, Some(tab2));
    }

    #[test]
    fn apply_osc_lets_the_same_source_leave_needs_input() {
        // 사용자가 응답하면 같은 출처 탭의 UserPromptSubmit(running)이 자연 강등한다.
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        d.apply_osc(
            batch(&[(s1, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );
        assert!(d.apply_osc(batch(&[(s1, status_notify("wmux:running", ""))]), 2_000));
        assert_eq!(
            agent(&d, ws),
            (
                AgentStatus::Running,
                Some(tab1),
                // 빈 body 는 앞선 메시지를 지우지 않는다 (notify.rs last-non-empty).
                Some("approve?".to_owned())
            )
        );
    }

    #[test]
    fn apply_osc_suppresses_unread_on_visible_tab_only() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        let (tab2, s2) = create_terminal_tab(&mut d, pane);

        d.apply_osc(
            batch(&[
                (s1, status_notify("wmux:idle", "done")),
                (s2, status_notify("wmux:idle", "done")),
            ]),
            1_000,
        );
        // 가시 탭(active 워크스페이스 + 그 pane 의 active_tab)은 억제, 숨은 탭은 세팅.
        assert_eq!(tab_view(&d, tab2).notification, NotificationState::None);
        assert_eq!(tab_view(&d, tab1).notification, NotificationState::Unread);

        // 다른 워크스페이스로 나가면 같은 탭도 비가시가 된다.
        create_ws(&mut d, "other");
        d.apply_osc(batch(&[(s2, status_notify("wmux:idle", "done"))]), 2_000);
        assert_eq!(tab_view(&d, tab2).notification, NotificationState::Unread);
    }

    #[test]
    fn activate_tab_clears_unread() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        create_terminal_tab(&mut d, pane);
        d.apply_osc(batch(&[(s1, status_notify("wmux:idle", "done"))]), 1_000);
        assert_eq!(tab_view(&d, tab1).notification, NotificationState::Unread);

        d.dispatch(Command::ActivateTab { tab: tab1 }).unwrap();
        assert_eq!(tab_view(&d, tab1).notification, NotificationState::None);
    }

    #[test]
    fn switch_workspace_clears_unread_of_each_panes_active_tab() {
        let (mut d, _host) = dispatcher();
        let (ws1, pane1) = create_ws(&mut d, "one");
        let (tab_a, sa) = create_terminal_tab(&mut d, pane1);
        let (tab_b, sb) = create_terminal_tab(&mut d, pane1);
        let (pane2, _split) = split_empty(&mut d, pane1, SplitDirection::Horizontal);
        let (tab_c, sc) = create_terminal_tab(&mut d, pane2);
        // 다른 워크스페이스로 나가 전 탭을 비가시로 만든 뒤 알림을 세운다.
        create_ws(&mut d, "two");
        d.apply_osc(
            batch(&[
                (sa, status_notify("wmux:idle", "a")),
                (sb, status_notify("wmux:idle", "b")),
                (sc, status_notify("wmux:idle", "c")),
            ]),
            1_000,
        );
        for tab in [tab_a, tab_b, tab_c] {
            assert_eq!(tab_view(&d, tab).notification, NotificationState::Unread);
        }

        d.dispatch(Command::SwitchWorkspace { workspace: ws1 })
            .unwrap();
        // 각 pane 의 active_tab 만 해제 — 뒤에 숨은 탭 a 는 그대로.
        assert_eq!(tab_view(&d, tab_b).notification, NotificationState::None);
        assert_eq!(tab_view(&d, tab_c).notification, NotificationState::None);
        assert_eq!(tab_view(&d, tab_a).notification, NotificationState::Unread);
    }

    #[test]
    fn close_tab_clears_unread_of_promoted_tab_when_visible() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (tab_a, sa) = create_terminal_tab(&mut d, pane);
        let (tab_b, _sb) = create_terminal_tab(&mut d, pane);
        // tab_b 가 active 라 tab_a 는 숨어 있다 — unread 가 선다.
        d.apply_osc(batch(&[(sa, status_notify("wmux:idle", "a"))]), 1_000);
        assert_eq!(tab_view(&d, tab_a).notification, NotificationState::Unread);

        // active 탭을 닫으면 tab_a 가 승격돼 곧바로 화면에 드러난다 — 가시화 = 읽음.
        d.dispatch(Command::CloseTab { tab: tab_b }).unwrap();
        assert_eq!(tab_view(&d, tab_a).notification, NotificationState::None);
    }

    #[test]
    fn close_tab_keeps_unread_of_promoted_tab_in_background_workspace() {
        let (mut d, _host) = dispatcher();
        let (_ws1, pane) = create_ws(&mut d, "one");
        let (tab_a, sa) = create_terminal_tab(&mut d, pane);
        let (tab_b, _sb) = create_terminal_tab(&mut d, pane);
        create_ws(&mut d, "two"); // ws1 전체가 비가시로.
        d.apply_osc(batch(&[(sa, status_notify("wmux:idle", "a"))]), 1_000);

        // 백그라운드 워크스페이스 안의 승격은 가시화가 아니다 — unread 유지.
        d.dispatch(Command::CloseTab { tab: tab_b }).unwrap();
        assert_eq!(tab_view(&d, tab_a).notification, NotificationState::Unread);
    }

    #[test]
    fn close_workspace_fallback_clears_unread_of_newly_visible_tabs() {
        let (mut d, _host) = dispatcher();
        let (_ws1, pane) = create_ws(&mut d, "one");
        let (tab_a, sa) = create_terminal_tab(&mut d, pane);
        let (tab_b, sb) = create_terminal_tab(&mut d, pane);
        let (ws2, _pane2) = create_ws(&mut d, "two"); // ws1 비가시 상태에서 알림.
        d.apply_osc(
            batch(&[
                (sa, status_notify("wmux:idle", "a")),
                (sb, status_notify("wmux:idle", "b")),
            ]),
            1_000,
        );

        // active 워크스페이스를 닫으면 ws1 이 fallback 으로 드러난다 —
        // SwitchWorkspace 와 같은 규칙: active_tab(b)만 해제, 숨은 a 는 유지.
        d.dispatch(Command::CloseWorkspace { workspace: ws2 })
            .unwrap();
        assert_eq!(tab_view(&d, tab_b).notification, NotificationState::None);
        assert_eq!(tab_view(&d, tab_a).notification, NotificationState::Unread);
    }

    #[test]
    fn close_tab_resets_agent_status_source() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        create_terminal_tab(&mut d, pane);
        d.apply_osc(
            batch(&[(s1, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );

        d.dispatch(Command::CloseTab { tab: tab1 }).unwrap();
        // 상태·출처만 되돌아간다 (미리보기 메시지는 리셋 대상이 아니다).
        assert_eq!(
            agent(&d, ws),
            (AgentStatus::Idle, None, Some("approve?".to_owned()))
        );
    }

    #[test]
    fn close_pane_resets_agent_status_source_for_any_removed_tab() {
        // pane 의 두 번째 탭이 출처여도 리셋된다 — 제거되는 탭 전부를 확인하지
        // 않으면 죽은 탭의 needsInput 이 사이드바에 영원히 남는다.
        let (mut d, _host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");
        let (pane2, _split) = split_empty(&mut d, pane1, SplitDirection::Vertical);
        create_terminal_tab(&mut d, pane2);
        let (second, s2) = create_terminal_tab(&mut d, pane2);
        d.apply_osc(
            batch(&[(s2, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );
        assert_eq!(agent(&d, ws).1, Some(second));

        d.dispatch(Command::ClosePane { pane: pane2 }).unwrap();
        assert_eq!(agent(&d, ws).0, AgentStatus::Idle);
        assert_eq!(agent(&d, ws).1, None);
    }

    #[test]
    fn session_exited_resets_agent_status_source() {
        let (mut d, _host) = dispatcher();
        let (ws, pane) = create_ws(&mut d, "ws");
        let (tab1, s1) = create_terminal_tab(&mut d, pane);
        let (tab2, s2) = create_terminal_tab(&mut d, pane);
        d.apply_osc(
            batch(&[(s2, status_notify("wmux:needsInput", "approve?"))]),
            1_000,
        );
        assert_eq!(agent(&d, ws).1, Some(tab2));

        // 출처가 아닌 탭의 종료는 상태를 건드리지 않는다.
        d.apply_event(SessionEvent::SessionExited {
            session: s1,
            code: Some(0),
        });
        assert_eq!(agent(&d, ws).0, AgentStatus::NeedsInput);
        assert_eq!(agent(&d, ws).1, Some(tab2));

        // 출처 탭의 종료는 Idle 로 되돌린다.
        let rev = d.state().revision;
        d.apply_event(SessionEvent::SessionExited {
            session: s2,
            code: Some(0),
        });
        assert_eq!(agent(&d, ws).0, AgentStatus::Idle);
        assert_eq!(agent(&d, ws).1, None);
        assert_eq!(d.state().revision, rev + 1);
        let _ = tab1;
    }

    #[test]
    fn apply_osc_bumps_revision_once_per_batch() {
        let (mut d, _host) = dispatcher();
        let (_ws, pane) = create_ws(&mut d, "ws");
        let (_tab1, s1) = create_terminal_tab(&mut d, pane);
        let (_tab2, s2) = create_terminal_tab(&mut d, pane);
        let rev = d.state().revision;

        // 두 세션 × 여러 이벤트가 한 배치에 모여도 revision 은 1회만 오른다.
        assert!(d.apply_osc(
            batch(&[
                (s1, OscEvent::Osc0Title("a".into())),
                (s1, status_notify("wmux:running", "one")),
                (s2, OscEvent::Osc0Title("b".into())),
                (s2, status_notify("wmux:idle", "two")),
            ]),
            1_000,
        ));
        assert_eq!(d.state().revision, rev + 1);

        // 바뀔 것이 없는 배치는 false — 글루가 스냅샷 발행을 건너뛴다.
        assert!(!d.apply_osc(
            batch(&[
                (s2, OscEvent::Osc0Title("b".into())),
                (s2, status_notify("wmux:idle", "two")),
            ]),
            1_000,
        ));
        assert_eq!(d.state().revision, rev + 1);
    }

    /// pty_session 없는 터미널 탭 값 — persist sanitize 직후 형태.
    fn sessionless_tab(id: u64, status: TerminalStatus, cwd: Option<&str>) -> Tab {
        Tab {
            id: TabId(id),
            title: format!("tab-{id}"),
            kind: TabKind::Terminal {
                pty_session: None,
                status,
                cwd: cwd.map(String::from),
            },
            notification: NotificationState::None,
            last_activity_ms: None,
        }
    }

    /// 복원 직후(sanitize 완료) 모양의 상태 — ws 1, split 4 아래 pane 2·3.
    /// pane 2: tab 5 (Running, cwd 없음 → root_path 상속 대상).
    /// pane 3: tab 6 (Running, cwd /custom), tab 7 (Exited — 재스폰 비대상).
    /// next_id 8, revision 3.
    fn adopted_state() -> AppState {
        AppState {
            workspaces: vec![Workspace {
                id: WorkspaceId(1),
                name: "restored".into(),
                root_path: Some("/root".into()),
                distro: Some("Ubuntu".into()),
                git_branch: None,
                git_dirty: None,
                layout: SplitTree::Split {
                    id: SplitId(4),
                    direction: SplitDirection::Horizontal,
                    ratio: 0.5,
                    first: Box::new(SplitTree::Leaf { pane: PaneId(2) }),
                    second: Box::new(SplitTree::Leaf { pane: PaneId(3) }),
                },
                panes: [
                    (
                        PaneId(2),
                        Pane {
                            id: PaneId(2),
                            tabs: vec![sessionless_tab(5, TerminalStatus::Running, None)],
                            active_tab: Some(TabId(5)),
                        },
                    ),
                    (
                        PaneId(3),
                        Pane {
                            id: PaneId(3),
                            tabs: vec![
                                sessionless_tab(6, TerminalStatus::Running, Some("/custom")),
                                sessionless_tab(7, TerminalStatus::Exited { code: Some(1) }, None),
                            ],
                            active_tab: Some(TabId(6)),
                        },
                    ),
                ]
                .into(),
                active_pane: PaneId(2),
                agent_status: AgentStatus::Idle,
                last_agent_message: None,
                agent_status_source: None,
            }],
            active_workspace: Some(WorkspaceId(1)),
            next_id: 8,
            revision: 3,
        }
    }

    fn adopted_dispatcher() -> (Dispatcher, FakeSessionHost) {
        let host = FakeSessionHost::default();
        (
            Dispatcher::adopt(adopted_state(), Box::new(host.clone())),
            host,
        )
    }

    /// `tab.kind` 의 (pty_session, status) 를 읽는 관측 헬퍼.
    fn terminal_kind(
        d: &Dispatcher,
        pane: PaneId,
        ti: usize,
    ) -> (Option<SessionId>, TerminalStatus) {
        let TabKind::Terminal {
            pty_session,
            status,
            ..
        } = &d.state().workspaces[0].panes[&pane].tabs[ti].kind
        else {
            panic!("terminal 탭이어야 함");
        };
        (*pty_session, *status)
    }

    #[test]
    fn adopt_takes_state_without_spawning() {
        let (d, host) = adopted_dispatcher();
        // 채택만 — 스폰·kill 부수효과 전혀 없음, 상태 원본 그대로.
        assert!(host.spawns().is_empty());
        assert!(host.kills().is_empty());
        assert_eq!(*d.state(), adopted_state());
    }

    #[test]
    fn running_terminal_tabs_lists_only_sessionless_running() {
        let (mut d, _host) = adopted_dispatcher();
        // Exited 탭(7)은 제외 — Running·pty_session None 인 5·6 만.
        assert_eq!(d.running_terminal_tabs(), vec![TabId(5), TabId(6)]);
        // 재스폰돼 세션이 채워진 탭은 열거에서 빠진다.
        d.respawn_tab(TabId(5)).unwrap();
        assert_eq!(d.running_terminal_tabs(), vec![TabId(6)]);
    }

    #[test]
    fn respawn_fills_session_and_bumps_revision() {
        let (mut d, host) = adopted_dispatcher();
        let s5 = d.respawn_tab(TabId(5)).unwrap();
        let s6 = d.respawn_tab(TabId(6)).unwrap();
        assert_eq!((s5, s6), (1, 2));
        assert_eq!(
            terminal_kind(&d, PaneId(2), 0),
            (Some(s5), TerminalStatus::Running)
        );
        assert_eq!(
            terminal_kind(&d, PaneId(3), 0),
            (Some(s6), TerminalStatus::Running)
        );
        // 회당 revision += 1 (스냅샷 전파) — adopted revision 3 에서 시작.
        assert_eq!(d.state().revision, 5);

        // 스폰 파라미터: cwd = 탭 cwd(없으면 root_path), distro = ws 기본, 80×24,
        // history_tab = 재스폰 대상 탭의 id (재시작 전과 같은 history 파일).
        let spawns = host.spawns();
        assert_eq!(
            spawns[0],
            ShellSpawnReq {
                cwd: Some("/root".into()),
                distro: Some("Ubuntu".into()),
                cols: 80,
                rows: 24,
                history_tab: Some(5),
            }
        );
        assert_eq!(spawns[1].cwd.as_deref(), Some("/custom"));
        // 탭에 기록된 cwd 는 재스폰이 바꾸지 않는다 (생성 시점 값 보존).
        let TabKind::Terminal { cwd, .. } = &d.state().workspaces[0].panes[&PaneId(2)].tabs[0].kind
        else {
            panic!("terminal 탭이어야 함");
        };
        assert_eq!(*cwd, None);
    }

    /// 재스폰은 탭 id 를 그대로 history 키로 쓴다 — 재시작 후에도 같은 탭이 같은
    /// history 파일을 물게 하는 것이 탭별 history 의 요점이다 (체크포인트 2 UX).
    #[test]
    fn respawn_carries_history_tab_of_the_same_tab() {
        let (mut d, host) = adopted_dispatcher();
        d.respawn_tab(TabId(5)).unwrap();
        d.respawn_tab(TabId(6)).unwrap();
        let history: Vec<Option<u64>> = host.spawns().iter().map(|r| r.history_tab).collect();
        assert_eq!(history, vec![Some(TabId(5).0), Some(TabId(6).0)]);
    }

    #[test]
    fn respawn_failure_demotes_tab_to_exited() {
        let (mut d, host) = adopted_dispatcher();
        host.set_fail_spawn(true);
        let err = d.respawn_tab(TabId(5)).unwrap_err();
        assert!(matches!(err, CommandError::SpawnFailed { .. }), "{err:?}");
        // 강등이 설계된 결과 상태 — pty_session 은 None 유지, revision 반영.
        assert_eq!(
            terminal_kind(&d, PaneId(2), 0),
            (None, TerminalStatus::Exited { code: None })
        );
        assert_eq!(d.state().revision, 4);
        // 강등된 탭은 이후 재스폰 대상이 아니다 (부적합 = UnknownTarget).
        host.set_fail_spawn(false);
        let err = d.respawn_tab(TabId(5)).unwrap_err();
        assert!(matches!(err, CommandError::UnknownTarget { .. }), "{err:?}");
        assert_eq!(d.running_terminal_tabs(), vec![TabId(6)]);
    }

    #[test]
    fn respawn_rejects_ineligible_targets_without_state_change() {
        let (mut d, host) = adopted_dispatcher();
        let s5 = d.respawn_tab(TabId(5)).unwrap();
        let before = serde_json::to_value(d.state()).unwrap();
        // 부적합 3종: Exited 저장 탭 / 이미 세션 있는 탭 / 미지 id — 전부
        // UnknownTarget 에러이고 상태·revision 불변 (부적합 호출 = 프로그램 결함).
        for tab in [TabId(7), TabId(5), TabId(99)] {
            let err = d.respawn_tab(tab).unwrap_err();
            assert!(
                matches!(err, CommandError::UnknownTarget { .. }),
                "tab {tab:?} → {err:?}"
            );
            assert_eq!(
                serde_json::to_value(d.state()).unwrap(),
                before,
                "tab {tab:?} 가 상태를 바꿈"
            );
        }
        // 성공한 첫 재스폰 외에 스폰이 더 일어나지 않았다.
        assert_eq!(host.spawns().len(), 1);
        assert_eq!(
            terminal_kind(&d, PaneId(2), 0),
            (Some(s5), TerminalStatus::Running)
        );
    }

    #[test]
    fn adopt_preserves_id_continuity_for_later_dispatch() {
        let (mut d, _host) = adopted_dispatcher();
        d.respawn_tab(TabId(5)).unwrap();
        d.respawn_tab(TabId(6)).unwrap();
        // adopt 된 next_id(8)에서 발급이 이어진다 — 복원 후 생성이 기존 id 와
        // 충돌하지 않는다 (dispatch 가 debug 불변식 검사도 수행).
        let (tab, _session) = create_terminal_tab(&mut d, PaneId(2));
        assert_eq!(tab, TabId(8));
        assert_eq!(d.state().next_id, 9);
    }
}
