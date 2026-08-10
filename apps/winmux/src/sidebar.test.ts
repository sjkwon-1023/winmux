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

/** 카드의 이름 편집 입력 (F2 대상). */
function renameInput(card: Element): HTMLInputElement {
  const found = card.querySelector<HTMLInputElement>(".ws-card-rename");
  if (found === null) throw new Error("missing .ws-card-rename");
  return found;
}

/** 편집 입력에 키를 하나 보낸다 — 실제 리스너(keydown)와 같은 경로. */
function press(input: HTMLInputElement, key: string): void {
  input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
}

function mount(): {
  sidebar: Sidebar;
  cards: () => HTMLElement[];
  dispatched: Command[];
  /** "+ New workspace" 콜백 호출 횟수 — 폴더 선택 흐름은 main.ts 소유라
   *  사이드바는 콜백만 부른다. */
  newWorkspaceCalls: () => number;
} {
  const root = document.createElement("div");
  document.body.replaceChildren(root);
  const dispatched: Command[] = [];
  let newWorkspaceCalls = 0;
  const sidebar = new Sidebar(
    root,
    async (cmd) => {
      dispatched.push(cmd);
      return null;
    },
    () => {
      newWorkspaceCalls += 1;
    },
  );
  const cardsEl = root.querySelector<HTMLElement>(".sidebar-cards");
  if (cardsEl === null) throw new Error("sidebar-cards not mounted");
  return {
    sidebar,
    cards: () => Array.from(cardsEl.querySelectorAll<HTMLElement>(".ws-card")),
    dispatched,
    newWorkspaceCalls: () => newWorkspaceCalls,
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

    sidebar.render(snapshot(2, [ws(1, { rootPath: "/home/u/code/winmux" }), ws(2), ws(3)], 1));

    const after = cards();
    expect(after[0]).toBe(before[0]);
    expect(child(after[0], ".ws-card-path").hidden).toBe(false);
    expect(child(after[0], ".ws-card-path").textContent).toBe("~/code/winmux");
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

describe("Sidebar inline rename (F2)", () => {
  it("commits with Enter, dispatching renameWorkspace for the active card", () => {
    const { sidebar, cards, dispatched } = mount();
    // ws 2 가 활성 — F2 는 활성 카드만 편집한다.
    sidebar.render(snapshot(1, THREE, 2));
    const card = cards()[1];
    const name = child(card, ".ws-card-name");
    const input = renameInput(card);
    expect(input.hidden).toBe(true);

    sidebar.beginRename();
    expect(input.hidden).toBe(false);
    expect(name.hidden).toBe(true);
    expect(input.value).toBe("ws 2"); // 현재 이름이 채워진다

    // 앞뒤 공백은 UI 가 다듬어 보낸다 (코어는 받은 값을 그대로 저장한다).
    input.value = "  renamed  ";
    press(input, "Enter");

    expect(dispatched).toEqual([{ type: "renameWorkspace", workspace: 2, name: "renamed" }]);
    // 확정하면 편집이 끝나고 카드가 원래 모습으로 돌아온다.
    expect(input.hidden).toBe(true);
    expect(name.hidden).toBe(false);
  });

  it("cancels with Escape and drops blank names without dispatching", () => {
    const { sidebar, cards, dispatched } = mount();
    sidebar.render(snapshot(1, THREE, 2));
    const card = cards()[1];
    const input = renameInput(card);

    sidebar.beginRename();
    input.value = "typed but abandoned";
    press(input, "Escape");
    expect(input.hidden).toBe(true);
    expect(child(card, ".ws-card-name").textContent).toBe("ws 2");
    expect(dispatched).toEqual([]);

    // 공백뿐인 이름은 코어가 거부하는 값 — 보내지 않고 편집을 유지한다.
    sidebar.beginRename();
    input.value = "   ";
    press(input, "Enter");
    expect(dispatched).toEqual([]);
    expect(input.hidden).toBe(false);

    // 이름이 그대로면 무변경 dispatch 도 보내지 않는다 (카드 클릭 규칙과 동일).
    input.value = "ws 2";
    press(input, "Enter");
    expect(dispatched).toEqual([]);
    expect(input.hidden).toBe(true);
  });

  it("skips patching the card being edited and catches up when editing ends", () => {
    const { sidebar, cards } = mount();
    sidebar.render(snapshot(1, THREE, 2));
    const card = cards()[1];
    const input = renameInput(card);
    sidebar.beginRename();
    input.value = "half typed";

    // 편집 중 도착한 스냅샷 — 이 카드는 건드리지 않는다 (입력값·IME 보호).
    sidebar.render(snapshot(2, [ws(1), ws(2, { agentStatus: "running" }), ws(3)], 2));
    expect(input.value).toBe("half typed");
    expect(child(card, ".ws-card-status").textContent).toBe("idle");

    // 편집이 끝나면 밀린 갱신을 한 번에 만회한다 (다음 스냅샷을 기다리지 않는다).
    press(input, "Escape");
    expect(child(card, ".ws-card-status").textContent).toBe("running");
  });
});

describe("Sidebar interaction across patches", () => {
  it("delegates the new-workspace button to the picker callback", () => {
    const { sidebar, newWorkspaceCalls } = mount();
    sidebar.render(snapshot(1, THREE, 1));
    const button = document.querySelector<HTMLElement>(".sidebar-new");
    if (button === null) throw new Error("missing .sidebar-new");

    button.click();

    // 대화상자 호출·CreateWorkspace dispatch 는 main.ts 소유다 (인라인 폼 없음).
    expect(newWorkspaceCalls()).toBe(1);
    expect(document.querySelector(".sidebar-form")).toBeNull();
  });

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
