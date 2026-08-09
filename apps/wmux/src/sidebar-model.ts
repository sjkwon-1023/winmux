// 사이드바 워크스페이스 카드 모델 계산 (DOM-free — vitest 대상, 13단계 D3).
//
// Workspace 배열 + activeWorkspace 를 렌더 가능한 카드 모델 배열로 사영한다.
// DOM 조립(sidebar.ts)과 판정 로직을 분리해 상태 매핑·경로 축약·null 생략을
// 순수 테스트로 잠근다 (tab-strip-model 과 같은 구도). 18단계부터 agentStatus·
// message·unread 는 OSC 라우팅으로 실제 값이 들어오는 동적 필드다 — gitBranch/
// gitDirty 만 19단계 전까지 null 이라 branch 는 생략 렌더가 기본이다.
//
// 여기에 렌더 판정(reconcilePlan)까지 두는 이유: DOM 재조립 여부는 카드 모델의
// id 멤버십·필드 동일성만으로 결정되는 순수 판정이라, DOM 없는 vitest 로 잠글 수
// 있어야 한다 (view-reconcile 이 뷰 수명에 대해 하는 일과 같은 구도).

import type { AgentStatus, Workspace, WorkspaceId } from "./types";

/** 워크스페이스 카드 1개의 렌더 모델. */
export interface WorkspaceCardModel {
  workspace: WorkspaceId;
  name: string;
  /** activeWorkspace 인지 — 하이라이트 + 클릭 no-op 판정에 쓰인다. */
  active: boolean;
  status: AgentStatus;
  /** 상태 아이콘 — running ⚡ / needsInput 🔔 / idle · (계획 v2 6장). */
  statusIcon: string;
  /** lastAgentMessage 의 첫 줄 미리보기 — null·공백뿐이면 null (카드에서 생략). */
  message: string | null;
  /** 워크스페이스 집계 unread dot — 어느 pane 의 어느 탭이든 미확인 알림이 있으면
   *  true. agentStatus 와 별개인 이유: 토큰 불일치 777·OSC 9 같은 **상태 중립**
   *  알림은 agent_status 를 바꾸지 않으므로(18단계 규약), 집계 dot 이 없으면
   *  백그라운드 워크스페이스에서 그 알림이 어디에도 안 보인다 (3층 중 워크스페이스 층). */
  unread: boolean;
  /** "main*" 형식 (dirty 면 * 접미) — gitBranch null 이면 null. */
  branch: string | null;
  /** rootPath 축약 (~/ 치환 + 뒤 2세그먼트 유지) — null 이면 null. */
  path: string | null;
  counts: { panes: number; tabs: number };
}

const STATUS_ICONS: Record<AgentStatus, string> = {
  running: "⚡",
  needsInput: "🔔",
  idle: "·",
};

/** rootPath 축약 — `/home/<user>` 접두를 `~` 로 치환하고, 나머지 세그먼트가
 *  2개를 넘으면 중간을 `…` 로 접어 뒤 2세그먼트만 남긴다.
 *  예: `/home/u/a/b/c` → `~/…/b/c`, `/home/u/code` → `~/code`,
 *  `/srv/a/b/c` → `…/b/c`, `/srv/data` → 원문 유지. */
export function abbreviatePath(rootPath: string | null): string | null {
  if (rootPath === null) return null;
  const home = rootPath.replace(/^\/home\/[^/]+(?=\/|$)/, "~");
  const isHome = home.startsWith("~");
  const rest = isHome ? home.slice(1) : home;
  // 빈 세그먼트 제거 — 후행 슬래시·중복 슬래시를 흡수한다.
  const segs = rest.split("/").filter((s) => s.length > 0);
  if (isHome) {
    if (segs.length === 0) return "~";
    if (segs.length <= 2) return `~/${segs.join("/")}`;
    return `~/…/${segs.slice(-2).join("/")}`;
  }
  if (segs.length <= 2) return rootPath;
  return `…/${segs.slice(-2).join("/")}`;
}

/** lastAgentMessage 1줄 절단 — 첫 줄만 남긴다 (시각적 말줄임은 CSS 몫).
 *  null 이거나 첫 줄이 공백뿐이면 null (카드에서 행 자체를 생략). */
function firstLine(message: string | null): string | null {
  if (message === null) return null;
  const line = (message.split(/\r?\n/, 1)[0] ?? "").trim();
  return line.length === 0 ? null : line;
}

/** 닫기 confirm 판정 (계획 D4) — **실행 중인** 터미널 세션이 1개라도 있는가.
 *  CloseWorkspace 는 그 세션들을 죽이는 파괴적 동작이라 confirm 을 거친다.
 *  exited 만 남은 워크스페이스는 죽일 세션이 없으므로 confirm 없이 닫는다
 *  (리뷰 finding — "sessions will be killed" 경고가 거짓이 되는 것 방지). */
export function hasRunningTerminals(ws: Workspace): boolean {
  return Object.values(ws.panes).some((pane) =>
    pane.tabs.some(
      (tab) => tab.kind.type === "terminal" && tab.kind.status.type === "running",
    ),
  );
}

/** Workspace 배열 → 카드 모델 배열 (순서 유지). */
export function sidebarModel(
  workspaces: Workspace[],
  activeWorkspace: WorkspaceId | null,
): WorkspaceCardModel[] {
  return workspaces.map((ws) => {
    const panes = Object.values(ws.panes);
    return {
      workspace: ws.id,
      name: ws.name,
      active: ws.id === activeWorkspace,
      status: ws.agentStatus,
      statusIcon: STATUS_ICONS[ws.agentStatus],
      message: firstLine(ws.lastAgentMessage),
      unread: panes.some((pane) => pane.tabs.some((tab) => tab.notification === "unread")),
      // gitDirty 는 boolean|null — null(값 미도입, 19단계 전)은 clean 취급.
      branch: ws.gitBranch === null ? null : `${ws.gitBranch}${ws.gitDirty === true ? "*" : ""}`,
      path: abbreviatePath(ws.rootPath),
      counts: {
        panes: panes.length,
        tabs: panes.reduce((sum, pane) => sum + pane.tabs.length, 0),
      },
    };
  });
}

/** 카드 리스트 렌더 판정 (18단계 B-6).
 *  - `skip`: 모델이 완전히 동일 — DOM 을 건드리지 않는다.
 *  - `patch`: 카드 id 배열의 멤버십·순서가 같고 필드만 변함 — 기존 카드 노드를
 *    유지한 채 텍스트·클래스만 갱신한다.
 *  - `rebuild`: 카드가 추가·삭제·재정렬됨(또는 첫 렌더) — 리스트를 재조립한다. */
export type CardReconcile = "skip" | "patch" | "rebuild";

/** 카드 모델 1개의 전 필드 동일성 — 이 카드의 DOM 을 건드릴지 판정한다. */
export function sameCard(a: WorkspaceCardModel, b: WorkspaceCardModel): boolean {
  return (
    a.workspace === b.workspace &&
    a.name === b.name &&
    a.active === b.active &&
    a.status === b.status &&
    a.statusIcon === b.statusIcon &&
    a.message === b.message &&
    a.unread === b.unread &&
    a.branch === b.branch &&
    a.path === b.path &&
    a.counts.panes === b.counts.panes &&
    a.counts.tabs === b.counts.tabs
  );
}

/** 직전 렌더 모델(첫 렌더면 null) × 이번 모델 → 렌더 판정. 순수 함수 —
 *  실행(재조립·패치)은 sidebar.ts 몫이다.
 *
 *  판정을 id 멤버십·순서와 필드 동일성으로 쪼개는 것이 이 단계의 핵심이다:
 *  18단계부터 status·message·unread 가 매 OSC 마다 변하는 동적 필드가 되어
 *  "모델 전체 직렬화 키가 같을 때만 스킵"하던 기존 가드가 상시로 뚫린다.
 *  그때 리스트를 통째로 재조립하면 눌린 카드 엘리먼트가 mousedown~click 사이에
 *  갈아치워져 클릭이 유실된다 (ADR-0003 결정 7 의 탭바 스왈로와 같은 결함). */
export function reconcilePlan(
  prev: WorkspaceCardModel[] | null,
  next: WorkspaceCardModel[],
): CardReconcile {
  if (prev === null || prev.length !== next.length) return "rebuild";
  for (let i = 0; i < next.length; i += 1) {
    const before = prev[i];
    const after = next[i];
    if (before === undefined || after === undefined) return "rebuild";
    if (before.workspace !== after.workspace) return "rebuild";
  }
  const changed = next.some((after, i) => {
    const before = prev[i];
    return before === undefined || !sameCard(before, after);
  });
  return changed ? "patch" : "skip";
}
