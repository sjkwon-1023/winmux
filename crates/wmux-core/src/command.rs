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

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model::{
    AgentStatus, AppState, NotificationState, Pane, PaneId, SplitDirection, SplitTree, Tab, TabId,
    TabKind, TerminalStatus, Workspace, WorkspaceId,
};
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
    /// 워크스페이스 + 빈 pane 1개(Leaf)를 만들고 active 로 지정한다.
    CreateWorkspace {
        name: String,
        root_path: Option<String>,
        distro: Option<String>,
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
    /// `pane` 의 leaf 를 분할해 새 빈 pane 을 second(우/하)로 만들고 포커스를 새
    /// pane 으로 옮긴다.
    SplitPane {
        pane: PaneId,
        direction: SplitDirection,
    },
    /// 소속 세션 kill + tree collapse. 워크스페이스의 마지막 pane 은 닫을 수 없다
    /// ([`CommandError::LastPane`]).
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
    /// 으면 다음 탭, 마지막 탭이었으면 None — 빈 pane 허용).
    CloseTab {
        tab: TabId,
    },
}

/// CreateTab 의 탭 명세. 뷰어 variant 는 21단계에 추가된다 — variant 자체를
/// 생략해 미구현 경로가 타입 수준에서 존재하지 않게 한다.
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
}

/// dispatch 성공 결과. 생성된 안정 ID 를 돌려줘 dev 훅·MCP 가 후속 조작에 쓴다.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CommandOutput {
    WorkspaceCreated {
        workspace: WorkspaceId,
        pane: PaneId,
    },
    /// SplitPane 결과 — 새로 생긴 빈 pane.
    PaneCreated { pane: PaneId },
    TabCreated {
        tab: TabId,
        /// terminal 탭이면 스폰된 PTY 세션 id. (뷰어 탭은 None — 21단계.)
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
}

impl Default for ShellSpawnReq {
    fn default() -> Self {
        Self {
            cwd: None,
            distro: None,
            cols: 80,
            rows: 24,
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
                                }
                            }
                        }
                    }
                }
                // 미지 session 이면 changed == false — 무해한 no-op (모듈 doc 참조).
                if changed {
                    self.state.revision += 1;
                }
            }
        }
    }

    fn execute(&mut self, cmd: Command) -> Result<CommandOutput, CommandError> {
        match cmd {
            Command::CreateWorkspace {
                name,
                root_path,
                distro,
            } => {
                let workspace = WorkspaceId(self.state.alloc_id());
                let pane = PaneId(self.state.alloc_id());
                self.state.workspaces.push(Workspace {
                    id: workspace,
                    name,
                    root_path,
                    distro,
                    git_branch: None,
                    git_dirty: None,
                    layout: SplitTree::Leaf { pane },
                    panes: [(pane, empty_pane(pane))].into(),
                    active_pane: pane,
                    agent_status: AgentStatus::Idle,
                    last_agent_message: None,
                });
                self.state.active_workspace = Some(workspace);
                Ok(CommandOutput::WorkspaceCreated { workspace, pane })
            }

            Command::SwitchWorkspace { workspace } => {
                if self.state.workspace(workspace).is_none() {
                    return Err(unknown("workspace", workspace.0));
                }
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
                    // 없으면 None (마지막 워크스페이스 닫기 허용).
                    self.state.active_workspace = self.state.workspaces.first().map(|ws| ws.id);
                }
                Ok(CommandOutput::Done)
            }

            Command::FocusPane { pane } => {
                let wi = self.ws_index_of_pane(pane)?;
                self.state.workspaces[wi].active_pane = pane;
                Ok(CommandOutput::Done)
            }

            Command::SplitPane { pane, direction } => {
                let wi = self.ws_index_of_pane(pane)?;
                let new_pane = PaneId(self.state.alloc_id());
                let ws = &mut self.state.workspaces[wi];
                let split_ok = ws.layout.split(pane, direction, new_pane);
                debug_assert!(split_ok, "불변식: panes 의 pane 은 layout leaf 로 존재");
                ws.panes.insert(new_pane, empty_pane(new_pane));
                ws.active_pane = new_pane;
                Ok(CommandOutput::PaneCreated { pane: new_pane })
            }

            Command::ClosePane { pane } => {
                let wi = self.ws_index_of_pane(pane)?;
                if self.state.workspaces[wi].panes.len() <= 1 {
                    return Err(CommandError::LastPane);
                }
                let ws = &mut self.state.workspaces[wi];
                let removed = ws
                    .panes
                    .remove(&pane)
                    .expect("ws_index_of_pane 이 존재를 보장");
                let collapse_ok = ws.layout.remove(pane);
                debug_assert!(collapse_ok, "불변식: panes 의 pane 은 layout leaf 로 존재");
                if ws.active_pane == pane {
                    // 닫힌 pane 이 포커스였으면 collapse 후 leaf 순서상 첫 pane 으로.
                    ws.active_pane = ws.layout.leaves()[0];
                }
                for tab in &removed.tabs {
                    self.kill_if_terminal(tab);
                }
                Ok(CommandOutput::Done)
            }

            Command::CreateTab { pane, tab } => {
                let wi = self.ws_index_of_pane(pane)?;
                let NewTab::Terminal { cwd } = tab;
                let ws = &self.state.workspaces[wi];
                // 탭 cwd 미지정 시 워크스페이스 root_path 가 기본 (계획 v2 4장).
                let cwd = cwd.or_else(|| ws.root_path.clone());
                let req = ShellSpawnReq {
                    cwd: cwd.clone(),
                    distro: ws.distro.clone(),
                    ..ShellSpawnReq::default()
                };
                // 원자성: spawn 실패 시 상태(탭·next_id)가 변하지 않도록 spawn 먼저.
                let session =
                    self.host
                        .spawn_shell(req)
                        .map_err(|e| CommandError::SpawnFailed {
                            message: e.to_string(),
                        })?;
                let tab_id = TabId(self.state.alloc_id());
                let pane_ref = self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("ws_index_of_pane 이 존재를 보장");
                pane_ref.tabs.push(Tab {
                    id: tab_id,
                    title: "Terminal".to_owned(),
                    kind: TabKind::Terminal {
                        pty_session: Some(session),
                        status: TerminalStatus::Running,
                        cwd,
                    },
                    notification: NotificationState::None,
                    last_activity_ms: None,
                });
                pane_ref.active_tab = Some(tab_id);
                Ok(CommandOutput::TabCreated {
                    tab: tab_id,
                    session: Some(session),
                })
            }

            Command::ActivateTab { tab } => {
                let (wi, pane, _) = self.locate_tab(tab)?;
                self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("locate_tab 이 존재를 보장")
                    .active_tab = Some(tab);
                Ok(CommandOutput::Done)
            }

            Command::CloseTab { tab } => {
                let (wi, pane, ti) = self.locate_tab(tab)?;
                let pane_ref = self.state.workspaces[wi]
                    .panes
                    .get_mut(&pane)
                    .expect("locate_tab 이 존재를 보장");
                let removed = pane_ref.tabs.remove(ti);
                if pane_ref.active_tab == Some(tab) {
                    // 직전 탭으로 조정. 첫 탭이었으면 (제거 후 index 0 에 온) 다음
                    // 탭, 마지막 남은 탭이었으면 None — 빈 pane 허용 (10단계 임시).
                    pane_ref.active_tab = if pane_ref.tabs.is_empty() {
                        None
                    } else {
                        Some(pane_ref.tabs[ti.saturating_sub(1)].id)
                    };
                }
                self.kill_if_terminal(&removed);
                Ok(CommandOutput::Done)
            }
        }
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

fn empty_pane(id: PaneId) -> Pane {
    Pane {
        id,
        tabs: Vec::new(),
        active_tab: None,
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

    /// 워크스페이스 1개를 만들고 (workspace, 초기 pane) id 를 돌려주는 헬퍼.
    fn create_ws(d: &mut Dispatcher, name: &str) -> (WorkspaceId, PaneId) {
        match d
            .dispatch(Command::CreateWorkspace {
                name: name.into(),
                root_path: None,
                distro: None,
            })
            .unwrap()
        {
            CommandOutput::WorkspaceCreated { workspace, pane } => (workspace, pane),
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

    #[test]
    fn create_workspace_makes_empty_pane_and_activates() {
        let (mut d, _host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
            })
            .unwrap();
        assert_eq!(
            out,
            CommandOutput::WorkspaceCreated {
                workspace: WorkspaceId(1),
                pane: PaneId(2),
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
    fn full_flow_kills_sessions_and_collapses() {
        let (mut d, host) = dispatcher();
        let (ws, pane1) = create_ws(&mut d, "ws");

        // 분할 → 새 빈 pane 이 second 로 생기고 포커스 이동.
        let out = d
            .dispatch(Command::SplitPane {
                pane: pane1,
                direction: SplitDirection::Vertical,
            })
            .unwrap();
        let CommandOutput::PaneCreated { pane: pane2 } = out else {
            panic!("unexpected output: {out:?}");
        };
        {
            let w = d.state().workspace(ws).unwrap();
            assert_eq!(w.active_pane, pane2);
            assert_eq!(w.layout.leaves(), vec![pane1, pane2]);
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
            },
            Command::ClosePane { pane: PaneId(99) },
            Command::CreateTab {
                pane: PaneId(99),
                tab: NewTab::Terminal { cwd: None },
            },
            Command::ActivateTab { tab: TabId(99) },
            Command::CloseTab { tab: TabId(99) },
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
    fn create_tab_uses_workspace_defaults_for_spawn() {
        let (mut d, host) = dispatcher();
        let out = d
            .dispatch(Command::CreateWorkspace {
                name: "ws".into(),
                root_path: Some("/proj".into()),
                distro: Some("Ubuntu".into()),
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

        // 마지막 탭 닫기 → 빈 pane (None) 허용.
        d.dispatch(Command::CloseTab { tab: tab2 }).unwrap();
        assert_eq!(active(&d), None);
        assert!(d.state().workspace(ws).unwrap().panes[&pane]
            .tabs
            .is_empty());
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
        let out = d
            .dispatch(Command::SplitPane {
                pane: pane2,
                direction: SplitDirection::Horizontal,
            })
            .unwrap();
        let CommandOutput::PaneCreated { pane: pane3 } = out else {
            panic!("unexpected output: {out:?}");
        };

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
}
