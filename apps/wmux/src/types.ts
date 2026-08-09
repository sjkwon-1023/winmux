// wmux-core 모델·커맨드의 수기 TS 미러.
// 정본은 crates/wmux-core/src/{model,command}.rs 의 rustdoc, 직렬화 계약은
// fixtures/stage10-*.json — internal tag("type")·camelCase·nullable 필드·panes 맵
// 키가 문자열 숫자인 점을 그대로 미러한다. types.test.ts 가 같은 fixture 를
// 소비해 표류를 잠근다 (10단계 계획 0-7 — Rust round-trip 단독으론 무효).

/** 안정 ID — Rust 쪽 u64 newtype(WorkspaceId/PaneId/TabId/SplitId)은 JSON 에서
 *  그냥 숫자다. AppState 단일 카운터 발급이라 종류 불문 전역 유일하다. */
export type WorkspaceId = number;
export type PaneId = number;
export type TabId = number;
export type SplitId = number;

/** 휘발성 PTY 세션 id (u32) — 안정 ID 와 별개 공간 (model.rs 참조). */
export type SessionId = number;

/** `get_state` 응답·`state-changed` emit 의 공통 형태 (command.rs StateSnapshot). */
export interface StateSnapshot {
  revision: number;
  state: AppState;
}

export interface AppState {
  workspaces: Workspace[];
  /** 마지막 워크스페이스를 닫으면 null. */
  activeWorkspace: WorkspaceId | null;
  nextId: number;
  revision: number;
}

export interface Workspace {
  id: WorkspaceId;
  name: string;
  rootPath: string | null;
  distro: string | null;
  /** git 정보 — 타입 공간만 확정, 값 채움은 19단계. */
  gitBranch: string | null;
  gitDirty: boolean | null;
  layout: SplitTree;
  /** Rust BTreeMap<PaneId, Pane> — JSON object 키 제약으로 키가 문자열 숫자("2")다.
   *  조회는 String(paneId) 로 한다. */
  panes: Record<string, Pane>;
  activePane: PaneId;
  agentStatus: AgentStatus;
  lastAgentMessage: string | null;
}

export type AgentStatus = "running" | "needsInput" | "idle";

export type NotificationState = "none" | "unread";

export type SplitDirection = "horizontal" | "vertical";

/** 워크스페이스 레이아웃 이진 트리 — internal tag "type" (model.rs SplitTree). */
export type SplitTree =
  | { type: "leaf"; pane: PaneId }
  | {
      type: "split";
      /** split 노드의 안정 ID — resizeSplit 의 대상 주소 (계획 D1). */
      id: SplitId;
      direction: SplitDirection;
      /** first 가 차지하는 비율 (0.0~1.0 개구간). */
      ratio: number;
      first: SplitTree;
      second: SplitTree;
    };

/** pane(= TabContainer). 탭 0개인 빈 pane 허용 — 10단계 임시 상태. */
export interface Pane {
  id: PaneId;
  tabs: Tab[];
  activeTab: TabId | null;
}

export interface Tab {
  id: TabId;
  title: string;
  kind: TabKind;
  notification: NotificationState;
  lastActivityMs: number | null;
}

/** 탭 종류별 상태 — internal tag "type". 뷰어 3종은 타입 공간만 확정 (21단계). */
export type TabKind =
  | {
      type: "terminal";
      /** 휘발성 PTY 세션 id. 세션 종료 후에도 유지된다 (Exited 탭 표시용). */
      ptySession: SessionId | null;
      status: TerminalStatus;
      cwd: string | null;
    }
  | { type: "folderBrowser"; path: string }
  | { type: "textViewer"; path: string; scrollTop: number }
  | { type: "markdownViewer"; path: string; scrollTop: number };

export type TerminalStatus =
  | { type: "running" }
  | { type: "exited"; code: number | null };

/** CreateTab 의 탭 명세 (command.rs NewTab) — 뷰어 variant 는 21단계. */
export type NewTab = { type: "terminal"; cwd: string | null };

/** 직렬화 가능한 command bus 명령 집합 (command.rs Command).
 *  JSON 은 internal tag: {"type": "createWorkspace", "name": ...}. */
export type Command =
  /** tab 이 non-null 이면 초기 pane 에 그 탭까지 원자 생성 (계획 13-D1).
   *  필드 누락(undefined)은 null 과 동일 — 13단계 이전 클라이언트 하위호환. */
  | {
      type: "createWorkspace";
      name: string;
      rootPath: string | null;
      distro: string | null;
      tab?: NewTab | null;
    }
  | { type: "switchWorkspace"; workspace: WorkspaceId }
  | { type: "closeWorkspace"; workspace: WorkspaceId }
  | { type: "focusPane"; pane: PaneId }
  /** tab 이 non-null 이면 새 pane 에 그 탭까지 원자 생성 (계획 D5). */
  | { type: "splitPane"; pane: PaneId; direction: SplitDirection; tab: NewTab | null }
  /** ratio 는 finite·개구간 (0, 1) — 아니면 invalidRatio 에러 (계획 D2). */
  | { type: "resizeSplit"; split: SplitId; ratio: number }
  | { type: "closePane"; pane: PaneId }
  | { type: "createTab"; pane: PaneId; tab: NewTab }
  | { type: "activateTab"; tab: TabId }
  | { type: "closeTab"; tab: TabId };

/** dispatch 성공 결과 (command.rs CommandOutput). */
export type CommandOutput =
  /** createWorkspace 결과 — 생성된 안정 ID 전부. tab/session 은
   *  createWorkspace.tab 이 non-null 이었을 때만 non-null (계획 13-D1). */
  | {
      type: "workspaceCreated";
      workspace: WorkspaceId;
      pane: PaneId;
      tab: TabId | null;
      session: SessionId | null;
    }
  /** splitPane 결과 — 생성된 안정 ID 전부. tab/session 은 splitPane.tab 이
   *  non-null 이었을 때만 non-null (계획 D5). */
  | {
      type: "paneCreated";
      pane: PaneId;
      split: SplitId;
      tab: TabId | null;
      session: SessionId | null;
    }
  | { type: "tabCreated"; tab: TabId; session: SessionId | null }
  | { type: "done" };

/** dispatch 실패 (command.rs CommandError) — invoke reject payload 로 도착한다. */
export type CommandError =
  | { type: "unknownTarget"; target: string }
  | { type: "lastPane" }
  | { type: "spawnFailed"; message: string }
  | { type: "invalidRatio"; ratio: number };
