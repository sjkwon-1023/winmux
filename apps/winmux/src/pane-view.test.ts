// @vitest-environment happy-dom
//
// 탭바 DOM identity 검증 (18단계 B-7) — 순수 판정(tabStripPlan) 테스트가 못 잡는
// 부분을 잠근다: 판정이 patch 여도 렌더가 실제로 탭 버튼을 갈아치우면 클릭이
// mousedown~click 사이에 유실된다 (ADR-0003 결정 7 의 스왈로). 그래서 "같은 노드
// 객체(===)에 제목·dot 만 갱신됐는가"를 실제 DOM 으로 단언한다. pane 층 배지의
// on/off 와, mousedown(FocusPane·send-mode) → click(ActivateTab) 순서도 함께 건다.
//
// 21단계에서 뷰어 seam 이 붙는다: placeholder 는 터미널도 뷰어도 없을 때만 뜬다는
// 상호 배타 규칙과, 헤더 폴더 버튼의 CreateTab 명세를 여기서 잠근다.
//
// happy-dom 은 이 파일 전용 환경이다 (상단 @vitest-environment) — 나머지 프론트
// 테스트는 계속 DOM 없는 node 환경에서 돈다.

import { describe, expect, it } from "vitest";

import { PaneView } from "./pane-view";
import type { SendController, ViewRegistry, ViewerRegistry } from "./pane-view";
import type { VisibleViewer } from "./view-reconcile";
import type { ViewerKind, ViewerView } from "./viewer-view";
import type { Command, NotificationState, Pane, PaneId, Tab, TerminalStatus } from "./types";

function terminalTab(
  id: number,
  opts: { title?: string; status?: TerminalStatus; notification?: NotificationState } = {},
): Tab {
  return {
    id,
    title: opts.title ?? `tab ${id}`,
    kind: {
      type: "terminal",
      ptySession: id * 100,
      status: opts.status ?? { type: "running" },
      cwd: null,
    },
    notification: opts.notification ?? "none",
    lastActivityMs: null,
  };
}

function folderTab(id: number, path = "/home/u"): Tab {
  return {
    id,
    title: `folder ${id}`,
    kind: { type: "folderBrowser", path },
    notification: "none",
    lastActivityMs: null,
  };
}

function pane(tabs: Tab[], activeTab: number | null): Pane {
  return { id: 1, tabs, activeTab };
}

/** 최소 ViewerView 스텁 — 마운트 사실과 update 로 흘러온 kind 만 기록한다. */
class FakeViewerView implements ViewerView {
  readonly root: HTMLDivElement;
  readonly kinds: ViewerKind[] = [];
  disposed = false;

  constructor(parent: HTMLElement) {
    this.root = document.createElement("div");
    this.root.className = "fake-viewer";
    parent.appendChild(this.root);
  }

  update(kind: ViewerKind): void {
    this.kinds.push(kind);
  }
  flushScroll(): void {}
  focus(): void {}
  dispose(): void {
    this.disposed = true;
    this.root.remove();
  }
}

/** 탭 안의 자식 조회 — 없으면 던진다 (테스트에서 null 분기를 없애기 위함). */
function child(el: Element, selector: string): HTMLElement {
  const found = el.querySelector<HTMLElement>(selector);
  if (found === null) throw new Error(`missing ${selector}`);
  return found;
}

interface Harness {
  view: PaneView;
  tabs: () => HTMLElement[];
  badge: () => HTMLElement;
  placeholder: () => HTMLElement;
  headerButton: (title: string) => HTMLButtonElement;
  dispatched: Command[];
  send: { active: boolean; resolved: PaneId[] };
  viewers: Map<number, FakeViewerView>;
  /** ensure 가 null 을 주는 탭 — 아직 구현이 없는 뷰어 종류를 흉내낸다. */
  unmountable: Set<number>;
}

function mount(paneId = 1): Harness {
  const dispatched: Command[] = [];
  const send = { active: false, resolved: [] as PaneId[] };
  const viewers = new Map<number, FakeViewerView>();
  const unmountable = new Set<number>();
  // 터미널 뷰 레지스트리는 쓰지 않는다 — 이 테스트들은 visible=null 로만
  // 렌더하므로 ensure 가 불리면 그 자체가 결함이다.
  const views: ViewRegistry = {
    get: () => undefined,
    ensure: () => {
      throw new Error("ensure should not be called");
    },
  };
  const viewerRegistry: ViewerRegistry = {
    get: (tab) => viewers.get(tab),
    ensure: (target: VisibleViewer, parent) => {
      if (unmountable.has(target.tab)) return null;
      const existing = viewers.get(target.tab);
      if (existing !== undefined) return existing;
      const created = new FakeViewerView(parent);
      viewers.set(target.tab, created);
      return created;
    },
  };
  const controller: SendController = {
    isActive: () => send.active,
    arm: () => {},
    resolve: (target) => send.resolved.push(target),
    flashError: () => {},
  };
  const view = new PaneView(
    paneId,
    async (cmd) => {
      dispatched.push(cmd);
      return null;
    },
    views,
    viewerRegistry,
    controller,
  );
  document.body.replaceChildren(view.root);
  return {
    view,
    tabs: () => Array.from(view.root.querySelectorAll<HTMLElement>(".pane-tabs .tab")),
    badge: () => child(view.root, ".pane-dot"),
    placeholder: () => child(view.root, ".pane-placeholder"),
    headerButton: (title) => {
      // 접두사 매칭 — 툴팁 뒤에 단축키 표기 "(Ctrl+Shift+…)"가 붙으므로 기능
      // 설명 부분으로만 찾는다 (테스트가 단축키 문자열을 하드코딩하지 않게).
      const found = view.root.querySelector<HTMLButtonElement>(
        `.pane-header button[title^="${title}"]`,
      );
      if (found === null) throw new Error(`missing header button ${title}`);
      return found;
    },
    dispatched,
    send,
    viewers,
    unmountable,
  };
}

/** planViewerSync 가 내려주는 마운트 항목 형태. */
function viewerMount(tab: number, path = "/home/u"): VisibleViewer {
  return { pane: 1, tab, kind: { type: "folderBrowser", path } };
}

const THREE = [terminalTab(10), terminalTab(11), terminalTab(12)];

describe("PaneView tab strip rendering", () => {
  it("patches a changed title in place, keeping the same tab nodes", () => {
    const { view, tabs } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();
    expect(before).toHaveLength(3);
    // 안 바뀌는 탭의 제목 텍스트 노드 — setText 가드가 이 노드를 살려두는지 본다.
    const untouched = child(before[0], ".tab-title").firstChild;

    view.update(
      pane([terminalTab(10), terminalTab(11, { title: "claude — winmux" }), terminalTab(12)], 10),
      true,
      null,
      null,
    );

    const after = tabs();
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after[2]).toBe(before[2]);
    expect(child(after[1], ".tab-title").textContent).toBe("claude — winmux");
    expect(after[1].title).toBe("claude — winmux");
    expect(child(after[0], ".tab-title").firstChild).toBe(untouched);
  });

  it("toggles the unread dot and the exited badge without rebuilding", () => {
    const { view, tabs } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();
    expect(child(before[1], ".tab-dot").hidden).toBe(true);
    expect(child(before[1], ".tab-exited").hidden).toBe(true);

    view.update(
      pane(
        [
          terminalTab(10),
          terminalTab(11, { notification: "unread", status: { type: "exited", code: 1 } }),
          terminalTab(12),
        ],
        10,
      ),
      true,
      null,
      null,
    );

    const after = tabs();
    expect(after[1]).toBe(before[1]);
    expect(child(after[1], ".tab-dot").hidden).toBe(false);
    expect(child(after[1], ".tab-exited").hidden).toBe(false);
    expect(after[1].classList.contains("exited")).toBe(true);
  });

  it("moves the active class without rebuilding the tabs", () => {
    const { view, tabs } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();
    expect(before.map((t) => t.classList.contains("active"))).toEqual([true, false, false]);

    view.update(pane(THREE, 11), true, null, null);

    const after = tabs();
    expect(after[1]).toBe(before[1]);
    expect(after.map((t) => t.classList.contains("active"))).toEqual([false, true, false]);
  });

  it("touches nothing when the tab model is unchanged", () => {
    const { view, tabs } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();
    const titleNode = child(before[0], ".tab-title").firstChild;

    // 무관 스냅샷(예: 다른 pane 의 FocusPane) — skip 판정이라 DOM 무접촉.
    view.update(pane(THREE, 10), false, null, null);

    const after = tabs();
    expect(after[0]).toBe(before[0]);
    expect(child(after[0], ".tab-title").firstChild).toBe(titleNode);
  });

  it("rebuilds when a tab is added or removed", () => {
    const { view, tabs } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();

    view.update(pane([...THREE, terminalTab(13)], 10), true, null, null);
    const grown = tabs();
    expect(grown).toHaveLength(4);
    expect(grown[0]).not.toBe(before[0]);

    view.update(pane([terminalTab(10), terminalTab(12)], 10), true, null, null);
    const shrunk = tabs();
    expect(shrunk).toHaveLength(2);
    expect(shrunk.map((t) => child(t, ".tab-title").textContent)).toEqual(["tab 10", "tab 12"]);
    expect(shrunk[0]).not.toBe(grown[0]);
  });
});

describe("PaneView unread badge", () => {
  it("stays hidden while no tab has an unread notification", () => {
    const { view, badge } = mount();
    view.update(pane(THREE, 10), true, null, null);
    expect(badge().hidden).toBe(true);
  });

  it("shows for an unread hidden tab and clears when the tab is read", () => {
    const { view, tabs, badge } = mount();
    view.update(pane(THREE, 10), true, null, null);
    const before = tabs();

    // 표시 중이 아닌 탭(12)의 알림 — pane 층 배지가 이걸 표면화한다.
    view.update(
      pane([terminalTab(10), terminalTab(11), terminalTab(12, { notification: "unread" })], 10),
      true,
      null,
      null,
    );
    expect(badge().hidden).toBe(false);
    // 배지 갱신이 탭 노드를 갈아치우지 않는다.
    expect(tabs()[0]).toBe(before[0]);

    view.update(pane(THREE, 10), true, null, null);
    expect(badge().hidden).toBe(true);
  });

  it("survives a strip rebuild", () => {
    const { view, badge } = mount();
    view.update(pane(THREE, 10), true, null, null);
    view.update(
      pane([...THREE, terminalTab(13, { notification: "unread" })], 10),
      true,
      null,
      null,
    );
    expect(badge().hidden).toBe(false);
  });
});

describe("PaneView tab interaction across patches", () => {
  it("keeps the activate and close wiring on patched tabs", () => {
    const { view, tabs, dispatched } = mount();
    view.update(pane(THREE, 10), true, null, null);
    view.update(
      pane([terminalTab(10), terminalTab(11, { title: "renamed" }), terminalTab(12)], 10),
      true,
      null,
      null,
    );

    tabs()[1].click();
    expect(dispatched).toEqual([{ type: "activateTab", tab: 11 }]);

    child(tabs()[2], ".tab-close").click();
    expect(dispatched).toEqual([
      { type: "activateTab", tab: 11 },
      { type: "closeTab", tab: 12 },
    ]);
  });

  it("re-reads active from the patched model instead of a stale closure", () => {
    const { view, tabs, dispatched } = mount();
    view.update(pane(THREE, 10), true, null, null);
    // 탭 11 이 활성이 된 뒤의 클릭은 no-op 이어야 한다 (무변경 revision 잡음 방지).
    view.update(pane(THREE, 11), true, null, null);

    tabs()[1].click();
    expect(dispatched).toEqual([]);

    // 반대로 비활성이 된 탭 10 은 다시 활성화를 보낸다.
    tabs()[0].click();
    expect(dispatched).toEqual([{ type: "activateTab", tab: 10 }]);
  });

  it("keeps the mousedown FocusPane ahead of the tab click", () => {
    const { view, tabs, dispatched } = mount(3);
    view.update(pane(THREE, 10), false, null, null); // 비활성 pane

    const tab = tabs()[1];
    tab.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    tab.click();

    expect(dispatched).toEqual([
      { type: "focusPane", pane: 3 },
      { type: "activateTab", tab: 11 },
    ]);
  });

  it("resolves the send target instead of focusing while send-mode is armed", () => {
    const { view, tabs, dispatched, send } = mount(3);
    view.update(pane(THREE, 10), false, null, null);
    send.active = true;

    tabs()[1].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));

    expect(send.resolved).toEqual([3]);
    expect(dispatched).toEqual([]);
  });
});

describe("PaneView viewer seam (21단계)", () => {
  it("mounts the viewer and hides the placeholder (no simultaneous display)", () => {
    const { view, placeholder, viewers } = mount();
    view.update(pane([folderTab(10)], 10), true, null, viewerMount(10));

    expect(viewers.get(10)?.root.isConnected).toBe(true);
    expect(placeholder().style.display).toBe("none");
    expect(view.shownTab).toBe(10);
  });

  it("pushes the current kind on every render so navigation reaches the view", () => {
    const { view, viewers } = mount();
    view.update(pane([folderTab(10)], 10), true, null, viewerMount(10));
    view.update(pane([folderTab(10, "/etc")], 10), true, null, viewerMount(10, "/etc"));

    expect(viewers.get(10)?.kinds).toEqual([
      { type: "folderBrowser", path: "/home/u" },
      { type: "folderBrowser", path: "/etc" },
    ]);
  });

  it("falls back to the placeholder when no viewer could be mounted", () => {
    const { view, placeholder, unmountable } = mount();
    // 아직 구현이 없는 뷰어 종류(청크 C2·D) — ensure 가 null 을 준다.
    unmountable.add(10);
    view.update(pane([folderTab(10)], 10), true, null, viewerMount(10));

    expect(view.shownTab).toBeNull();
    expect(placeholder().style.display).not.toBe("none");
    expect(placeholder().textContent).toContain("/home/u");
  });

  it("shows the placeholder only when there is neither a terminal nor a viewer", () => {
    const { view, placeholder } = mount();
    view.update(pane([terminalTab(10)], 10), true, null, null);
    expect(placeholder().style.display).not.toBe("none");
    expect(view.shownTab).toBeNull();
  });

  it("dispatches CreateTab{folderBrowser, path: null} from the header folder button", () => {
    const { view, headerButton, dispatched } = mount(4);
    view.update(pane(THREE, 10), true, null, null);

    headerButton("New folder browser tab").click();

    expect(dispatched).toEqual([
      { type: "createTab", pane: 4, tab: { type: "folderBrowser", path: null } },
    ]);
  });
});
