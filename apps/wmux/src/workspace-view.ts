// 활성 워크스페이스의 split tree 렌더 진입점 (11단계 청크 B → 12단계 청크 C).
//
// 렌더 전략 (계획 1-B):
// - structureKey 가 직전과 같으면 각 split 컨테이너 자식의 flex 만 in-place
//   갱신한다. 단 드래그 활성 split(activeDrags — splitter 가 등록)은 건너뛴다:
//   드래그 중 도착하는 스냅샷이 프리뷰 ratio 를 밟지 않게 하는 D2 가드다.
// - 다르면 DOM 을 재구축하되 pane 콘텐츠 엘리먼트(PaneView.root)는 레지스트리
//   에서 재사용(reparent)해 xterm 재attach·replay 왕복을 피한다 — 현 렌더러가
//   DOM 이라 reparent 리스크가 낮다 (WebGL 활성화 시 재평가).
// - split 컨테이너는 SplitId 로 키잉한다 (경로 인덱스 금지 — 계획 D1).
//
// keep-alive 뷰 수명 (12단계 — 계획 D3·D4-b): 탭별 TerminalView 레지스트리
// (Map<TabId, TerminalView>)를 여기서 소유하고, 스냅샷마다 planViewSync 로
// dispose(탭 사라짐·워크스페이스 밖) / detachSessions(attach 안 하는 세션 전부 —
// fire-and-forget 스윕, 부트 첫 스냅샷 포함) / visible(pane 별 표시 탭)을 집행한다.
//
// focus 보상 경로 (계획 D7 — attach 자동 focus 제거의 대가): pendingFocus 1칸을
// 두고 ① 부트/리로드 첫 리컨실 후 활성 pane 의 뷰, ② main.dispatchUI 가
// requestFocus 로 넘긴 대상(TabCreated/PaneCreated 의 새 탭, ActivateTab 탭,
// FocusPane 의 pane)을 렌더 후 해소한다. 요청 즉시도 1회 시도한다 — state-changed
// 이벤트가 invoke 응답보다 먼저 처리돼 렌더가 이미 끝난 경우가 있기 때문이다.
//
// split 컨테이너 자식 구조 불변식: [first, splitter.handle, second] — syncRatios
// 의 children 0/2 접근과 buildNode 의 append 순서가 이 불변식을 공유한다.

import { detachTerminal } from "./backend";
import { PaneView } from "./pane-view";
import type { ViewRegistry } from "./pane-view";
import { Splitter } from "./splitter";
import type { DragGuard } from "./splitter";
import { flexPair, structureKey } from "./split-layout";
import type { SwitchTracer } from "./switch-trace";
import { TerminalView } from "./terminal-view";
import { planViewSync } from "./view-reconcile";
import type { VisibleView } from "./view-reconcile";
import type {
  Command,
  CommandOutput,
  PaneId,
  SessionId,
  SplitId,
  SplitTree,
  StateSnapshot,
  TabId,
  Workspace,
} from "./types";

type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** focus 보상 대상 (계획 D7). "pane" 은 그 pane 의 표시 중 뷰를 뜻한다 —
 *  FocusPane 은 cmd 에 pane 이 있어 activePane 스냅샷 도착을 기다릴 필요가 없다. */
export type FocusRequest =
  | { kind: "tab"; tab: TabId }
  | { kind: "pane"; pane: PaneId }
  /** 렌더 시점의 활성 워크스페이스 activePane — CloseTab/ClosePane 처럼 "닫힌 뒤
   *  어디가 남는지"를 스냅샷이 알려줘야 하는 보상에 쓴다. */
  | { kind: "activePane" };

/** 활성 워크스페이스 해석 — 없으면 null (main.ts 상태 라인과 공유). */
export function activeWorkspace(snapshot: StateSnapshot): Workspace | null {
  return snapshot.state.workspaces.find((w) => w.id === snapshot.state.activeWorkspace) ?? null;
}

function collectLeaves(tree: SplitTree, out: Set<PaneId>): void {
  if (tree.type === "leaf") {
    out.add(tree.pane);
    return;
  }
  collectLeaves(tree.first, out);
  collectLeaves(tree.second, out);
}

export class WorkspaceView {
  private readonly paneViews = new Map<PaneId, PaneView>();
  private readonly splitContainers = new Map<SplitId, HTMLElement>();
  private splitters: Splitter[] = [];
  private readonly activeDrags = new Set<SplitId>();
  private lastKey: string | null = null;
  private lastSnapshot: StateSnapshot | null = null;

  /** 탭별 keep-alive 터미널 뷰 — 앱 수준 레지스트리 (계획 D3). */
  private readonly views = new Map<TabId, TerminalView>();
  /** 보상 focus 보류 1칸. rendersLeft: 미해소 렌더가 이만큼 지나면 stale 로
   *  폐기한다 — invoke 응답이 앞선 무관 이벤트 렌더보다 먼저 처리되는 race 에서
   *  구 revision 렌더가 보상을 조기 폐기하지 않게 하면서(리뷰 finding), 닫힌 탭
   *  대상 요청이 영구 보류로도 남지 않게 한다. */
  private pendingFocus: { req: FocusRequest; rendersLeft: number } | null = null;
  /** detach 스윕 전이 추적 — 직전 렌더에서 이미 미부착이던 세션은 재스윕하지
   *  않는다 (멱등이지만 매 revision 반복 invoke 는 잡음 — 리뷰 finding). */
  private sweptSessions = new Set<SessionId>();
  /** 부트 보상 focus 를 첫 리컨실(워크스페이스 존재) 1회로 제한하는 래치. */
  private booted = false;

  private readonly guard: DragGuard = {
    begin: (id) => this.activeDrags.add(id),
    end: (id) => this.activeDrags.delete(id),
  };

  /** PaneView 에 내려주는 레지스트리 접근 계약 — 생성(lazy attach)·조회만 노출,
   *  dispose 는 리컨실(render)이 독점한다. */
  private readonly registry: ViewRegistry = {
    get: (tab) => this.views.get(tab),
    ensure: (tab, session, parent, onAttachError) => this.ensureView(tab, session, parent, onAttachError),
  };

  /** splitter 의 dispatch 실패·pointercancel 복원 콜백 — 최신 채택 스냅샷의
   *  ratio 를 재적용한다 (프리뷰 잔재 제거). 구조가 그새 바뀌었어도 syncRatios
   *  는 현존 컨테이너만 만지므로 안전하다. */
  private readonly restoreRatios = (): void => {
    const snapshot = this.lastSnapshot;
    if (snapshot === null) return;
    const ws = activeWorkspace(snapshot);
    if (ws !== null) this.syncRatios(ws.layout);
  };

  constructor(
    private readonly rootEl: HTMLElement,
    private readonly dispatch: DispatchFn,
    /** 전환 지연 tracer (14단계) — ensureView 의 새 attach 를 계측 대상으로
     *  등록한다. trace 미진행 시 markAttachStart 가 즉시 false 라 오버헤드 없음. */
    private readonly tracer: SwitchTracer,
  ) {}

  /** 스냅샷 반영 진입점 — store 구독에서 revision 순으로 호출된다. */
  render(snapshot: StateSnapshot): void {
    this.lastSnapshot = snapshot;

    // keep-alive 리컨실 — 구조 렌더보다 먼저: dispose 로 뷰가 정리된 뒤에
    // updatePanes 가 가시성·lazy attach 를 만진다 (계획 D3·D4-b).
    const plan = planViewSync(this.views.keys(), snapshot);
    for (const tab of plan.dispose) {
      // 한 뷰의 dispose 이상이 나머지 정리·렌더 전체를 중단시키지 않게 격리.
      try {
        this.views.get(tab)?.dispose();
      } catch (err) {
        console.error("view dispose failed", tab, err);
      }
      this.views.delete(tab);
    }
    const unattached = new Set(plan.detachSessions);
    for (const session of plan.detachSessions) {
      // fire-and-forget 스윕 (멱등) — 부트 첫 스냅샷 포함. F5 후 미방문 탭
      // 세션의 죽은 채널이 paused 에 고착되는 것을 여기서 치운다 (D4-b).
      // 직전 렌더에서 이미 미부착이던 세션은 건너뛴다 — 전이 시점 1회면 충분
      // (매 revision 반복 invoke 잡음 방지, 리뷰 finding).
      if (this.sweptSessions.has(session)) continue;
      void detachTerminal(session).catch((err) =>
        console.error("detach sweep failed", session, err),
      );
    }
    this.sweptSessions = unattached;

    const ws = activeWorkspace(snapshot);
    if (ws === null) {
      this.clear();
      this.rootEl.textContent = "(no workspace)";
      return;
    }

    const key = structureKey(ws.layout);
    if (key !== this.lastKey) {
      this.rebuild(ws.layout);
      this.lastKey = key;
    } else {
      this.syncRatios(ws.layout);
    }
    this.updatePanes(ws, plan.visible);

    // 부트/리로드 보상 focus (D7) — 첫 리컨실 후 활성 pane 의 뷰 1곳.
    if (!this.booted) {
      this.booted = true;
      if (this.pendingFocus === null) {
        this.pendingFocus = { req: { kind: "pane", pane: ws.activePane }, rendersLeft: 1 };
      }
    }
    this.tryResolveFocus(true);
  }

  /** focus 보상 요청 (main.dispatchUI 성공 경로 — 계획 D7). 즉시 1회 시도하고,
   *  대상이 아직 없으면(스냅샷 미도착) 다음 render 가 해소한다. */
  requestFocus(req: FocusRequest): void {
    // rendersLeft 3: 명령 결과 스냅샷보다 앞선 무관 이벤트 렌더가 1~2개 끼어도
    // 보상이 살아남고, 정말 stale 한 요청(대상 탭이 닫힘)은 몇 렌더 안에 폐기된다.
    this.pendingFocus = { req, rendersLeft: 3 };
    // activePane 류는 즉시 해소하지 않는다 (리뷰 finding) — invoke 응답이 명령의
    // 스냅샷보다 먼저 처리되는 순서에서는 lastSnapshot 이 아직 명령 이전 상태라,
    // 전환 전 워크스페이스의 activePane 을 focus 하고 보상을 소진해 버린다.
    // 대상이 명시된 tab/pane 류만 즉시 시도한다 (그 대상은 stale 스냅샷에서도
    // 동일 객체다).
    if (req.kind !== "activePane") this.tryResolveFocus(false);
  }

  /** pendingFocus 해소 시도. atRender 면 미해소마다 rendersLeft 를 줄이고 0 이
   *  되면 폐기한다 (stale 요청이 보류로 영구히 남지 않게). */
  private tryResolveFocus(atRender: boolean): void {
    const pending = this.pendingFocus;
    if (pending === null) return;
    const view = this.focusTarget(pending.req);
    if (view !== null) {
      view.focus();
      this.pendingFocus = null;
      return;
    }
    if (atRender && --pending.rendersLeft <= 0) this.pendingFocus = null;
  }

  /** 요청 → focus 할 뷰. 숨은 뷰는 대상이 아니다 (display:none 은 focus 불가) —
   *  표시 여부는 pane 의 shownTab 으로 판정한다. */
  private focusTarget(req: FocusRequest): TerminalView | null {
    if (req.kind === "activePane") {
      const ws = this.lastSnapshot === null ? null : activeWorkspace(this.lastSnapshot);
      if (ws === null) return null;
      return this.focusTarget({ kind: "pane", pane: ws.activePane });
    }
    if (req.kind === "pane") {
      const paneView = this.paneViews.get(req.pane);
      const shown = paneView?.shownTab ?? null;
      return shown === null ? null : this.views.get(shown) ?? null;
    }
    const view = this.views.get(req.tab);
    if (view === undefined) return null;
    for (const paneView of this.paneViews.values()) {
      if (paneView.shownTab === req.tab) return view;
    }
    return null;
  }

  /** 레지스트리 ensure 구현 — 없으면 생성 + lazy attach. attach 실패는 가리지
   *  않는다: 뷰를 정리하고 onAttachError 로 pane 에 노출한다 (다음 스냅샷 렌더가
   *  자연 재시도한다 — revision 변화 때만이라 폭주하지 않는다). */
  private ensureView(
    tab: TabId,
    session: SessionId,
    parent: HTMLElement,
    onAttachError: (message: string) => void,
  ): TerminalView {
    let view = this.views.get(tab);
    if (view !== undefined) return view;
    // 전환 계측 (14단계): 진행 중 trace 가 이 탭의 새 attach 를 수락한 경우에만
    // replay 완료 훅을 단다 — keep-alive 재사용(위 early return)은 replay 왕복이
    // 없어 계측 대상이 아니다. attach 가 실패하면 trace 는 완주하지 못하고 다음
    // begin 이 폐기한다 (실패 자체는 아래 catch 가 별도로 노출한다).
    let onTraceReplayDone: ((bytes: number) => void) | undefined;
    if (this.tracer.markAttachStart(tab, performance.now())) {
      onTraceReplayDone = (bytes) => this.tracer.markReplayDone(tab, bytes, performance.now());
    }
    const created = new TerminalView(parent, session, onTraceReplayDone);
    this.views.set(tab, created);
    created.attach().catch((err) => {
      console.error("attach_terminal failed", err);
      if (this.views.get(tab) !== created) return; // 이미 리컨실로 정리됨
      created.dispose();
      this.views.delete(tab);
      onAttachError(`attach failed (session ${session}): ${String(err)}`);
    });
    return created;
  }

  /** 구조 재구축 — pane 뷰는 레지스트리 재사용, 이탈 pane 은 dispose.
   *  (터미널 뷰 수명은 planViewSync 소유 — pane dispose 는 DOM·observer 만.) */
  private rebuild(tree: SplitTree): void {
    const keep = new Set<PaneId>();
    collectLeaves(tree, keep);
    for (const [id, view] of this.paneViews) {
      if (!keep.has(id)) {
        view.dispose();
        this.paneViews.delete(id);
      }
    }
    for (const s of this.splitters) s.dispose();
    this.splitters = [];
    this.splitContainers.clear();

    const built = this.buildNode(tree);
    // 루트는 #view(flex)를 꽉 채운다 — 중첩에서 승격된 엘리먼트의 stale
    // inline flex 를 확정값으로 덮는다.
    built.style.flex = "1 1 0px";
    this.rootEl.replaceChildren(built);
  }

  private buildNode(tree: SplitTree): HTMLElement {
    if (tree.type === "leaf") {
      let view = this.paneViews.get(tree.pane);
      if (view === undefined) {
        view = new PaneView(tree.pane, this.dispatch, this.registry);
        this.paneViews.set(tree.pane, view);
      }
      return view.root; // 기존 뷰는 append 시 자동 reparent 된다
    }

    const container = document.createElement("div");
    container.className = `split split-${tree.direction}`;
    const first = this.buildNode(tree.first);
    const second = this.buildNode(tree.second);
    const splitter = new Splitter({
      container,
      splitId: tree.id,
      direction: tree.direction,
      first,
      second,
      guard: this.guard,
      dispatch: this.dispatch,
      restore: this.restoreRatios,
    });
    this.applyRatio(first, second, tree.ratio);
    container.append(first, splitter.handle, second); // 자식 구조 불변식 (파일 상단)
    this.splitContainers.set(tree.id, container);
    this.splitters.push(splitter);
    return container;
  }

  /** ratio in-place 갱신 — 드래그 활성 split 은 건너뛴다 (D2 가드). */
  private syncRatios(tree: SplitTree): void {
    if (tree.type === "leaf") return;
    if (!this.activeDrags.has(tree.id)) {
      const container = this.splitContainers.get(tree.id);
      if (container !== undefined) {
        const first = container.children.item(0);
        const second = container.children.item(2);
        if (first instanceof HTMLElement && second instanceof HTMLElement) {
          this.applyRatio(first, second, tree.ratio);
        }
      }
    }
    this.syncRatios(tree.first);
    this.syncRatios(tree.second);
  }

  private applyRatio(first: HTMLElement, second: HTMLElement, ratio: number): void {
    const pair = flexPair(ratio);
    // basis 0 고정 — 핸들(4px)을 제외한 공간이 grow 비율로만 나뉜다.
    first.style.flex = `${pair.first} 1 0px`;
    second.style.flex = `${pair.second} 1 0px`;
  }

  private updatePanes(ws: Workspace, visible: VisibleView[]): void {
    const visibleByPane = new Map<PaneId, VisibleView>();
    for (const entry of visible) visibleByPane.set(entry.pane, entry);
    for (const [id, view] of this.paneViews) {
      // panes 맵 키는 문자열 숫자 (JSON object 키 제약 — types.ts 참조).
      const pane = ws.panes[String(id)];
      if (pane === undefined) {
        // 코어 불변식(트리 leaf ⊆ panes 키)상 도달 불가 — 가리지 않고 노출.
        console.error("pane in layout but missing from panes map", id);
        continue;
      }
      view.update(pane, id === ws.activePane, visibleByPane.get(id) ?? null);
    }
  }

  /** 전체 해제 — 워크스페이스 부재(no workspace) 상태 전환용. 터미널 뷰
   *  레지스트리는 여기 오기 전에 이미 비어 있다 — render 의 리컨실이 먼저 돌고,
   *  활성 워크스페이스가 없으면 alive 탭 전부가 dispose 목록에 떨어지기 때문. */
  private clear(): void {
    for (const view of this.paneViews.values()) view.dispose();
    this.paneViews.clear();
    for (const s of this.splitters) s.dispose();
    this.splitters = [];
    this.splitContainers.clear();
    this.activeDrags.clear();
    // pendingFocus 는 유지한다 (리뷰 finding) — 명령 응답과 그 스냅샷 사이에
    // ws-null 렌더가 끼는 경로(마지막 워크스페이스 닫기 직후 재생성 등)에서
    // 보상이 살아남아야 한다. stale 요청은 rendersLeft 가 정리한다.
    this.lastKey = null;
    this.rootEl.replaceChildren();
  }
}
