//! 애플리케이션 순수 상태 모델 (계획 v2 4장, 10단계 계획 3-B).
//!
//! UI·PTY·Tauri 에 의존하지 않는 상태 트리. 계층은
//! `AppState → Workspace → SplitTree → Pane → Tab` 이며, 워크스페이스 레벨 탭은
//! 없다 — 탭 층은 pane 내부 하나뿐이다 (계획 v2 4장).
//!
//! # 안정 ID
//!
//! [`WorkspaceId`]/[`PaneId`]/[`TabId`] 는 `AppState::next_id` **단일 u64 카운터**에서
//! 발급되는 세션 내 안정 ID 다 (persistence 15단계·MCP v2 의 참조 대상). PTY 의
//! 휘발성 [`SessionId`](crate::session::SessionId)(u32 alias)와는 newtype 으로
//! 타입 수준에서 구분된다.
//!
//! # 직렬화 계약
//!
//! 전 타입 `#[serde(rename_all = "camelCase")]`. 이 JSON 형태가 프론트 스냅샷과
//! 15단계 persistence 의 계약이며, golden fixture(`fixtures/stage10-*.json`)를
//! cargo test 와 프론트 vitest 가 공유 소비해 표류를 막는다. `panes` 맵의 키는
//! JSON 에서 문자열 숫자(`"2"`)로 직렬화된다 (JSON object 키 제약).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::session::SessionId;

/// 워크스페이스의 안정 ID. `AppState::alloc_id` 단일 카운터에서 발급.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);

/// pane(= TabContainer)의 안정 ID. 외부(MCP v2)에서 대상 지정에 쓰인다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(pub u64);

/// 탭의 안정 ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// 앱 전체 상태의 루트. 소유자는 Rust 쪽 dispatcher — 프론트는 스냅샷 뷰만 받는다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub workspaces: Vec<Workspace>,
    /// 마지막 워크스페이스를 닫으면 None 이 될 수 있다.
    pub active_workspace: Option<WorkspaceId>,
    /// 다음에 발급될 안정 ID. Workspace/Pane/Tab 전 종류가 공유한다 (1부터).
    pub next_id: u64,
    /// 성공한 상태 변이마다 +1. 프론트 스냅샷의 stale 판정 가드.
    pub revision: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            workspaces: Vec::new(),
            active_workspace: None,
            next_id: 1,
            revision: 0,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 안정 ID 발급 — 전 종류(Workspace/Pane/Tab)가 이 단일 카운터를 공유하므로
    /// id 값만으로도 전역 유일하다.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|ws| ws.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|ws| ws.id == id)
    }
}

/// 워크스페이스 — 레이아웃(SplitTree) 1개와 pane 들을 소유한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// 워크스페이스의 정체성이자 새 탭의 기본 cwd (계획 v2 4장).
    pub root_path: Option<String>,
    /// 이 워크스페이스의 터미널이 연결될 WSL 배포판 (계획 v2 5장).
    pub distro: Option<String>,
    /// git 정보 — 타입 공간만 확정, 값 채움은 19단계 (10단계 계획 1장).
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub layout: SplitTree,
    pub panes: BTreeMap<PaneId, Pane>,
    pub active_pane: PaneId,
    /// 18단계가 갱신 — 지금은 Idle 고정.
    pub agent_status: AgentStatus,
    /// 사이드바 미리보기용 OSC 777 body (계획 v2 9장) — 갱신은 18단계.
    pub last_agent_message: Option<String>,
}

impl Workspace {
    /// 상태 불변식 검사 (debug 빌드 전용).
    ///
    /// - layout 의 leaf 집합 == `panes` 키 집합 (중복 leaf 도 불허)
    /// - `active_pane` 은 `panes` 에 존재
    /// - 각 pane 의 `active_tab` 은 그 pane 의 `tabs` 에 존재
    pub fn debug_assert_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            let leaf_list = self.layout.leaves();
            let leaf_set: std::collections::BTreeSet<PaneId> = leaf_list.iter().copied().collect();
            debug_assert_eq!(
                leaf_list.len(),
                leaf_set.len(),
                "workspace {:?}: layout 에 중복 leaf",
                self.id
            );
            let key_set: std::collections::BTreeSet<PaneId> = self.panes.keys().copied().collect();
            debug_assert_eq!(
                leaf_set, key_set,
                "workspace {:?}: layout leaf 집합 != panes 키 집합",
                self.id
            );
            debug_assert!(
                self.panes.contains_key(&self.active_pane),
                "workspace {:?}: active_pane {:?} 이 panes 에 없음",
                self.id,
                self.active_pane
            );
            for pane in self.panes.values() {
                if let Some(tab) = pane.active_tab {
                    debug_assert!(
                        pane.tabs.iter().any(|t| t.id == tab),
                        "pane {:?}: active_tab {:?} 이 tabs 에 없음",
                        pane.id,
                        tab
                    );
                }
            }
        }
    }
}

/// 에이전트 상태 3값 enum — 이진 unread 가 아니다 (계획 v2 9장).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Running,
    NeedsInput,
    Idle,
}

/// 탭 알림 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationState {
    None,
    Unread,
}

/// 분할 방향.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// 워크스페이스 레이아웃 이진 트리. leaf 가 pane 하나에 대응한다.
///
/// JSON 은 다른 계약(TabKind·Command 등)과 동일한 internal tag 를 쓴다:
/// `{"type": "leaf", "pane": 2}` | `{"type": "split", ...}` — TS 쪽이 `type`
/// discriminant 하나로 일관되게 narrowing 하고, persistence(15단계)도 같은
/// 규약을 따른다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SplitTree {
    #[serde(rename_all = "camelCase")]
    Leaf { pane: PaneId },
    #[serde(rename_all = "camelCase")]
    Split {
        direction: SplitDirection,
        /// first 가 차지하는 비율 (0.0~1.0). 기본 0.5, resize 는 12단계.
        /// f64 인 이유: JSON/TS 의 수 표현이 f64 라, f32 는 0.3 같은 값의
        /// 왕복에서 미세 변형을 일으켜 fixture 동등성·persistence 안정성을 깬다.
        ratio: f64,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

impl SplitTree {
    /// `target` leaf 를 `Split` 로 치환한다. 기존 pane 이 `first`(좌/상),
    /// 새 pane 이 `second`(우/하)로 들어가며 ratio 는 0.5.
    /// `target` 이 tree 에 없으면 false 를 반환하고 변경 없음.
    pub fn split(&mut self, target: PaneId, direction: SplitDirection, new_pane: PaneId) -> bool {
        match self {
            SplitTree::Leaf { pane } if *pane == target => {
                *self = SplitTree::Split {
                    direction,
                    ratio: 0.5,
                    first: Box::new(SplitTree::Leaf { pane: target }),
                    second: Box::new(SplitTree::Leaf { pane: new_pane }),
                };
                true
            }
            SplitTree::Leaf { .. } => false,
            SplitTree::Split { first, second, .. } => {
                first.split(target, direction, new_pane)
                    || second.split(target, direction, new_pane)
            }
        }
    }

    /// `target` leaf 를 제거하고 형제 subtree 를 부모 자리로 승격(collapse)한다.
    /// 루트가 단일 leaf 인 경우(워크스페이스의 마지막 pane)는 제거할 수 없어
    /// false — 호출자(ClosePane)가 `LastPane` 에러로 사전에 막는다.
    pub fn remove(&mut self, target: PaneId) -> bool {
        let SplitTree::Split { first, second, .. } = self else {
            return false;
        };
        if matches!(**first, SplitTree::Leaf { pane } if pane == target) {
            // second 를 루트 자리로 승격. mem::replace 의 대체값은 곧바로
            // *self 에 덮여 버려지는 dummy 다.
            let promoted = std::mem::replace(&mut **second, SplitTree::Leaf { pane: target });
            *self = promoted;
            return true;
        }
        if matches!(**second, SplitTree::Leaf { pane } if pane == target) {
            let promoted = std::mem::replace(&mut **first, SplitTree::Leaf { pane: target });
            *self = promoted;
            return true;
        }
        first.remove(target) || second.remove(target)
    }

    /// leaf 들을 좌→우(in-order) 순으로 나열한다.
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            SplitTree::Leaf { pane } => out.push(*pane),
            SplitTree::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// `pane` 이 leaf 로 존재하는지.
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            SplitTree::Leaf { pane: p } => *p == pane,
            SplitTree::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }
}

/// pane(= TabContainer). 탭이 0개인 빈 pane 도 허용된다 — 10단계 임시 상태이며
/// 12단계 pane 정리 규칙에서 재결정한다 (10단계 계획 1장).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
}

/// 탭 — 종류는 [`TabKind`] 로 구분한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub kind: TabKind,
    pub notification: NotificationState,
    /// 마지막 활동 시각 (epoch ms) — 갱신은 18단계.
    pub last_activity_ms: Option<u64>,
}

/// 탭 종류별 상태. 뷰어 3종은 타입 공간만 확정 — 생성 경로는 21단계
/// (10단계 계획 1장 "타입 공간은 지금 확정" 기준).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TabKind {
    Terminal {
        /// 휘발성 PTY 세션 id. 세션 종료 후에도 유지된다 (Exited 탭 표시용).
        pty_session: Option<SessionId>,
        status: TerminalStatus,
        cwd: Option<String>,
    },
    FolderBrowser {
        path: String,
    },
    TextViewer {
        path: String,
        scroll_top: f64,
    },
    MarkdownViewer {
        path: String,
        scroll_top: f64,
    },
}

/// 터미널 탭의 프로세스 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TerminalStatus {
    Running,
    Exited { code: Option<u32> },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> SplitTree {
        SplitTree::Leaf { pane: PaneId(id) }
    }

    fn split(direction: SplitDirection, first: SplitTree, second: SplitTree) -> SplitTree {
        SplitTree::Split {
            direction,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn split_replaces_leaf_with_split() {
        let mut tree = leaf(1);
        assert!(tree.split(PaneId(1), SplitDirection::Horizontal, PaneId(2)));
        assert_eq!(tree, split(SplitDirection::Horizontal, leaf(1), leaf(2)));
    }

    #[test]
    fn split_nested_targets_inner_leaf() {
        let mut tree = split(SplitDirection::Horizontal, leaf(1), leaf(2));
        assert!(tree.split(PaneId(2), SplitDirection::Vertical, PaneId(3)));
        assert_eq!(
            tree,
            split(
                SplitDirection::Horizontal,
                leaf(1),
                split(SplitDirection::Vertical, leaf(2), leaf(3)),
            )
        );
    }

    #[test]
    fn split_missing_target_is_noop() {
        let mut tree = split(SplitDirection::Horizontal, leaf(1), leaf(2));
        let before = tree.clone();
        assert!(!tree.split(PaneId(9), SplitDirection::Vertical, PaneId(3)));
        assert_eq!(tree, before);
    }

    #[test]
    fn remove_promotes_sibling_subtree() {
        // (1 | (2 / 3)) 에서 2 제거 → (1 | 3), 다시 3 제거 → 1 단일 leaf.
        let mut tree = split(
            SplitDirection::Horizontal,
            leaf(1),
            split(SplitDirection::Vertical, leaf(2), leaf(3)),
        );
        assert!(tree.remove(PaneId(2)));
        assert_eq!(tree, split(SplitDirection::Horizontal, leaf(1), leaf(3)));
        assert!(tree.remove(PaneId(3)));
        assert_eq!(tree, leaf(1));
    }

    #[test]
    fn remove_first_child_promotes_second() {
        // ((1 / 2) | 3) 에서 3 제거 → (1 / 2) subtree 가 루트로 승격.
        let inner = split(SplitDirection::Vertical, leaf(1), leaf(2));
        let mut tree = split(SplitDirection::Horizontal, inner.clone(), leaf(3));
        assert!(tree.remove(PaneId(3)));
        assert_eq!(tree, inner);
    }

    #[test]
    fn remove_root_leaf_is_rejected() {
        let mut tree = leaf(1);
        assert!(!tree.remove(PaneId(1)));
        assert_eq!(tree, leaf(1));
    }

    #[test]
    fn remove_missing_target_is_noop() {
        let mut tree = split(SplitDirection::Horizontal, leaf(1), leaf(2));
        let before = tree.clone();
        assert!(!tree.remove(PaneId(9)));
        assert_eq!(tree, before);
    }

    #[test]
    fn leaves_are_in_left_to_right_order() {
        let tree = split(
            SplitDirection::Horizontal,
            split(SplitDirection::Vertical, leaf(1), leaf(2)),
            leaf(3),
        );
        assert_eq!(tree.leaves(), vec![PaneId(1), PaneId(2), PaneId(3)]);
    }

    #[test]
    fn contains_checks_leaves_only() {
        let tree = split(SplitDirection::Horizontal, leaf(1), leaf(2));
        assert!(tree.contains(PaneId(1)));
        assert!(tree.contains(PaneId(2)));
        assert!(!tree.contains(PaneId(3)));
    }

    #[test]
    fn workspace_invariants_hold_for_consistent_state() {
        let pane = |id: u64| Pane {
            id: PaneId(id),
            tabs: Vec::new(),
            active_tab: None,
        };
        let ws = Workspace {
            id: WorkspaceId(1),
            name: "test".into(),
            root_path: None,
            distro: None,
            git_branch: None,
            git_dirty: None,
            layout: split(SplitDirection::Horizontal, leaf(2), leaf(3)),
            panes: [(PaneId(2), pane(2)), (PaneId(3), pane(3))].into(),
            active_pane: PaneId(2),
            agent_status: AgentStatus::Idle,
            last_agent_message: None,
        };
        ws.debug_assert_invariants();
    }
}
