// 워크스페이스 사이드바 (13단계 D3·D4) — 카드 리스트 + 하단 "+ New workspace"
// 인라인 폼.
//
// 렌더 전략 (18단계 B-6 — 카드 id 키잉 reconcile): reconcilePlan 의 판정대로
// skip(무변경, DOM 무접촉) / patch(카드 노드 유지 + 텍스트·클래스만 갱신) /
// rebuild(멤버십·순서 변화 → 재조립) 셋으로 갈린다.
//
// 왜 전체 재조립이면 안 되나: 클릭 진행 중(mousedown~click 사이) 렌더가 눌린 카드
// 엘리먼트를 갈아치우면 브라우저가 click 을 발화하지 않아 클릭이 유실된다
// (ADR-0003 결정 7 의 탭바 스왈로와 같은 결함). 13단계의 "모델 직렬화 키가 같으면
// 스킵" 가드는 agentStatus 가 idle 고정·message 가 null 이던 시절에만 성립했고,
// 18단계에서 status·message·집계 unread 가 OSC 마다 변하는 동적 필드가 되면서
// 무관 알림 하나로도 뚫린다 — 그래서 스킵 가드는 skip 판정으로만 남기고, 값이
// 변한 경우의 기본 경로를 in-place 패치로 바꾼다.
//
// 하단 폼은 카드 리스트와 분리된 고정 DOM 이라 어느 판정에서도 재조립 대상이
// 아니다 — 입력 중 스냅샷이 와도 폼 상태가 날아가지 않는다.
//
// 폼 입력은 표준 DOM <input> 이다 — 키 가로채기를 하지 않는다. 폼에 포커스가
// 있는 동안 키 입력은 xterm 포커스 밖이라 터미널로 새지 않는다. 전역 캡처
// (keys.ts 가로채기 목록 + main.ts 의 Ctrl+Shift+R 리로드)는 폼 포커스 중에도
// 그대로 동작하지만 전부 modifier 조합이라 이름·경로 타이핑과 충돌하지 않는다.
//
// 폼 열기는 하단 버튼 클릭 외에 Ctrl+Shift+N 으로도 들어온다 — main.ts 글루가
// focusNewWorkspace() 를 부른다 (dispatch 가 아닌 유일한 키 액션).
//
// 상호작용 (계획 D4):
// - 카드 클릭 = SwitchWorkspace (이미 활성이면 no-op 스킵 — 무변경 revision 잡음 방지).
// - × = CloseWorkspace. 실행 중인 터미널 세션이 1개라도 있으면 confirm() 을
//   거친다 — 그 세션들을 죽이는 파괴적 동작이다. 판정은 렌더 캐시가 아니라 클릭
//   시점의 최신 스냅샷으로 한다 (카드 DOM 이 스킵으로 오래됐을 수 있다).
// - 제출 = CreateWorkspace { tab: terminal } 원자 생성 (계획 13-D1) — 빈
//   워크스페이스 프레임 없이 터미널까지 한 번에 뜬다. 성공 시 폼을 닫고,
//   실패는 dispatchUI 가 상태 라인에 표면화하므로 폼을 유지해 재시도하게 한다.

import { shortcutLabel } from "./keys";
import { hasRunningTerminals, reconcilePlan, sameCard, sidebarModel } from "./sidebar-model";
import type { WorkspaceCardModel } from "./sidebar-model";
import type { Command, CommandOutput, StateSnapshot, WorkspaceId } from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼 (실패는 null, reject 없음). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** 카드 1개의 DOM 노드 묶음 — in-place 패치 대상. model 은 이 카드가 지금 그리고
 *  있는 모델로, 클릭 핸들러가 stale 클로저 대신 여기서 최신 값을 읽는다. */
interface CardNodes {
  root: HTMLElement;
  status: HTMLSpanElement;
  name: HTMLSpanElement;
  dot: HTMLSpanElement;
  message: HTMLDivElement;
  meta: HTMLDivElement;
  model: WorkspaceCardModel;
}

/** 값이 같으면 쓰지 않는 텍스트 대입 — textContent 재대입은 값이 같아도 자식
 *  텍스트 노드를 갈아치우므로, 무변경 렌더가 DOM 을 흔들지 않게 한다. */
function setText(el: HTMLElement, text: string): void {
  if (el.textContent !== text) el.textContent = text;
}

/** 메타 1줄 — branch · path · counts (null 필드는 생략, 말줄임은 CSS). */
function metaText(model: WorkspaceCardModel): string {
  const counts = `${model.counts.panes}p ${model.counts.tabs}t`;
  return [model.branch, model.path, counts].filter((p): p is string => p !== null).join(" · ");
}

export class Sidebar {
  private readonly cardsEl: HTMLDivElement;
  private readonly formEl: HTMLFormElement;
  private readonly nameInput: HTMLInputElement;
  private readonly rootPathInput: HTMLInputElement;
  private readonly distroInput: HTMLInputElement;
  private lastSnapshot: StateSnapshot | null = null;
  /** 직전 렌더의 카드 모델 (첫 렌더 전 null) — reconcilePlan 의 좌변 (파일 상단). */
  private lastCards: WorkspaceCardModel[] | null = null;
  /** 현재 화면에 붙어 있는 카드 노드 — workspace id 키잉, patch 판정의 대상. */
  private readonly cardNodes = new Map<WorkspaceId, CardNodes>();
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
    // 단축키 표기는 keys.ts 의 shortcutLabel 단일 소스에서 받는다 (표류 방지).
    newBtn.title = `New workspace (${shortcutLabel("newWorkspace")})`;

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
    const prev = this.lastCards;
    const plan = reconcilePlan(prev, model);
    if (plan === "skip") return;
    if (plan === "rebuild") {
      this.cardNodes.clear();
      const nodes = model.map((m) => this.card(m));
      for (const n of nodes) this.cardNodes.set(n.model.workspace, n);
      this.cardsEl.replaceChildren(...nodes.map((n) => n.root));
    } else {
      // 멤버십·순서가 같음이 판정으로 보장된다 — 변한 카드만 in-place 갱신.
      model.forEach((next, i) => {
        const before = prev?.[i];
        if (before !== undefined && sameCard(before, next)) return;
        const nodes = this.cardNodes.get(next.workspace);
        if (nodes !== undefined) this.applyCard(nodes, next);
      });
    }
    this.lastCards = model;
  }

  /** 새 워크스페이스 폼 열기 + name 입력 포커스 (Ctrl+Shift+N — main.ts 글루가
   *  부른다). 버튼 클릭이 토글인 것과 달리 여기는 항상 여는 방향이다: 단축키를
   *  다시 눌러 폼이 닫히면 "이름을 치려다 폼이 사라지는" 동작이 된다. */
  focusNewWorkspace(): void {
    this.formEl.hidden = false;
    this.nameInput.focus();
    // 이미 열려 있고 값이 남아 있는 경우를 위해 선택까지 해 둔다 — 바로 덮어쓸
    // 수 있다 (폼은 성공 제출 때만 reset 되므로 실패한 입력이 남아 있을 수 있다).
    this.nameInput.select();
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

  /** 카드 DOM 조립 — 값에 따라 있다 없다 하는 행(미리보기·집계 dot)도 노드는
   *  항상 만들고 hidden 으로만 토글한다. 노드 존재 자체가 변하면 그 카드의
   *  자식이 갈아치워져 in-place 패치의 의미가 없어지기 때문이다. */
  private card(model: WorkspaceCardModel): CardNodes {
    const el = document.createElement("div");
    el.className = "ws-card";

    const head = document.createElement("div");
    head.className = "ws-card-head";

    const status = document.createElement("span");
    status.className = "ws-card-status";

    const name = document.createElement("span");
    name.className = "ws-card-name";

    // 집계 unread dot — 워크스페이스 안 어느 탭이든 미확인 알림이 있으면 표시.
    const dot = document.createElement("span");
    dot.className = "ws-card-dot";
    dot.textContent = "●";
    dot.title = "Unread notification";

    const close = document.createElement("button");
    close.type = "button";
    close.className = "ws-card-close";
    close.textContent = "×";
    close.title = "Close workspace";
    close.addEventListener("click", (ev) => {
      ev.stopPropagation(); // 카드 클릭(전환)과 분리
      this.onClose(model.workspace);
    });

    head.append(status, name, dot, close);

    const message = document.createElement("div");
    message.className = "ws-card-message";

    const meta = document.createElement("div");
    meta.className = "ws-card-meta";

    el.append(head, message, meta);

    const nodes: CardNodes = { root: el, status, name, dot, message, meta, model };
    this.applyCard(nodes, model);

    el.addEventListener("click", () => {
      // 이미 활성이면 no-op 스킵 (계획 D4). in-place 패치로 카드가 살아남는 동안
      // 모델은 바뀌므로 클로저가 아니라 nodes.model 에서 최신 값을 읽는다.
      if (!nodes.model.active) {
        void this.dispatch({ type: "switchWorkspace", workspace: nodes.model.workspace });
      }
    });
    return nodes;
  }

  /** 카드 모델을 기존 노드에 반영 — 조립 직후와 in-place 패치가 같은 경로를 탄다. */
  private applyCard(nodes: CardNodes, model: WorkspaceCardModel): void {
    nodes.model = model;
    nodes.root.classList.toggle("active", model.active);

    setText(nodes.status, model.statusIcon);
    nodes.status.title = model.status;

    setText(nodes.name, model.name);
    nodes.name.title = model.name; // 잘린 이름의 툴팁

    nodes.dot.hidden = !model.unread;

    setText(nodes.message, model.message ?? "");
    nodes.message.title = model.message ?? "";
    nodes.message.hidden = model.message === null;

    const meta = metaText(model);
    setText(nodes.meta, meta);
    nodes.meta.title = meta;
  }

  /** × 클릭 — 터미널 탭이 있으면 confirm 후 CloseWorkspace (계획 D4). */
  private onClose(workspace: WorkspaceId): void {
    const ws =
      this.lastSnapshot?.state.workspaces.find((w) => w.id === workspace) ?? null;
    if (ws === null) return; // 이미 닫힌 카드의 늦은 클릭 — 보낼 것이 없다
    if (hasRunningTerminals(ws)) {
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
