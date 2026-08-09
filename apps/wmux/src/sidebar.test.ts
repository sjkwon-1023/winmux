// @vitest-environment happy-dom
//
// 사이드바 DOM identity 검증 (18단계 B-6) — 순수 판정(reconcilePlan) 테스트가 못
// 잡는 부분을 잠근다: 판정이 patch 여도 렌더가 실제로 노드를 갈아치우면 클릭이
// mousedown~click 사이에 유실된다 (ADR-0003 결정 7 의 스왈로). 그래서 "같은 노드
// 객체(===)에 텍스트만 갱신됐는가"를 실제 DOM 으로 단언한다.
//
// happy-dom 은 이 파일 전용 환경이다 (상단 @vitest-environment) — 나머지 프론트
// 테스트는 계속 DOM 없는 node 환경에서 돈다.

import { describe, expect, it } from "vitest";

import { Sidebar } from "./sidebar";
import type {
  AgentStatus,
  Command,
  NotificationState,
  StateSnapshot,
  Tab,
  Workspace,
  WorkspaceId,
} from "./types";

function terminalTab(id: number, notification: NotificationState): Tab {
  return {
    id,
    title: `tab ${id}`,
    kind: { type: "terminal", ptySession: id * 100, status: { type: "running" }, cwd: null },
    notification,
    lastActivityMs: null,
  };
}

function ws(
  id: number,
  opts: {
    name?: string;
    rootPath?: string | null;
    agentStatus?: AgentStatus;
    lastAgentMessage?: string | null;
    notification?: NotificationState;
  } = {},
): Workspace {
  const tab = terminalTab(id * 10, opts.notification ?? "none");
  return {
    id,
    name: opts.name ?? `ws ${id}`,
    rootPath: opts.rootPath ?? null,
    distro: null,
    gitBranch: null,
    gitDirty: null,
    layout: { type: "leaf", pane: id },
    panes: { [String(id)]: { id, tabs: [tab], activeTab: tab.id } },
    activePane: id,
    agentStatus: opts.agentStatus ?? "idle",
    lastAgentMessage: opts.lastAgentMessage ?? null,
  };
}

function snapshot(
  revision: number,
  workspaces: Workspace[],
  activeWorkspace: WorkspaceId | null,
): StateSnapshot {
  return { revision, state: { workspaces, activeWorkspace, nextId: 100, revision } };
}

/** 카드 안의 자식 조회 — 없으면 던진다 (테스트에서 null 분기를 없애기 위함). */
function child(card: Element, selector: string): HTMLElement {
  const found = card.querySelector<HTMLElement>(selector);
  if (found === null) throw new Error(`missing ${selector}`);
  return found;
}

function mount(): {
  sidebar: Sidebar;
  cards: () => HTMLElement[];
  dispatched: Command[];
} {
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  const dispatched: Command[] = [];
  const sidebar = new Sidebar(root, async (cmd) => {
    dispatched.push(cmd);
    return null;
  });
  const cardsEl = root.querySelector<HTMLElement>(".sidebar-cards");
  if (cardsEl === null) throw new Error("sidebar-cards not mounted");
  return {
    sidebar,
    cards: () => Array.from(cardsEl.querySelectorAll<HTMLElement>(".ws-card")),
    dispatched,
  };
}

const THREE = [ws(1), ws(2), ws(3)];

describe("Sidebar rendering", () => {
  it("patches dynamic fields in place, keeping the same card nodes", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();
    expect(before).toHaveLength(3);
    // 안 바뀌는 이름의 텍스트 노드 — setText 가드가 이 노드를 살려두는지 본다.
    const nameNode = child(before[1], ".ws-card-name").firstChild;
    expect(child(before[1], ".ws-card-dot").hidden).toBe(true);
    expect(child(before[1], ".ws-card-status").textContent).toBe("idle");

    sidebar.render(
      snapshot(
        2,
        [
          ws(1),
          ws(2, {
            agentStatus: "needsInput",
            lastAgentMessage: "continue?\n두 번째 줄",
            notification: "unread",
          }),
          ws(3),
        ],
        1,
      ),
    );

    const after = cards();
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[2]).toBe(before[2]);
    // 상태 줄은 상태 텍스트 + 메시지 첫 줄 한 줄이다 (별도 메시지 줄 없음).
    expect(child(after[1], ".ws-card-status").textContent).toBe("needs input — continue?");
    expect(child(after[1], ".ws-card-status").title).toBe("needs input — continue?");
    expect(child(after[1], ".ws-card-status").classList.contains("needs-input")).toBe(true);
    expect(child(after[1], ".ws-card-dot").hidden).toBe(false);
    expect(child(after[1], ".ws-card-name").firstChild).toBe(nameNode);
  });

  it("toggles the path row in place, keeping it out of the layout while null", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();
    expect(child(before[0], ".ws-card-path").hidden).toBe(true);

    sidebar.render(snapshot(2, [ws(1, { rootPath: "/home/u/code/wmux" }), ws(2), ws(3)], 1));

    const after = cards();
    expect(after[0]).toBe(before[0]);
    expect(child(after[0], ".ws-card-path").hidden).toBe(false);
    expect(child(after[0], ".ws-card-path").textContent).toBe("~/code/wmux");
  });

  it("touches nothing when the card model is unchanged", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();
    const statusNode = child(before[0], ".ws-card-status").firstChild;

    // revision 만 다른 무관 스냅샷 — skip 판정이라 DOM 을 건드리지 않는다.
    sidebar.render(snapshot(2, THREE, 1));

    const after = cards();
    expect(after[0]).toBe(before[0]);
    expect(child(after[0], ".ws-card-status").firstChild).toBe(statusNode);
  });

  it("moves the active class without rebuilding the cards", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();
    expect(before.map((c) => c.classList.contains("active"))).toEqual([true, false, false]);

    sidebar.render(snapshot(2, THREE, 2));

    const after = cards();
    expect(after[1]).toBe(before[1]);
    expect(after.map((c) => c.classList.contains("active"))).toEqual([false, true, false]);
  });

  it("rebuilds when workspace membership changes", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();

    sidebar.render(snapshot(2, [ws(1), ws(2), ws(3), ws(4)], 1));

    const after = cards();
    expect(after).toHaveLength(4);
    expect(after[0]).not.toBe(before[0]);
  });

  it("rebuilds when the cards are reordered", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const before = cards();

    sidebar.render(snapshot(2, [ws(2), ws(1), ws(3)], 1));

    const after = cards();
    expect(after.map((c) => child(c, ".ws-card-name").textContent)).toEqual([
      "ws 2",
      "ws 1",
      "ws 3",
    ]);
    expect(after[0]).not.toBe(before[0]);
  });
});

describe("Sidebar interaction across patches", () => {
  it("keeps the switch wiring on patched cards", () => {
    const { sidebar, cards, dispatched } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    sidebar.render(
      snapshot(2, [ws(1), ws(2, { agentStatus: "running", notification: "unread" }), ws(3)], 1),
    );

    cards()[1].click();

    expect(dispatched).toEqual([{ type: "switchWorkspace", workspace: 2 }]);
  });

  it("re-reads active from the patched model instead of a stale closure", () => {
    const { sidebar, cards, dispatched } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    // ws 2 가 활성이 된 뒤의 클릭은 no-op 이어야 한다 (계획 D4).
    sidebar.render(snapshot(2, THREE, 2));

    cards()[1].click();
    expect(dispatched).toEqual([]);

    // 반대로 비활성이 된 ws 1 은 다시 전환을 보낸다.
    cards()[0].click();
    expect(dispatched).toEqual([{ type: "switchWorkspace", workspace: 1 }]);
  });
});
