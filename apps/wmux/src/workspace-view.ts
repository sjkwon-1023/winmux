// 활성 워크스페이스의 split tree 렌더 진입점 (11단계 청크 B).
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
// split 컨테이너 자식 구조 불변식: [first, splitter.handle, second] — syncRatios
// 의 children 0/2 접근과 buildNode 의 append 순서가 이 불변식을 공유한다.

import { PaneView } from "./pane-view";
import { Splitter } from "./splitter";
import type { DragGuard } from "./splitter";
import { flexPair, structureKey } from "./split-layout";
import type {
  Command,
  CommandOutput,
  PaneId,
  SplitId,
  SplitTree,
  StateSnapshot,
  Workspace,
} from "./types";

type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

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

  private readonly guard: DragGuard = {
    begin: (id) => this.activeDrags.add(id),
    end: (id) => this.activeDrags.delete(id),
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
  ) {}

  /** 스냅샷 반영 진입점 — store 구독에서 revision 순으로 호출된다. */
  render(snapshot: StateSnapshot): void {
    this.lastSnapshot = snapshot;
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
    this.updatePanes(ws);
  }

  /** 구조 재구축 — pane 뷰는 레지스트리 재사용, 이탈 pane 은 dispose.
   *  (수명 규칙 "alive 뷰 ⊆ 활성 워크스페이스의 탭" — 계획 D3 의 전신.) */
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
        view = new PaneView(tree.pane, this.dispatch);
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

  private updatePanes(ws: Workspace): void {
    for (const [id, view] of this.paneViews) {
      // panes 맵 키는 문자열 숫자 (JSON object 키 제약 — types.ts 참조).
      const pane = ws.panes[String(id)];
      if (pane === undefined) {
        // 코어 불변식(트리 leaf ⊆ panes 키)상 도달 불가 — 가리지 않고 노출.
        console.error("pane in layout but missing from panes map", id);
        continue;
      }
      view.update(pane, id === ws.activePane);
    }
  }

  /** 전체 해제 — 워크스페이스 부재(no workspace) 상태 전환용. */
  private clear(): void {
    for (const view of this.paneViews.values()) view.dispose();
    this.paneViews.clear();
    for (const s of this.splitters) s.dispose();
    this.splitters = [];
    this.splitContainers.clear();
    this.activeDrags.clear();
    this.lastKey = null;
    this.rootEl.replaceChildren();
  }
}
