// tab-strip-model 검증 — active·exited·notification 조합과 탭 순서 보존.

import { describe, expect, it } from "vitest";

import { tabStripModel } from "./tab-strip-model";
import type { NotificationState, Pane, Tab, TerminalStatus } from "./types";

function terminalTab(
  id: number,
  opts: {
    title?: string;
    status?: TerminalStatus;
    notification?: NotificationState;
    session?: number | null;
  } = {},
): Tab {
  return {
    id,
    title: opts.title ?? `tab ${id}`,
    kind: {
      type: "terminal",
      ptySession: opts.session === undefined ? id * 100 : opts.session,
      status: opts.status ?? { type: "running" },
      cwd: null,
    },
    notification: opts.notification ?? "none",
    lastActivityMs: null,
  };
}

function viewerTab(id: number): Tab {
  return {
    id,
    title: `viewer ${id}`,
    kind: { type: "textViewer", path: "C:/tmp/a.txt", scrollTop: 0 },
    notification: "none",
    lastActivityMs: null,
  };
}

function pane(id: number, tabs: Tab[], activeTab: number | null): Pane {
  return { id, tabs, activeTab };
}

describe("tabStripModel", () => {
  it("returns an empty array for an empty pane", () => {
    expect(tabStripModel(pane(1, [], null))).toEqual([]);
  });

  it("marks only the activeTab as active, preserving tab order", () => {
    const p = pane(1, [terminalTab(10), terminalTab(11), terminalTab(12)], 11);
    const models = tabStripModel(p);
    expect(models.map((m) => m.tab)).toEqual([10, 11, 12]);
    expect(models.map((m) => m.active)).toEqual([false, true, false]);
    expect(models[0]?.title).toBe("tab 10");
  });

  it("flags exited only for terminal tabs whose status is exited", () => {
    const p = pane(
      1,
      [
        terminalTab(10, { status: { type: "running" } }),
        terminalTab(11, { status: { type: "exited", code: 0 } }),
        terminalTab(12, { status: { type: "exited", code: null } }),
        viewerTab(13), // terminal 이 아니므로 exited 판정 대상이 아니다
      ],
      10,
    );
    expect(tabStripModel(p).map((m) => m.exited)).toEqual([false, true, true, false]);
  });

  it("maps notification unread to true and none to false", () => {
    const p = pane(
      1,
      [terminalTab(10, { notification: "unread" }), terminalTab(11, { notification: "none" })],
      10,
    );
    expect(tabStripModel(p).map((m) => m.notification)).toEqual([true, false]);
  });

  it("combines flags independently (active+exited+notification on one tab)", () => {
    const p = pane(
      1,
      [terminalTab(10, { status: { type: "exited", code: 1 }, notification: "unread" })],
      10,
    );
    expect(tabStripModel(p)).toEqual([
      { tab: 10, title: "tab 10", active: true, exited: true, notification: true },
    ]);
  });
});
