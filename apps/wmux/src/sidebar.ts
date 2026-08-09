// 워크스페이스 사이드바 (13단계 D3·D4) — 카드 리스트 + 하단 "+ New workspace"
// 인라인 폼.
//
// 렌더 전략: 카드 모델(sidebar-model)의 직렬화 키가 직전과 같으면 카드 DOM
// 재조립을 건너뛴다 — pane-view 의 lastStripKey 와 같은 이유다: 클릭 진행 중
// (mousedown~click 사이) 무관 스냅샷 렌더가 눌린 카드 엘리먼트를 갈아치우면
// 브라우저가 click 을 발화하지 않아 클릭이 유실된다. 하단 폼은 카드 리스트와
// 분리된 고정 DOM 이라 재조립 대상이 아니다 — 입력 중 스냅샷이 와도 폼 상태가
// 날아가지 않는다.
//
// 폼 입력은 표준 DOM <input> 이다 — 키 가로채기를 하지 않는다. 폼에 포커스가
// 있는 동안 키 입력은 xterm 포커스 밖이라 터미널로 새지 않고, 전역 캡처는
// Ctrl+Shift+R(리로드)뿐이라 충돌하지 않는다.
//
// 상호작용 (계획 D4):
// - 카드 클릭 = SwitchWorkspace (이미 활성이면 no-op 스킵 — 무변경 revision 잡음 방지).
// - × = CloseWorkspace. 터미널 탭이 1개라도 있으면 confirm() 을 거친다 — 그
//   세션 전부를 죽이는 파괴적 동작이다. 판정은 렌더 캐시가 아니라 클릭 시점의
//   최신 스냅샷으로 한다 (카드 DOM 이 스킵으로 오래됐을 수 있다).
// - 제출 = CreateWorkspace { tab: terminal } 원자 생성 (계획 13-D1) — 빈
//   워크스페이스 프레임 없이 터미널까지 한 번에 뜬다. 성공 시 폼을 닫고,
//   실패는 dispatchUI 가 상태 라인에 표면화하므로 폼을 유지해 재시도하게 한다.

import { hasTerminalTabs, sidebarModel } from "./sidebar-model";
import type { WorkspaceCardModel } from "./sidebar-model";
import type { Command, CommandOutput, StateSnapshot, WorkspaceId } from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼 (실패는 null, reject 없음). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

export class Sidebar {
  private readonly cardsEl: HTMLDivElement;
  private readonly formEl: HTMLFormElement;
  private readonly nameInput: HTMLInputElement;
  private readonly rootPathInput: HTMLInputElement;
  private readonly distroInput: HTMLInputElement;
  private lastSnapshot: StateSnapshot | null = null;
  /** 직전 렌더의 카드 모델 직렬화 키 — 무변경 시 DOM 재조립 스킵 (파일 상단). */
  private lastCardsKey = "";
  /** 제출 in-flight 가드 — invoke 대기 중 중복 제출 방지. */
  private submitting = false;

  constructor(
    rootEl: HTMLElement,
    private readonly dispatch: DispatchFn,
  ) {
    this.cardsEl = document.createElement("div");
    this.cardsEl.className = "sidebar-cards";

    const footer = document.createElement("div");
    footer.className = "sidebar-footer";

    const newBtn = document.createElement("button");
    newBtn.type = "button";
    newBtn.className = "sidebar-new";
    newBtn.textContent = "+ New workspace";

    this.nameInput = this.textInput("name");
    this.nameInput.required = true;
    this.rootPathInput = this.textInput("/home/<user>/project (optional)");
    this.distroInput = this.textInput("WSL distro (optional)");

    const submit = document.createElement("button");
    submit.type = "submit";
    submit.className = "sidebar-form-submit";
    submit.textContent = "Create";

    this.formEl = document.createElement("form");
    this.formEl.className = "sidebar-form";
    this.formEl.hidden = true;
    this.formEl.append(this.nameInput, this.rootPathInput, this.distroInput, submit);
    this.formEl.addEventListener("submit", (ev) => {
      ev.preventDefault(); // 페이지 네비게이션 방지 — dispatch 로만 제출한다
      void this.submitNew();
    });

    newBtn.addEventListener("click", () => {
      this.formEl.hidden = !this.formEl.hidden;
      if (!this.formEl.hidden) this.nameInput.focus();
    });

    footer.append(newBtn, this.formEl);
    rootEl.append(this.cardsEl, footer);
  }

  /** 스냅샷 반영 진입점 — store 구독에서 revision 순으로 호출된다. */
  render(snapshot: StateSnapshot): void {
    this.lastSnapshot = snapshot;
    const model = sidebarModel(snapshot.state.workspaces, snapshot.state.activeWorkspace);
    const key = JSON.stringify(model);
    if (key === this.lastCardsKey) return;
    this.lastCardsKey = key;
    this.cardsEl.replaceChildren(...model.map((m) => this.card(m)));
  }

  private textInput(placeholder: string): HTMLInputElement {
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = placeholder;
    // WebView 자동완성·자동교정이 경로/이름 입력을 방해하지 않게 끈다.
    input.autocomplete = "off";
    input.spellcheck = false;
    return input;
  }

  private card(model: WorkspaceCardModel): HTMLElement {
    const el = document.createElement("div");
    el.className = "ws-card";
    if (model.active) el.classList.add("active");

    const head = document.createElement("div");
    head.className = "ws-card-head";

    const status = document.createElement("span");
    status.className = "ws-card-status";
    status.textContent = model.statusIcon;
    status.title = model.status;

    const name = document.createElement("span");
    name.className = "ws-card-name";
    name.textContent = model.name;
    name.title = model.name; // 잘린 이름의 툴팁

    const close = document.createElement("button");
    close.type = "button";
    close.className = "ws-card-close";
    close.textContent = "×";
    close.title = "Close workspace";
    close.addEventListener("click", (ev) => {
      ev.stopPropagation(); // 카드 클릭(전환)과 분리
      this.onClose(model.workspace);
    });

    head.append(status, name, close);
    el.append(head);

    if (model.message !== null) {
      const message = document.createElement("div");
      message.className = "ws-card-message";
      message.textContent = model.message;
      message.title = model.message;
      el.append(message);
    }

    // 메타 1줄 — branch · path · counts (null 필드는 생략, 말줄임은 CSS).
    const counts = `${model.counts.panes}p ${model.counts.tabs}t`;
    const metaParts = [model.branch, model.path, counts].filter((p): p is string => p !== null);
    const meta = document.createElement("div");
    meta.className = "ws-card-meta";
    meta.textContent = metaParts.join(" · ");
    meta.title = meta.textContent;
    el.append(meta);

    el.addEventListener("click", () => {
      // 이미 활성이면 no-op 스킵 (계획 D4).
      if (!model.active) {
        void this.dispatch({ type: "switchWorkspace", workspace: model.workspace });
      }
    });
    return el;
  }

  /** × 클릭 — 터미널 탭이 있으면 confirm 후 CloseWorkspace (계획 D4). */
  private onClose(workspace: WorkspaceId): void {
    const ws =
      this.lastSnapshot?.state.workspaces.find((w) => w.id === workspace) ?? null;
    if (ws === null) return; // 이미 닫힌 카드의 늦은 클릭 — 보낼 것이 없다
    if (hasTerminalTabs(ws)) {
      const ok = confirm(
        `Close workspace "${ws.name}"? All terminal sessions in it will be killed.`,
      );
      if (!ok) return;
    }
    void this.dispatch({ type: "closeWorkspace", workspace });
  }

  private async submitNew(): Promise<void> {
    if (this.submitting) return;
    const name = this.nameInput.value.trim();
    if (name.length === 0) {
      // required 속성이 빈 값은 막지만 공백뿐인 값은 통과시킨다 — 여기서 거른다.
      this.nameInput.focus();
      return;
    }
    const rootPath = this.rootPathInput.value.trim();
    const distro = this.distroInput.value.trim();
    this.submitting = true;
    try {
      const out = await this.dispatch({
        type: "createWorkspace",
        name,
        rootPath: rootPath.length === 0 ? null : rootPath,
        distro: distro.length === 0 ? null : distro,
        tab: { type: "terminal", cwd: null },
      });
      if (out === null) return; // 실패 — 상태 라인에 표면화됨, 폼 유지 (파일 상단)
      this.formEl.hidden = true;
      this.formEl.reset();
    } finally {
      this.submitting = false;
    }
  }
}
