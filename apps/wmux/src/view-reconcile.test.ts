// view-reconcile(planViewSync) 검증 — 부트 스윕(D4-b)·탭 닫힘·워크스페이스 밖
// dispose·keep-alive 유지·세션 없는 active 탭.

import { describe, expect, it } from "vitest";

import { planViewSync } from "./view-reconcile";
import type { Pane, StateSnapshot, Tab, Workspace } from "./types";

function terminalTab(id: number, session: number | null, exited = false): Tab {
  return {
    id,
    title: `tab ${id}`,
    kind: {
      type: "terminal",
      ptySession: session,
      status: exited ? { type: "exited", code: 0 } : { type: "running" },
      cwd: null,
    },
    notification: "none",
    lastActivityMs: null,
  };
}

function pane(id: number, tabs: Tab[], activeTab: number | null): Pane {
  return { id, tabs, activeTab };
}

function workspace(id: number, panes: Pane[], activePane: number): Workspace {
  const record: Record<string, Pane> = {};
  for (const p of panes) record[String(p.id)] = p;
  return {
    id,
    name: `ws ${id}`,
    rootPath: null,
    distro: null,
    gitBranch: null,
    gitDirty: null,
    // planViewSync 는 layout 을 보지 않는다 — 형태만 유효한 leaf 로 채운다.
    layout: { type: "leaf", pane: panes[0]?.id ?? 0 },
    panes: record,
    activePane,
    agentStatus: "idle",
    lastAgentMessage: null,
  };
}

function snapshot(workspaces: Workspace[], activeWorkspace: number | null): StateSnapshot {
  return {
    revision: 1,
    state: { workspaces, activeWorkspace, nextId: 100, revision: 1 },
  };
}

describe("planViewSync", () => {
  it("boot: attaches only visible tabs and sweeps every other session (D4-b)", () => {
    // 활성 ws: pane 1(active 탭 10=세션 100, 배경 탭 11=세션 101),
    //          pane 2(active 탭 12=세션 102). 비활성 ws: 탭 13=세션 103.
    const snap = snapshot(
      [
        workspace(
          1,
          [
            pane(1, [terminalTab(10, 100), terminalTab(11, 101)], 10),
            pane(2, [terminalTab(12, 102)], 12),
          ],
          1,
        ),
        workspace(2, [pane(3, [terminalTab(13, 103)], 13)], 3),
      ],
      1,
    );
    const plan = planViewSync([], snap);
    expect(plan.visible).toEqual([
      { pane: 1, tab: 10, session: 100 },
      { pane: 2, tab: 12, session: 102 },
    ]);
    expect(plan.dispose).toEqual([]);
    // 부트 스윕: 미방문(배경) 탭 101 + 비활성 ws 의 103 전부 detach.
    expect([...plan.detachSessions].sort((a, b) => a - b)).toEqual([101, 103]);
  });

  it("keeps alive background tabs of the active workspace (no dispose, no detach)", () => {
    const snap = snapshot(
      [workspace(1, [pane(1, [terminalTab(10, 100), terminalTab(11, 101)], 10)], 1)],
      1,
    );
    // 탭 11 은 배경이지만 alive(keep-alive) — dispose 도 detach 도 안 된다.
    const plan = planViewSync([10, 11], snap);
    expect(plan.dispose).toEqual([]);
    expect(plan.detachSessions).toEqual([]);
    expect(plan.visible).toEqual([{ pane: 1, tab: 10, session: 100 }]);
  });

  it("disposes an alive tab that vanished from the snapshot (tab closed)", () => {
    const snap = snapshot(
      [workspace(1, [pane(1, [terminalTab(10, 100)], 10)], 1)],
      1,
    );
    // 탭 11 은 닫혀 스냅샷에 없다 — dispose 로 떨어지고, 세션은 스냅샷에 없으므로
    // detach 스윕 대상도 아니다 (dispose 쪽 detach 가 처리).
    const plan = planViewSync([10, 11], snap);
    expect(plan.dispose).toEqual([11]);
    expect(plan.detachSessions).toEqual([]);
  });

  it("disposes alive tabs outside the active workspace without double-detaching", () => {
    const snap = snapshot(
      [
        workspace(1, [pane(1, [terminalTab(10, 100)], 10)], 1),
        workspace(2, [pane(2, [terminalTab(20, 200)], 20)], 2),
      ],
      1,
    );
    // 워크스페이스 전환 직후: ws 2 의 탭 20 이 아직 alive — dispose 대상.
    // 세션 200 은 alive 라 detach 목록에 넣지 않는다 (dispose 가 detach — 멱등 중복 회피).
    const plan = planViewSync([10, 20], snap);
    expect(plan.dispose).toEqual([20]);
    expect(plan.detachSessions).toEqual([]);
  });

  it("skips active tabs without a pty session and still sweeps their pane's others", () => {
    const snap = snapshot(
      [
        workspace(
          1,
          [pane(1, [terminalTab(10, null), terminalTab(11, 101)], 10)],
          1,
        ),
      ],
      1,
    );
    const plan = planViewSync([], snap);
    // active 탭 10 은 세션이 없어 visible 이 아니고, 배경 탭 11 은 스윕된다.
    expect(plan.visible).toEqual([]);
    expect(plan.detachSessions).toEqual([101]);
  });

  it("keeps exited sessions visible (replay display)", () => {
    const snap = snapshot(
      [workspace(1, [pane(1, [terminalTab(10, 100, true)], 10)], 1)],
      1,
    );
    const plan = planViewSync([], snap);
    expect(plan.visible).toEqual([{ pane: 1, tab: 10, session: 100 }]);
    expect(plan.detachSessions).toEqual([]);
  });

  it("disposes everything when there is no active workspace", () => {
    const snap = snapshot([workspace(1, [pane(1, [terminalTab(10, 100)], 10)], 1)], null);
    const plan = planViewSync([10], snap);
    expect(plan.visible).toEqual([]);
    expect(plan.dispose).toEqual([10]);
    expect(plan.detachSessions).toEqual([]);
  });
});
