// pane 1개의 뷰 — 헤더(탭바 + 탭 생성·분할 아이콘) + keep-alive 콘텐츠 영역
// (12단계 청크 C).
//
// 콘텐츠는 keep-alive 다 (계획 D3): 탭별 TerminalView 는 앱 수준 레지스트리
// (workspace-view 소유 Map<TabId, TerminalView>)가 소유하고, 여기서는 ViewRegistry
// 를 통해 얻어 setVisible(display 토글)로 전환만 한다 — 탭 전환에 dispose/재생성·
// replay 왕복이 없다. 어떤 탭이 보일지는 view-reconcile(planViewSync)의 visible
// 판정을 workspace-view 가 pane 별로 내려준다 (판정 로직 단일화). 뷰 생성(lazy
// attach)은 첫 가시화 때 ensure 로 일어난다.
//
// fit 은 pane 당 ResizeObserver 1개(콘텐츠 영역 관찰 — 계획 D7)가 표시 중인 뷰의
// scheduleFit 만 부른다. 뷰당 observer 는 없다 (terminal-view 참조).
//
// 클릭 포커스(계획 1-B): 컨테이너 mousedown 을 capture 단계에서 받아 비활성
// pane 이면 FocusPane 을 dispatch 한다. preventDefault 는 하지 않는다 — xterm 의
// 포커스·선택 처리를 강탈하면 안 되기 때문이다. DOM 포커스는 그대로 흘러가고
// 모델의 active_pane 만 따라온다.

import { tabStripModel } from "./tab-strip-model";
import type { TabButtonModel } from "./tab-strip-model";
import type { TerminalView } from "./terminal-view";
import type { VisibleView } from "./view-reconcile";
import type {
  Command,
  CommandOutput,
  Pane,
  PaneId,
  SessionId,
  Tab,
  TabId,
  TabKind,
} from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼. 실패는 상태 라인에 표면화되고
 *  null 로 돌아온다 (reject 하지 않는다). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** keep-alive 뷰 레지스트리 접근 계약 — 소유자는 workspace-view 다.
 *  ensure 는 없으면 생성 + attach 시작(lazy)하고, attach 실패 시 뷰를 정리한 뒤
 *  onAttachError 로 알린다 (호출한 pane 이 placeholder 에 에러를 노출한다). */
export interface ViewRegistry {
  get(tab: TabId): TerminalView | undefined;
  ensure(
    tab: TabId,
    session: SessionId,
    parent: HTMLElement,
    onAttachError: (message: string) => void,
  ): TerminalView;
}

/** 콘텐츠 placeholder 텍스트 (터미널 뷰가 없는 경우 — 영어 UI 텍스트). */
function placeholderText(tab: Tab | null): string {
  if (tab === null) return "no tabs — press + to open a new terminal tab";
  const kind: TabKind = tab.kind;
  switch (kind.type) {
    case "terminal":
      // ptySession 없는 terminal 탭 — visible 판정에서 제외된 경우.
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
  private readonly tabStripEl: HTMLDivElement;
  private readonly placeholderEl: HTMLDivElement;
  private readonly resizeObserver: ResizeObserver;
  private isActive = false;
  private shown: TabId | null = null;

  constructor(
    readonly paneId: PaneId,
    private readonly dispatch: DispatchFn,
    private readonly views: ViewRegistry,
  ) {
    this.root = document.createElement("div");
    this.root.className = "pane";
    // 진단 (체크포인트 1 버그 3): DOM 상 어느 슬롯에 어느 pane 의 뷰가 앉았는지
    // devtools·rebuild 로그에서 즉시 판별할 수 있게 id 를 데이터 속성으로 남긴다.
    this.root.dataset.paneId = String(paneId);

    this.contentEl = document.createElement("div");
    this.contentEl.className = "pane-content";
    this.placeholderEl = document.createElement("div");
    this.placeholderEl.className = "pane-placeholder";
    this.contentEl.append(this.placeholderEl);

    this.tabStripEl = document.createElement("div");
    this.tabStripEl.className = "pane-tabs";
    this.root.append(this.buildHeader(), this.contentEl);

    this.root.addEventListener(
      "mousedown",
      (ev) => {
        // 비활성 pane 클릭 → 모델 포커스 이동. preventDefault 금지 (파일 상단).
        // 탭 클릭도 이 경로가 FocusPane 을 담당한다 (onTabClick 주석 참조).
        // 주 버튼만 — 우/중클릭은 컨텍스트 메뉴·붙여넣기 등 다른 의미를 갖는다.
        if (ev.button !== 0) return;
        if (!this.isActive) void this.dispatch({ type: "focusPane", pane: this.paneId });
      },
      { capture: true },
    );

    // pane 당 observer 1개 (D7) — 표시 중인 뷰에만 fit 을 전달한다.
    this.resizeObserver = new ResizeObserver(() => {
      if (this.shown !== null) this.views.get(this.shown)?.scheduleFit();
    });
    this.resizeObserver.observe(this.contentEl);
  }

  /** 현재 표시 중인 탭 — workspace-view 의 focus 보상 경로가 조회한다. */
  get shownTab(): TabId | null {
    return this.shown;
  }

  private buildHeader(): HTMLElement {
    const header = document.createElement("div");
    header.className = "pane-header";

    const browser = this.iconButton("◎", "Browser tab (v2)", null);
    browser.disabled = true;

    // 진단 (체크포인트 1 버그 3): 헤더가 어느 pane 소속인지 화면에서 바로 보이게
    // id 를 표시한다 — "클릭한 헤더 ≠ 의도한 pane" 재현 시 즉시 판별된다.
    const idLabel = document.createElement("span");
    idLabel.className = "pane-id";
    idLabel.textContent = `#${this.paneId}`;

    header.append(
      idLabel,
      this.tabStripEl,
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
        const cmd = command();
        // 진단 로그 (체크포인트 1 버그 3 — "엉뚱한 pane 분할" 재현 시 어떤
        // PaneView 의 버튼이 어떤 커맨드를 보냈는지 즉시 판별하기 위한 상시
        // debug 레벨 기록. 정적 분석으로는 배선·구조 경로가 결백해 재현
        // 데이터가 필요하다).
        console.debug("[wmux] pane-icon click", { pane: this.paneId, cmd });
        void this.dispatch(cmd);
      });
    }
    return btn;
  }

  /** 스냅샷 반영 — 활성 테두리·탭바 갱신 + keep-alive 뷰 가시성 전환.
   *  visible 은 planViewSync 가 이 pane 에 대해 판정한 항목(없으면 null)이다. */
  update(pane: Pane, active: boolean, visible: VisibleView | null): void {
    this.isActive = active;
    this.root.classList.toggle("active", active);
    this.renderTabStrip(pane);

    if (visible !== null) {
      // 첫 가시화 때 lazy attach — 이미 있으면 레지스트리의 기존 뷰 그대로.
      this.views.ensure(visible.tab, visible.session, this.contentEl, (message) =>
        this.showAttachError(visible.tab, message),
      );
      this.shown = visible.tab;
    } else {
      this.shown = null;
    }

    // 이 pane 탭들의 keep-alive 뷰 가시성 동기화 — 표시 1개, 나머지 숨김.
    for (const tab of pane.tabs) {
      this.views.get(tab.id)?.setVisible(tab.id === this.shown);
    }

    if (this.shown === null) {
      const tab = pane.tabs.find((t) => t.id === pane.activeTab) ?? null;
      this.placeholderEl.textContent = placeholderText(tab);
      this.placeholderEl.style.display = ""; // 스타일시트의 flex 복원
    } else {
      this.placeholderEl.style.display = "none";
    }
  }

  /** 직전 렌더의 탭 모델 직렬화 키 — 무변경 시 DOM 재조립을 건너뛰기 위한 캐시. */
  private lastStripKey = "";

  /** 탭 모델이 변했을 때만 strip DOM 을 재조립한다. 클릭 진행 중(mousedown~click
   *  사이)에 무관 스냅샷 렌더가 눌린 탭 엘리먼트를 갈아치우면 브라우저가 click 을
   *  발화하지 않아 "비활성 pane 탭은 두 번 클릭해야 먹는" 버그가 된다 (리뷰 high
   *  finding). FocusPane 스냅샷은 이 pane 의 탭 모델을 바꾸지 않으므로 이 스킵이
   *  유실 창을 닫는다. */
  private renderTabStrip(pane: Pane): void {
    const model = tabStripModel(pane);
    const key = JSON.stringify(model);
    if (key === this.lastStripKey) return;
    this.lastStripKey = key;
    this.tabStripEl.replaceChildren(...model.map((m) => this.tabButton(m)));
  }

  private tabButton(model: TabButtonModel): HTMLElement {
    // 컨테이너는 div — X 가 <button> 이라 버튼 중첩을 피한다.
    const el = document.createElement("div");
    el.className = "tab";
    if (model.active) el.classList.add("active");
    if (model.exited) el.classList.add("exited");
    el.title = model.title; // 잘린 제목의 툴팁

    const title = document.createElement("span");
    title.className = "tab-title";
    title.textContent = model.title;
    el.append(title);

    if (model.notification) {
      const dot = document.createElement("span");
      dot.className = "tab-dot";
      dot.textContent = "●";
      el.append(dot);
    }
    if (model.exited) {
      const badge = document.createElement("span");
      badge.className = "tab-exited";
      badge.textContent = "exited";
      el.append(badge);
    }

    const close = document.createElement("button");
    close.type = "button";
    close.className = "tab-close";
    close.textContent = "×";
    close.title = "Close tab";
    close.addEventListener("click", (ev) => {
      ev.stopPropagation(); // 탭 활성화 클릭과 분리
      void this.dispatch({ type: "closeTab", tab: model.tab });
    });
    el.append(close);

    el.addEventListener("click", () => this.onTabClick(model));
    return el;
  }

  /** 탭 클릭 처리. 비활성 pane 의 FocusPane 은 root 의 mousedown capture 가
   *  같은 제스처(mousedown → click 순서)에서 이미 dispatch 했다 — 여기서 또
   *  보내면 무변경 revision 잡음이 된다. */
  private onTabClick(model: TabButtonModel): void {
    if (!model.active) {
      // ActivateTab 성공 시의 뷰 focus 는 main.dispatchUI 의 보상 경로가
      // requestFocus 로 처리한다 (계획 D7).
      void this.dispatch({ type: "activateTab", tab: model.tab });
      return;
    }
    // 이미 active 탭: dispatch 없이(no-op 스킵) 뷰 focus 만. pane 이 비활성인
    // 경우는 mousedown 의 FocusPane 성공 보상이 focus 를 처리한다.
    if (this.isActive) this.views.get(model.tab)?.focus();
  }

  /** attach 실패 노출 — 레지스트리(ensure)가 뷰를 정리한 뒤 부른다. 실패한 탭이
   *  아직 표시 대상이면 placeholder 에 에러를 띄운다 (다음 스냅샷 렌더가 재시도). */
  private showAttachError(tab: TabId, message: string): void {
    if (this.shown !== tab) return;
    this.shown = null;
    this.placeholderEl.textContent = message;
    this.placeholderEl.style.display = "";
  }

  /** pane 뷰 해제 — observer·DOM 만 정리한다. 터미널 뷰는 레지스트리 소유라
   *  여기서 dispose 하지 않는다 — pane 이 닫히면 그 탭들이 스냅샷에서 사라져
   *  view-reconcile 의 dispose 목록으로 정리된다 (계획 D3). */
  dispose(): void {
    this.resizeObserver.disconnect();
    this.root.remove();
  }
}
