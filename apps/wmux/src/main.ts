// wmux 앱 엔트리 — store 구독 → 활성 workspace/pane/탭 1개를 전면 렌더 (10단계).
// 분할 렌더·탭바 UI 는 11~12단계 — 지금은 상단 상태 라인(워크스페이스 이름·탭 수·
// revision) + 단일 터미널 뷰. 커맨드 트리거는 dev 훅 window.__wmux (계획 3-C —
// 실 UI 전까지의 명시적 조작 표면. 예: __wmux.dispatch({type:"createTab", ...})).

import { dispatch, getState } from "./backend";
import { Store } from "./store";
import { TerminalView } from "./terminal-view";
import type { Pane, SessionId, StateSnapshot, Tab, TabKind, Workspace } from "./types";

declare global {
  interface Window {
    /** dev 조작 표면 — 실 UI(11~13단계) 전까지 dispatch/getState 를 콘솔에서 호출한다. */
    __wmux: { dispatch: typeof dispatch; getState: typeof getState };
  }
}

function requireElement(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`missing #${id} element`);
  return el;
}

/** 활성 workspace → 활성 pane → 활성 탭 해석. 각 단계 부재 시 null. */
function resolveActive(snapshot: StateSnapshot): {
  ws: Workspace | null;
  pane: Pane | null;
  tab: Tab | null;
} {
  const ws =
    snapshot.state.workspaces.find((w) => w.id === snapshot.state.activeWorkspace) ?? null;
  // panes 맵 키는 문자열 숫자 (JSON object 키 제약 — types.ts 참조).
  const pane = ws === null ? null : (ws.panes[String(ws.activePane)] ?? null);
  const tab = pane?.tabs.find((t) => t.id === pane.activeTab) ?? null;
  return { ws, pane, tab };
}

/** 터미널 뷰를 붙일 세션 — terminal 탭이고 pty_session 이 있을 때만. */
function sessionOf(tab: Tab | null): SessionId | null {
  if (tab === null || tab.kind.type !== "terminal") return null;
  return tab.kind.ptySession;
}

/** 뷰 영역에 표시할 placeholder 텍스트 (터미널 뷰가 없는 경우). */
function placeholderText(tab: Tab | null): string {
  if (tab === null) return "(no active tab — __wmux.dispatch 로 createTab 하세요)";
  const kind: TabKind = tab.kind;
  switch (kind.type) {
    case "terminal":
      // sessionOf 가 null 을 준 경우 — pty_session 없는 terminal 탭.
      return "(terminal tab without pty session)";
    case "folderBrowser":
      return `folderBrowser: ${kind.path} (뷰어는 21단계)`;
    case "textViewer":
      return `textViewer: ${kind.path} (뷰어는 21단계)`;
    case "markdownViewer":
      return `markdownViewer: ${kind.path} (뷰어는 21단계)`;
  }
}

class App {
  private readonly statusEl = requireElement("status-line");
  private readonly viewEl = requireElement("view");
  private readonly store = new Store();
  private view: TerminalView | null = null;
  private viewSession: SessionId | null = null;

  async init(): Promise<void> {
    // dev 훅은 부트스트랩 실패와 무관하게 먼저 노출한다 — 실패 시 콘솔에서
    // getState 로 상태를 직접 확인할 수 있어야 한다.
    window.__wmux = { dispatch, getState };
    this.store.subscribe((snapshot) => this.render(snapshot));
    await this.store.init();
  }

  private render(snapshot: StateSnapshot): void {
    const { ws, tab } = resolveActive(snapshot);
    this.renderStatusLine(snapshot, ws);
    this.renderView(sessionOf(tab), tab);
  }

  private renderStatusLine(snapshot: StateSnapshot, ws: Workspace | null): void {
    if (ws === null) {
      this.statusEl.textContent = `no workspace · rev ${snapshot.revision}`;
      return;
    }
    const tabCount = Object.values(ws.panes).reduce((n, p) => n + p.tabs.length, 0);
    this.statusEl.textContent = `workspace: ${ws.name} · tabs: ${tabCount} · rev ${snapshot.revision}`;
  }

  private renderView(session: SessionId | null, tab: Tab | null): void {
    if (session === this.viewSession) {
      // 같은 세션이면 뷰 유지 — revision 마다 재attach 하면 replay 왕복·화면
      // 리셋이 반복된다. 뷰가 없는 상태면 placeholder 텍스트만 갱신한다.
      if (this.view === null) this.viewEl.textContent = placeholderText(tab);
      return;
    }

    this.view?.dispose();
    this.view = null;
    this.viewSession = session;
    this.viewEl.replaceChildren(); // placeholder 텍스트·뷰 잔재 제거

    if (session === null) {
      this.viewEl.textContent = placeholderText(tab);
      return;
    }

    const view = new TerminalView(this.viewEl, session);
    this.view = view;
    view.attach().catch((err) => {
      // attach 실패는 가리지 않는다 — 뷰를 정리하고 에러를 화면에 그대로 노출.
      console.error("attach_terminal failed", err);
      if (this.view !== view) return; // 이미 다른 뷰로 전환됨
      view.dispose();
      this.view = null;
      this.viewSession = null;
      this.viewEl.textContent = `attach failed (session ${session}): ${String(err)}`;
    });
  }
}

async function main(): Promise<void> {
  const app = new App();
  await app.init();
}

main().catch((err) => {
  console.error("app bootstrap failed", err);
  // 부트스트랩 실패를 화면에도 노출한다 — 빈 화면으로 가리지 않는다.
  const el = document.getElementById("status-line");
  if (el !== null) el.textContent = `bootstrap failed: ${String(err)}`;
});
