// pane 1개의 뷰 — 헤더(탭 생성·분할 아이콘) + 콘텐츠 영역 (11단계 청크 B).
//
// [임시 구조 — 12단계(청크 C)에서 교체 예정] 콘텐츠 렌더는 10단계 main.ts 의
// 단일 뷰 로직을 pane 단위로 일반화한 것이다: 활성 탭 1개만 TerminalView 로
// 렌더하고 탭/세션이 바뀌면 dispose 후 재생성한다. 탭별 keep-alive(뷰 Map +
// display:none 유지, 계획 D3)·탭바·pane 당 ResizeObserver 1개(D7)는 다음 청크가
// 이 파일을 교체하며 얹는다.
//
// 클릭 포커스(계획 1-B): 컨테이너 mousedown 을 capture 단계에서 받아 비활성
// pane 이면 FocusPane 을 dispatch 한다. preventDefault 는 하지 않는다 — xterm 의
// 포커스·선택 처리를 강탈하면 안 되기 때문이다. DOM 포커스는 그대로 흘러가고
// 모델의 active_pane 만 따라온다.

import { TerminalView } from "./terminal-view";
import type {
  Command,
  CommandOutput,
  Pane,
  PaneId,
  SessionId,
  Tab,
  TabKind,
} from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼. 실패는 상태 라인에 표면화되고
 *  null 로 돌아온다 (reject 하지 않는다). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** 터미널 뷰를 붙일 세션 — terminal 탭이고 ptySession 이 있을 때만. */
function sessionOf(tab: Tab | null): SessionId | null {
  if (tab === null || tab.kind.type !== "terminal") return null;
  return tab.kind.ptySession;
}

/** 콘텐츠 영역 placeholder 텍스트 (터미널 뷰가 없는 경우 — 영어 UI 텍스트). */
function placeholderText(tab: Tab | null): string {
  if (tab === null) return "(no tabs — use the header buttons)";
  const kind: TabKind = tab.kind;
  switch (kind.type) {
    case "terminal":
      // sessionOf 가 null 을 준 경우 — ptySession 없는 terminal 탭.
      return "(terminal tab without pty session)";
    case "folderBrowser":
      return `folderBrowser: ${kind.path} (viewer lands in stage 21)`;
    case "textViewer":
      return `textViewer: ${kind.path} (viewer lands in stage 21)`;
    case "markdownViewer":
      return `markdownViewer: ${kind.path} (viewer lands in stage 21)`;
  }
}

export class PaneView {
  readonly root: HTMLDivElement;
  private readonly contentEl: HTMLDivElement;
  private view: TerminalView | null = null;
  private viewSession: SessionId | null = null;
  private isActive = false;

  constructor(
    readonly paneId: PaneId,
    private readonly dispatch: DispatchFn,
  ) {
    this.root = document.createElement("div");
    this.root.className = "pane";

    this.contentEl = document.createElement("div");
    this.contentEl.className = "pane-content";
    this.root.append(this.buildHeader(), this.contentEl);

    this.root.addEventListener(
      "mousedown",
      () => {
        // 비활성 pane 클릭 → 모델 포커스 이동. preventDefault 금지 (파일 상단).
        if (!this.isActive) void this.dispatch({ type: "focusPane", pane: this.paneId });
      },
      { capture: true },
    );
  }

  private buildHeader(): HTMLElement {
    const header = document.createElement("div");
    header.className = "pane-header";

    const browser = this.iconButton("◎", "Browser tab (v2)", null);
    browser.disabled = true;

    header.append(
      this.iconButton("+", "New terminal tab", () => ({
        type: "createTab",
        pane: this.paneId,
        tab: { type: "terminal", cwd: null },
      })),
      browser,
      // 분할은 원자 SplitPane — 새 pane 에 terminal 탭까지 한 번에 생성한다
      // (계획 D5: 컴포지션 금지, 중간 스냅샷 1프레임 렌더 방지).
      this.iconButton("◫", "Split left/right", () => ({
        type: "splitPane",
        pane: this.paneId,
        direction: "horizontal",
        tab: { type: "terminal", cwd: null },
      })),
      this.iconButton("⊟", "Split top/bottom", () => ({
        type: "splitPane",
        pane: this.paneId,
        direction: "vertical",
        tab: { type: "terminal", cwd: null },
      })),
    );
    return header;
  }

  private iconButton(
    label: string,
    title: string,
    command: (() => Command) | null,
  ): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = label;
    btn.title = title;
    if (command !== null) {
      btn.addEventListener("click", () => {
        void this.dispatch(command());
      });
    }
    return btn;
  }

  /** 스냅샷 반영 — 활성 테두리 토글 + 콘텐츠(활성 탭 1개) 갱신.
   *  같은 세션이면 뷰를 유지한다 — revision 마다 재attach 하면 replay 왕복·
   *  화면 리셋이 반복된다 (10단계 main.ts 로직 승계). */
  update(pane: Pane, active: boolean): void {
    this.isActive = active;
    this.root.classList.toggle("active", active);

    const tab = pane.tabs.find((t) => t.id === pane.activeTab) ?? null;
    const session = sessionOf(tab);
    if (session === this.viewSession) {
      if (this.view === null) this.contentEl.textContent = placeholderText(tab);
      return;
    }

    this.view?.dispose();
    this.view = null;
    this.viewSession = session;
    this.contentEl.replaceChildren(); // placeholder 텍스트·뷰 잔재 제거

    if (session === null) {
      this.contentEl.textContent = placeholderText(tab);
      return;
    }

    const view = new TerminalView(this.contentEl, session);
    this.view = view;
    view.attach().catch((err) => {
      // attach 실패는 가리지 않는다 — 뷰를 정리하고 에러를 화면에 노출.
      console.error("attach_terminal failed", err);
      if (this.view !== view) return; // 이미 다른 뷰로 전환됨
      view.dispose();
      this.view = null;
      this.viewSession = null;
      this.contentEl.textContent = `attach failed (session ${session}): ${String(err)}`;
    });
  }

  /** pane 뷰 해제 — 터미널 뷰 dispose(채널 detach 포함) + DOM 제거.
   *  세션 수명은 dispatcher 소유 — 여기서 세션을 죽이지 않는다. */
  dispose(): void {
    this.view?.dispose();
    this.view = null;
    this.viewSession = null;
    this.root.remove();
  }
}
