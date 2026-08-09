// sidebar-model 검증 — 상태 매핑·경로 축약 경계·null 생략·counts 집계·집계 unread,
// 그리고 렌더 판정(reconcilePlan)의 skip/patch/rebuild 3분기.

import { describe, expect, it } from "vitest";

import {
  abbreviatePath,
  hasRunningTerminals,
  reconcilePlan,
  sidebarModel,
} from "./sidebar-model";
import type { AgentStatus, NotificationState, Pane, Tab, Workspace } from "./types";

function terminalTab(id: number, notification: NotificationState = "none"): Tab {
  return {
    id,
    title: `tab ${id}`,
    kind: { type: "terminal", ptySession: id * 100, status: { type: "running" }, cwd: null },
    notification,
    lastActivityMs: null,
  };
}

function viewerTab(id: number): Tab {
  return {
    id,
    title: `viewer ${id}`,
    kind: { type: "textViewer", path: "/tmp/a.txt", scrollTop: 0 },
    notification: "none",
    lastActivityMs: null,
  };
}

function pane(id: number, tabs: Tab[]): Pane {
  return { id, tabs, activeTab: tabs[0]?.id ?? null };
}

function ws(
  id: number,
  opts: {
    name?: string;
    rootPath?: string | null;
    gitBranch?: string | null;
    gitDirty?: boolean | null;
    agentStatus?: AgentStatus;
    lastAgentMessage?: string | null;
    panes?: Record<string, Pane>;
  } = {},
): Workspace {
  return {
    id,
    name: opts.name ?? `ws ${id}`,
    rootPath: opts.rootPath ?? null,
    distro: null,
    gitBranch: opts.gitBranch ?? null,
    gitDirty: opts.gitDirty ?? null,
    layout: { type: "leaf", pane: 1 },
    panes: opts.panes ?? { "1": pane(1, [terminalTab(10)]) },
    activePane: 1,
    agentStatus: opts.agentStatus ?? "idle",
    lastAgentMessage: opts.lastAgentMessage ?? null,
  };
}

describe("sidebarModel", () => {
  it("maps agentStatus to status and icon", () => {
    const models = sidebarModel(
      [
        ws(1, { agentStatus: "running" }),
        ws(2, { agentStatus: "needsInput" }),
        ws(3, { agentStatus: "idle" }),
      ],
      1,
    );
    expect(models.map((m) => m.status)).toEqual(["running", "needsInput", "idle"]);
    expect(models.map((m) => m.statusIcon)).toEqual(["⚡", "🔔", "·"]);
  });

  it("marks only the activeWorkspace as active, preserving order", () => {
    const models = sidebarModel([ws(1), ws(2), ws(3)], 2);
    expect(models.map((m) => m.workspace)).toEqual([1, 2, 3]);
    expect(models.map((m) => m.active)).toEqual([false, true, false]);
  });

  it("marks nothing active when activeWorkspace is null (empty-state precursor)", () => {
    expect(sidebarModel([ws(1)], null).map((m) => m.active)).toEqual([false]);
  });

  it("omits message/branch/path as null when the model values are null", () => {
    const m = sidebarModel([ws(1)], 1)[0];
    expect(m?.message).toBeNull();
    expect(m?.branch).toBeNull();
    expect(m?.path).toBeNull();
  });

  it("cuts lastAgentMessage to its first line and nulls blank messages", () => {
    const models = sidebarModel(
      [
        ws(1, { lastAgentMessage: "done: 3 files changed\nsecond line\nthird" }),
        ws(2, { lastAgentMessage: "single line" }),
        ws(3, { lastAgentMessage: "  \n다음 줄" }), // 첫 줄이 공백뿐 → 생략
        ws(4, { lastAgentMessage: "" }),
      ],
      1,
    );
    expect(models.map((m) => m.message)).toEqual([
      "done: 3 files changed",
      "single line",
      null,
      null,
    ]);
  });

  it("formats branch with a dirty asterisk; null gitDirty counts as clean", () => {
    const models = sidebarModel(
      [
        ws(1, { gitBranch: "main", gitDirty: true }),
        ws(2, { gitBranch: "main", gitDirty: false }),
        ws(3, { gitBranch: "feat/x", gitDirty: null }),
      ],
      1,
    );
    expect(models.map((m) => m.branch)).toEqual(["main*", "main", "feat/x"]);
  });

  it("aggregates unread across every pane and tab of the workspace", () => {
    const models = sidebarModel(
      [
        // 백그라운드 pane 의 탭 하나만 unread — 상태 중립 알림의 표면화 경로.
        ws(1, {
          panes: {
            "1": pane(1, [terminalTab(10), terminalTab(11)]),
            "2": pane(2, [terminalTab(12, "unread")]),
          },
        }),
        ws(2, { panes: { "1": pane(1, [terminalTab(20), terminalTab(21)]) } }),
        ws(3, { panes: { "1": pane(1, []) } }),
      ],
      1,
    );
    expect(models.map((m) => m.unread)).toEqual([true, false, false]);
  });

  it("keeps unread independent of agentStatus (idle workspace can still have a dot)", () => {
    const m = sidebarModel(
      [
        ws(1, {
          agentStatus: "idle",
          panes: { "1": pane(1, [terminalTab(10, "unread")]) },
        }),
      ],
      1,
    )[0];
    expect(m?.status).toBe("idle");
    expect(m?.unread).toBe(true);
  });

  it("counts panes and sums tabs across panes", () => {
    const m = sidebarModel(
      [
        ws(1, {
          panes: {
            "1": pane(1, [terminalTab(10), terminalTab(11)]),
            "2": pane(2, [terminalTab(12)]),
            "3": pane(3, []),
          },
        }),
      ],
      1,
    )[0];
    expect(m?.counts).toEqual({ panes: 3, tabs: 3 });
  });
});

describe("abbreviatePath", () => {
  it("replaces the /home/<user> prefix with ~", () => {
    expect(abbreviatePath("/home/kwon1")).toBe("~");
    expect(abbreviatePath("/home/kwon1/code")).toBe("~/code");
    expect(abbreviatePath("/home/kwon1/code/wmux")).toBe("~/code/wmux");
  });

  it("collapses the middle keeping the last 2 segments", () => {
    expect(abbreviatePath("/home/kwon1/a/b/c")).toBe("~/…/b/c");
    expect(abbreviatePath("/home/kwon1/aa-project/wmux/main")).toBe("~/…/wmux/main");
    expect(abbreviatePath("/srv/data/proj/x")).toBe("…/proj/x");
  });

  it("keeps short non-home paths verbatim", () => {
    expect(abbreviatePath("/srv/data")).toBe("/srv/data");
    expect(abbreviatePath("/srv")).toBe("/srv");
  });

  it("does not treat a /home-prefixed name as the home dir itself", () => {
    // /home 바로 아래가 아닌 /homeX, /home 자체는 ~ 치환 대상이 아니다.
    expect(abbreviatePath("/home")).toBe("/home");
    expect(abbreviatePath("/homelab/x")).toBe("/homelab/x");
  });

  it("absorbs trailing slashes into segments", () => {
    expect(abbreviatePath("/home/kwon1/code/")).toBe("~/code");
    expect(abbreviatePath("/home/kwon1/a/b/c/")).toBe("~/…/b/c");
  });

  it("passes null through", () => {
    expect(abbreviatePath(null)).toBeNull();
  });
});

describe("hasRunningTerminals", () => {
  it("is true when any pane has a running terminal tab", () => {
    const w = ws(1, {
      panes: { "1": pane(1, [viewerTab(10)]), "2": pane(2, [terminalTab(11)]) },
    });
    expect(hasRunningTerminals(w)).toBe(true);
  });

  it("is false for viewer-only, empty, or exited-only panes", () => {
    expect(hasRunningTerminals(ws(1, { panes: { "1": pane(1, [viewerTab(10)]) } }))).toBe(false);
    expect(hasRunningTerminals(ws(2, { panes: { "1": pane(1, []) } }))).toBe(false);
    const exited = terminalTab(12);
    if (exited.kind.type === "terminal") exited.kind.status = { type: "exited", code: 0 };
    expect(hasRunningTerminals(ws(3, { panes: { "1": pane(1, [exited]) } }))).toBe(false);
  });
});

describe("reconcilePlan", () => {
  const three = () => sidebarModel([ws(1), ws(2), ws(3)], 1);

  it("rebuilds on the first render (no previous model)", () => {
    expect(reconcilePlan(null, three())).toBe("rebuild");
  });

  it("skips when the model is unchanged", () => {
    expect(reconcilePlan(three(), three())).toBe("skip");
  });

  it("patches when only dynamic fields change (status, message, unread)", () => {
    const next = sidebarModel(
      [
        ws(1, { agentStatus: "needsInput", lastAgentMessage: "continue?" }),
        ws(2, { panes: { "1": pane(1, [terminalTab(10, "unread")]) } }),
        ws(3),
      ],
      1,
    );
    expect(reconcilePlan(three(), next)).toBe("patch");
  });

  it("patches when the active workspace moves between existing cards", () => {
    expect(reconcilePlan(three(), sidebarModel([ws(1), ws(2), ws(3)], 2))).toBe("patch");
  });

  it("rebuilds when a card is added or removed", () => {
    expect(reconcilePlan(three(), sidebarModel([ws(1), ws(2), ws(3), ws(4)], 1))).toBe("rebuild");
    expect(reconcilePlan(three(), sidebarModel([ws(1), ws(3)], 1))).toBe("rebuild");
    expect(reconcilePlan(three(), [])).toBe("rebuild");
  });

  it("rebuilds when the cards are reordered (same membership)", () => {
    expect(reconcilePlan(three(), sidebarModel([ws(2), ws(1), ws(3)], 1))).toBe("rebuild");
  });
});
