// 사이드바 워크스페이스 카드 모델 계산 (DOM-free — vitest 대상, 13단계 D3).
//
// Workspace 배열 + activeWorkspace 를 렌더 가능한 카드 모델 배열로 사영한다.
// DOM 조립(sidebar.ts)과 판정 로직을 분리해 상태 매핑·경로 축약·null 생략을
// 순수 테스트로 잠근다 (tab-strip-model 과 같은 구도). agentStatus 는 18단계
// 전까지 코어가 idle 고정값을 주고, gitBranch/gitDirty 는 19단계 전까지 null 이라
// message·branch 는 당분간 생략 렌더가 기본이다 — 매핑 자체는 지금 잠근다.

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
  /** lastAgentMessage 의 첫 줄 — null·공백뿐이면 null (카드에서 생략). */
  message: string | null;
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

/** 닫기 confirm 판정 (계획 D4) — 터미널 탭이 1개라도 있는가.
 *  CloseWorkspace 는 그 세션 전부를 죽이는 파괴적 동작이라 confirm 을 거친다. */
export function hasTerminalTabs(ws: Workspace): boolean {
  return Object.values(ws.panes).some((pane) =>
    pane.tabs.some((tab) => tab.kind.type === "terminal"),
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
