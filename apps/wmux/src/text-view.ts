// textViewer 탭의 뷰 (21단계 청크 C2) — 파일을 512KiB **바이트 윈도우** 1개만
// 메모리에 두고, 그 안에서 고정 행높이 가상 스크롤로 그린다.
//
// 이 파일의 두 계약이 나머지를 지배한다.
//
// 1. **메모리 상주 = 윈도우 1개 고정** (계획 v2 "탭 타입별 동작"): 수백 MB 로그를
//    열어도 상주량이 창 하나를 넘지 않아야 하므로 스크롤 끝에서 자동으로 이어
//    읽지 않는다. 파일이 창보다 크면 상단 바의 first/prev/next/last 버튼으로
//    **명시적으로** 창을 옮긴다. 창 경계는 행 중간을 가르므로 선두(창 시작이
//    파일 중간일 때)·말미(EOF 가 아닐 때) 부분행과 UTF-8 파단 바이트를 잘라낸다.
// 2. **스크롤 왕복의 에코 가드** (계획 21단계 high 리스크): 스크롤이 멈추면
//    500ms 디바운스로 setViewerScroll(최상단 가시 행의 전역 byte offset)을
//    dispatch 하는데, 그 dispatch 가 다시 스냅샷 → update() 로 돌아온다. 스냅샷의
//    scrollTop 을 매번 적용하면 재렌더가 사용자 스크롤과 싸우므로 **마운트 시
//    1회(또는 경로 변경 시)만** 적용한다 — 판정은 순수 함수 shouldAdoptScroll.
//
// 윈도우 절삭(decodeWindow)·슬라이스 계산(visibleSlice)·에코 가드·디바운스
// (ScrollSettle)는 DOM·IPC 무의존으로 분리해 vitest 로 잠근다. 클래스는 그
// 결과를 DOM 에 옮기는 얇은 층이다.
//
// 리사이즈는 **자체 ResizeObserver** 로 받는다 (계획 프론트 계약): pane 의
// observer 는 터미널 fit 전용이라 뷰어까지 겸하게 만들지 않는다.

import type { TimerHost } from "./ack-batcher";
import { fsReadChunk, fsStat } from "./backend";
import type { ViewerKind, ViewerView } from "./viewer-view";
import type { Command, CommandOutput, TabId } from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼 (실패는 상태 라인에 표면화되고 null). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** 한 번에 읽어 상주시키는 바이트 수. 글루 상한(4MiB)보다 한참 아래다. */
export const WINDOW_BYTES = 512 * 1024;
/** 고정 행높이(px). 가상 스크롤의 spacer 높이·슬라이스 계산이 전부 이 값 기준
 *  이라 CSS 가 아니라 여기가 정본이다 (뷰가 `--text-line-height` 로 내려준다). */
export const LINE_HEIGHT_PX = 16;
/** viewport 위아래로 더 그리는 행 수 — 스크롤 중 빈 줄이 스치지 않게. */
export const OVERSCAN_LINES = 20;
/** 스크롤이 멎었다고 보는 시간 — 이 시간 뒤에 위치 1개만 dispatch 한다. */
export const SCROLL_SETTLE_MS = 500;

const LF = 0x0a;
const CR = 0x0d;

const defaultTimers: TimerHost = {
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as number),
};

/** 절삭까지 끝난 윈도우 1개. offset 은 전부 **파일 전역** byte offset 이다 —
 *  모델의 scrollTop 시맨틱(최상단 가시 행의 전역 byte offset)과 같은 좌표계라야
 *  창을 옮겨도 위치가 보존된다. */
export interface TextWindow {
  /** 유지 구간 첫 바이트의 전역 offset. */
  start: number;
  /** 유지 구간 마지막 바이트 **다음**의 전역 offset. */
  end: number;
  /** 표시용 행 (행말 CR 제거, 개행 미포함). */
  lines: string[];
  /** lines[i] 의 시작 전역 byte offset — lines 와 길이가 같다. */
  lineStarts: number[];
}

/** UTF-8 continuation 바이트(0b10xxxxxx)인가. */
function isContinuation(byte: number): boolean {
  return (byte & 0xc0) === 0x80;
}

/** lead 바이트가 예고하는 시퀀스 길이. continuation·불량 바이트는 0. */
function sequenceLength(byte: number): number {
  if (byte < 0x80) return 1;
  if ((byte & 0xe0) === 0xc0) return 2;
  if ((byte & 0xf0) === 0xe0) return 3;
  if ((byte & 0xf8) === 0xf0) return 4;
  return 0;
}

/** 행 1개 디코드 — 행말 CR 은 표시에서 뺀다 (CRLF 파일이 매 행 끝에 제어문자를
 *  달고 나오지 않게). byte offset 계산에는 영향이 없다. */
function decodeLine(decoder: TextDecoder, bytes: Uint8Array, from: number, to: number): string {
  const end = to > from && bytes[to - 1] === CR ? to - 1 : to;
  return decoder.decode(bytes.subarray(from, end));
}

/** 읽어 온 바이트 창 → 완결된 행들 (순수).
 *
 *  - `readOffset > 0` 이면 선두는 이전 창에 걸친 **부분행**이므로 첫 개행 다음
 *    까지 버린다.
 *  - `atEof` 가 아니면 말미도 다음 창으로 이어지는 부분행이므로 마지막 개행
 *    까지만 남긴다.
 *  - 남은 구간의 양 끝에서 잘린 멀티바이트 시퀀스를 제거한다 — 개행 경계로
 *    잘린 경우엔 애초에 생기지 않지만, **창보다 긴 행**(창 안에 개행이 하나도
 *    없는 경우)에서는 부분행 절삭을 포기하므로 여기서만 걸린다.
 *
 *  창 안에 개행이 없으면 절삭 대신 조각을 그대로 보여준다 — 빈 화면(모든 바이트
 *  절삭)보다 "이 창은 한 행의 일부"가 낫다. */
export function decodeWindow(bytes: Uint8Array, readOffset: number, atEof: boolean): TextWindow {
  let from = 0;
  let to = bytes.length;

  if (readOffset > 0) {
    const nl = bytes.indexOf(LF);
    if (nl >= 0) from = nl + 1;
  }
  if (!atEof) {
    const nl = bytes.lastIndexOf(LF);
    if (nl >= from) to = nl + 1;
  }

  // 선두: 파일의 진짜 시작이 아니면 앞에 남은 continuation 바이트를 버린다.
  if (readOffset + from > 0) {
    while (from < to && isContinuation(bytes[from])) from += 1;
  }
  // 말미: 마지막 lead 바이트의 시퀀스가 창을 넘어가면 그 lead 부터 버린다
  // (시퀀스 최대 4바이트라 뒤에서 4칸만 본다).
  if (!atEof) {
    for (let i = to - 1; i >= from && i >= to - 4; i -= 1) {
      const length = sequenceLength(bytes[i]);
      if (length === 0) continue; // continuation — 더 앞의 lead 를 찾는다
      if (i + length > to) to = i;
      break;
    }
  }

  const decoder = new TextDecoder();
  const lines: string[] = [];
  const lineStarts: number[] = [];
  let lineFrom = from;
  for (let i = from; i < to; i += 1) {
    if (bytes[i] !== LF) continue;
    lineStarts.push(readOffset + lineFrom);
    lines.push(decodeLine(decoder, bytes, lineFrom, i));
    lineFrom = i + 1;
  }
  // 개행으로 끝나면 그 뒤에 행이 하나 더 있는 것이 아니다 (빈 꼬리 행 금지).
  if (lineFrom < to) {
    lineStarts.push(readOffset + lineFrom);
    lines.push(decodeLine(decoder, bytes, lineFrom, to));
  }

  return { start: readOffset + from, end: readOffset + to, lines, lineStarts };
}

/** 이번에 실제로 그릴 행 구간 + 그 블록의 top offset(px). */
export interface SliceRange {
  first: number;
  /** exclusive. */
  last: number;
  top: number;
}

/** 스크롤 위치 → 렌더할 행 구간 (순수). viewport 에 걸리는 행 앞뒤로 overscan
 *  만큼 더 그린다. 전체 높이는 spacer 가 잡으므로 여기서는 구간만 정한다. */
export function visibleSlice(
  scrollTop: number,
  viewportHeight: number,
  totalLines: number,
  lineHeight: number = LINE_HEIGHT_PX,
  overscan: number = OVERSCAN_LINES,
): SliceRange {
  if (totalLines <= 0 || lineHeight <= 0) return { first: 0, last: 0, top: 0 };
  const top = Math.max(0, scrollTop);
  const firstVisible = Math.min(totalLines - 1, Math.floor(top / lineHeight));
  // +1: 위아래로 반 줄씩 걸치는 경우를 덮는다.
  const visibleCount = Math.ceil(Math.max(0, viewportHeight) / lineHeight) + 1;
  const first = Math.max(0, firstVisible - overscan);
  const last = Math.min(totalLines, firstVisible + visibleCount + overscan);
  return { first, last, top: first * lineHeight };
}

/** 전역 byte offset → 그 offset 을 담는 행 인덱스 (순수, 이분 탐색).
 *  창 앞이면 0, 창 뒤면 마지막 행 — 저장된 위치가 창 밖이어도 클램프한다. */
export function lineIndexForOffset(lineStarts: readonly number[], offset: number): number {
  if (lineStarts.length === 0) return 0;
  if (offset <= lineStarts[0]) return 0;
  let lo = 0;
  let hi = lineStarts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (lineStarts[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** 스냅샷의 scrollTop 을 지금 화면에 적용해야 하는가 (에코 가드 — 파일 상단 2).
 *
 *  `current` 는 이미 적용한 문서(null 이면 아직 마운트 전)다. 같은 파일에 대한
 *  갱신은 **전부 거절**한다: 우리가 보낸 setViewerScroll 이 스냅샷으로 되돌아와
 *  사용자가 그새 옮긴 스크롤을 덮는 것을 막는 것이 이 함수의 존재 이유다. 경로가
 *  바뀌면 다른 문서이므로 그 문서의 저장 위치를 1회 적용한다 (탭별 뷰라 실제로
 *  발생하지 않지만, update 계약상 방어한다). */
export function shouldAdoptScroll(current: { path: string } | null, nextPath: string): boolean {
  return current === null || current.path !== nextPath;
}

/** 윈도우 이동 버튼 (자동 이어읽기 없음 — 파일 상단 1). */
export type WindowAction = "first" | "prev" | "next" | "last";

/** 버튼 → 다음 창의 목표 시작 offset (순수).
 *
 *  `next` 는 현재 창의 **유지 구간 끝**(행 경계)에서 이어 붙어 바이트가 빠지지
 *  않는다. 마지막 창보다 뒤로는 가지 않는다. */
export function nextWindowStart(
  action: WindowAction,
  current: { start: number; end: number },
  size: number,
  windowBytes: number = WINDOW_BYTES,
): number {
  const lastStart = Math.max(0, size - windowBytes);
  switch (action) {
    case "first":
      return 0;
    case "prev":
      return Math.max(0, current.start - windowBytes);
    case "next":
      return Math.min(lastStart, current.end);
    case "last":
      return lastStart;
  }
}

/** 세 자리 구분 (순수 — 로케일에 기대지 않는다: WebView ICU 에 따라 결과가
 *  달라지면 테스트로 고정할 수 없다). */
function groupDigits(value: number): string {
  return String(Math.trunc(value)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** 상단 바의 byte 범위 표시 — 파일이 창보다 클 때만 뜬다. */
export function formatByteRange(start: number, end: number, size: number): string {
  return `bytes ${groupDigits(start)}–${groupDigits(end)} of ${groupDigits(size)}`;
}

/** 스크롤 settle 디바운스 (파일 상단 2의 절반) — 스크롤 이벤트마다 dispatch 하지
 *  않고, 마지막 이벤트로부터 settleMs 동안 조용하면 그때 위치 1개만 내보낸다.
 *  타이머를 주입할 수 있어 fake timer 로 결정적 테스트가 가능하다 (AckBatcher
 *  전례). DOM 무의존이라 뷰와 별개로 vitest 대상이다. */
export class ScrollSettle {
  /** 아직 보내지 않은 위치 (없으면 null). */
  private pending: number | null = null;
  private handle: unknown = null;
  /** 모델이 이미 갖고 있다고 믿는 위치 — 같은 값은 보내지 않는다. */
  private synced: number | null = null;

  constructor(
    private readonly send: (offset: number) => void,
    private readonly settleMs: number = SCROLL_SETTLE_MS,
    private readonly timers: TimerHost = defaultTimers,
  ) {}

  /** 모델과 합의된 위치를 새로 고정한다 (마운트 복원·창 이동 직후). 보류분은
   *  버린다 — 복원 때문에 발화한 scroll 이벤트를 되돌려 보내지 않기 위해서다. */
  markSynced(offset: number | null): void {
    this.clearTimer();
    this.pending = null;
    this.synced = offset;
  }

  /** 스크롤 이벤트 1회. 마지막 호출 기준으로 settleMs 뒤 send 된다. */
  observe(offset: number): void {
    this.clearTimer();
    if (offset === this.synced) {
      // 이미 모델에 있는 위치로 되돌아왔다 — 보낼 것이 없다.
      this.pending = null;
      return;
    }
    this.pending = offset;
    this.handle = this.timers.setTimeout(() => {
      this.handle = null;
      this.flush();
    }, this.settleMs);
  }

  /** 보류분 즉시 배출 (unmount 직전 flushScroll). 보류가 없으면 no-op. */
  flush(): void {
    this.clearTimer();
    const offset = this.pending;
    this.pending = null;
    if (offset === null || offset === this.synced) return;
    this.synced = offset;
    this.send(offset);
  }

  /** 타이머만 정리한다 — dispose 는 배출하지 않는다 (배출은 flush 의 몫). */
  dispose(): void {
    this.clearTimer();
    this.pending = null;
  }

  private clearTimer(): void {
    if (this.handle === null) return;
    this.timers.clearTimeout(this.handle);
    this.handle = null;
  }
}

/** 로드 실패 payload 의 표시 문자열 — 글루는 `Result<_, String>` 이라 문자열이
 *  오지만, IPC 레벨 실패 등 계약 밖 값도 삼키지 않는다. */
function describeError(err: unknown): string {
  return typeof err === "string" ? err : String(err);
}

export interface TextViewOptions {
  /** 타이머 구현 (테스트 주입). 기본은 전역 setTimeout/clearTimeout. */
  timers?: TimerHost;
  /** 스크롤 settle 디바운스(ms). */
  settleMs?: number;
}

export class TextView implements ViewerView {
  readonly root: HTMLDivElement;
  private readonly bannerEl: HTMLDivElement;
  private readonly barEl: HTMLDivElement;
  private readonly rangeEl: HTMLSpanElement;
  private readonly buttons: { action: WindowAction; el: HTMLButtonElement }[] = [];
  private readonly scrollEl: HTMLDivElement;
  private readonly spacerEl: HTMLDivElement;
  private readonly linesEl: HTMLDivElement;
  private readonly resizeObserver: ResizeObserver;
  private readonly settle: ScrollSettle;

  private path: string;
  private size = 0;
  private win: TextWindow = { start: 0, end: 0, lines: [], lineStarts: [] };
  private disposed = false;
  /** in-flight 로드 토큰 — 늦게 도착한 이전 창의 응답이 현재 창을 덮지 않게 한다
   *  (창 이동 버튼을 연타하면 순서가 뒤집힐 수 있다). */
  private loadToken = 0;
  /** 지금 DOM 에 그려져 있는 슬라이스 (같으면 다시 만들지 않는다). */
  private slice: { first: number; last: number } | null = null;
  /** 에코 가드 상태 — shouldAdoptScroll 의 좌변. */
  private adopted: { path: string } | null = null;
  /** 로드가 끝나면 적용할 전역 byte offset (마운트·경로 변경 시 1회). */
  private pendingOffset: number | null = null;

  constructor(
    parent: HTMLElement,
    private readonly tab: TabId,
    /** 워크스페이스 distro (없으면 null — 글루가 WMUX_DISTRO·기본 배포판 순으로
     *  해석한다). 워크스페이스 생성 후 바뀌지 않는 값이라 생성 시 고정한다. */
    private readonly distro: string | null,
    kind: ViewerKind,
    dispatch: DispatchFn,
    options: TextViewOptions = {},
  ) {
    this.path = kind.type === "textViewer" ? kind.path : "";
    this.adopted = { path: this.path };
    this.pendingOffset = kind.type === "textViewer" ? kind.scrollTop : 0;

    this.settle = new ScrollSettle(
      (offset) => {
        void dispatch({ type: "setViewerScroll", tab: this.tab, scrollTop: offset });
      },
      options.settleMs ?? SCROLL_SETTLE_MS,
      options.timers ?? defaultTimers,
    );

    this.root = document.createElement("div");
    this.root.className = "text-view";
    // 행높이의 정본은 TS 상수다 (LINE_HEIGHT_PX) — CSS 가 다른 값을 쓰면 spacer
    // 높이와 슬라이스 위치가 어긋나므로 커스텀 속성으로 내려준다.
    this.root.style.setProperty("--text-line-height", `${LINE_HEIGHT_PX}px`);

    this.bannerEl = document.createElement("div");
    this.bannerEl.className = "text-banner";
    this.bannerEl.hidden = true;

    this.rangeEl = document.createElement("span");
    this.rangeEl.className = "text-range";

    this.barEl = document.createElement("div");
    this.barEl.className = "text-bar";
    this.barEl.hidden = true;
    this.barEl.append(this.rangeEl);
    for (const [action, title] of [
      ["first", "First window"],
      ["prev", "Previous window"],
      ["next", "Next window"],
      ["last", "Last window"],
    ] as [WindowAction, string][]) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = "text-window-button";
      el.textContent = action;
      el.title = title;
      el.addEventListener("click", () => this.moveWindow(action));
      this.buttons.push({ action, el });
      this.barEl.append(el);
    }

    this.scrollEl = document.createElement("div");
    this.scrollEl.className = "text-scroll";
    // 실스크롤 컨테이너 자체를 focus 대상으로 둔다 — 방향키·PgUp/PgDn 스크롤이
    // 브라우저 기본 동작으로 붙는다 (뷰어지 에디터가 아니므로 자체 키 처리 없음).
    // Tab 순서에는 넣지 않는다 (프로그램적 focus 전용).
    this.scrollEl.tabIndex = -1;
    this.scrollEl.addEventListener("scroll", this.onScroll);

    this.spacerEl = document.createElement("div");
    this.spacerEl.className = "text-spacer";
    this.linesEl = document.createElement("div");
    this.linesEl.className = "text-lines";
    this.spacerEl.append(this.linesEl);
    this.scrollEl.append(this.spacerEl);

    this.root.append(this.bannerEl, this.barEl, this.scrollEl);
    parent.appendChild(this.root);

    // 자체 ResizeObserver (계획 프론트 계약) — pane 의 observer 는 터미널 fit
    // 전용이다. 크기가 바뀌면 viewport 에 들어오는 행 수가 달라진다.
    this.resizeObserver = new ResizeObserver(() => this.renderSlice());
    this.resizeObserver.observe(this.root);

    this.load(this.pendingOffset ?? 0);
  }

  /** 스냅샷 반영. 같은 파일이면 **아무것도 하지 않는다** — 스크롤 dispatch 가
   *  되돌아오는 매 렌더마다 위치를 재적용하면 사용자 스크롤과 싸운다 (에코 가드,
   *  shouldAdoptScroll). */
  update(kind: ViewerKind): void {
    if (kind.type !== "textViewer") {
      // 탭의 kind 종류는 생성 후 바뀌지 않는다 — 오면 레지스트리 배선 결함이다.
      console.error("[wmux] text view received a non-textViewer kind", kind);
      return;
    }
    if (!shouldAdoptScroll(this.adopted, kind.path)) return;
    this.adopted = { path: kind.path };
    this.path = kind.path;
    this.pendingOffset = kind.scrollTop;
    this.settle.markSynced(null);
    this.load(kind.scrollTop);
  }

  /** unmount 직전 보류분 배출 (탭이 스냅샷에 남아 있을 때만 불린다 —
   *  workspace-view 의 리컨실 계약). */
  flushScroll(): void {
    this.settle.flush();
  }

  focus(): void {
    this.scrollEl.focus();
  }

  dispose(): void {
    this.disposed = true;
    this.settle.dispose();
    this.resizeObserver.disconnect();
    this.scrollEl.removeEventListener("scroll", this.onScroll);
    this.root.remove();
  }

  /** 스크롤 이벤트 — 슬라이스는 즉시, 모델 기록은 settle 후. */
  private readonly onScroll = (): void => {
    this.renderSlice();
    const offset = this.topLineOffset();
    if (offset !== null) this.settle.observe(offset);
  };

  /** 최상단 가시 행의 전역 byte offset (모델 scrollTop 의 시맨틱). */
  private topLineOffset(): number | null {
    const starts = this.win.lineStarts;
    if (starts.length === 0) return null;
    const index = Math.min(
      starts.length - 1,
      Math.max(0, Math.floor(this.scrollEl.scrollTop / LINE_HEIGHT_PX)),
    );
    return starts[index];
  }

  private moveWindow(action: WindowAction): void {
    // 창을 옮기기 전에 지금 위치를 확정해 둔다 — 이동 후의 scroll 이벤트가
    // 이전 위치를 덮어쓰기 전에 모델에 남긴다.
    this.settle.flush();
    this.load(nextWindowStart(action, this.win, this.size));
  }

  /** 목표 offset 을 담는 창을 읽어 온다. 실패는 인라인 에러로 표면화하고 탭은
   *  유지한다 (없는·삭제된 파일도 모델에 남아 재시도가 가능해야 한다). */
  private load(target: number): void {
    const token = ++this.loadToken;
    this.setBanner("loading…", false);
    this.loadWindow(target, token).catch((err: unknown) => {
      if (this.disposed || token !== this.loadToken) return;
      this.renderError(describeError(err));
    });
  }

  private async loadWindow(target: number, token: number): Promise<void> {
    // 크기를 먼저 안다: EOF 판정(말미 부분행을 자를지)·범위 표시·창 이동 버튼이
    // 전부 전체 크기 위에 선다.
    const stat = await fsStat(this.distro, this.path);
    if (this.disposed || token !== this.loadToken) return;
    if (stat.is_dir) {
      this.renderError("it is a directory");
      return;
    }

    let start = Math.max(0, Math.min(target, stat.size));
    // 파일이 그새 줄어 목표가 EOF 밖이면 마지막 창으로 되감는다.
    if (start >= stat.size) start = Math.max(0, stat.size - WINDOW_BYTES);
    // 목표가 행 시작이면 그 직전 바이트(개행)까지 읽어 둔다 — 선두 부분행 절삭이
    // 목표 행 자체를 먹어치우지 않게 하는 1바이트다.
    const readOffset = start > 0 ? start - 1 : 0;

    const buffer = await fsReadChunk(this.distro, this.path, readOffset, WINDOW_BYTES);
    if (this.disposed || token !== this.loadToken) return;
    const bytes = new Uint8Array(buffer);
    const atEof = bytes.length < WINDOW_BYTES || readOffset + bytes.length >= stat.size;

    this.setBanner(null, false);
    this.showWindow(decodeWindow(bytes, readOffset, atEof), stat.size);
  }

  /** 새 창을 화면에 앉힌다 — 전체 높이·범위 표시·버튼 상태를 갱신하고, 보류된
   *  복원 위치가 있으면 거기로, 없으면 창 맨 위로 보낸다. */
  private showWindow(win: TextWindow, size: number): void {
    this.win = win;
    this.size = size;
    this.spacerEl.style.height = `${win.lines.length * LINE_HEIGHT_PX}px`;
    this.slice = null;

    const paged = win.start > 0 || win.end < size;
    this.barEl.hidden = !paged;
    if (paged) {
      this.rangeEl.textContent = formatByteRange(win.start, win.end, size);
      for (const { action, el } of this.buttons) {
        el.disabled =
          action === "first" || action === "prev" ? win.start === 0 : win.end >= size;
      }
    }

    const restore = this.pendingOffset;
    this.pendingOffset = null;
    const index = restore === null ? 0 : lineIndexForOffset(win.lineStarts, restore);
    const top = win.lineStarts[index] ?? null;
    // 복원(마운트)은 모델에서 온 값이라 그대로 되돌려 보낼 것이 없다 — scrollTop
    // 대입이 발화시키는 scroll 이벤트를 markSynced 로 잠재운다. 반대로 창 이동은
    // 새 위치를 모델에 남겨야 재마운트가 그 창으로 돌아온다.
    this.settle.markSynced(restore === null ? null : top);
    this.scrollEl.scrollTop = index * LINE_HEIGHT_PX;
    this.renderSlice();
    if (restore === null && top !== null) this.settle.observe(top);
  }

  /** 로드 실패 — 인라인 에러로 표면화하고 탭·모델 위치는 건드리지 않는다
   *  (빈 창이라 topLineOffset 이 null 이고, 저장된 scrollTop 을 0 으로 덮지
   *  않는다: 파일이 돌아오면 원래 지점으로 복원된다). */
  private renderError(message: string): void {
    this.setBanner(`cannot read ${this.path}: ${message}`, true);
    this.showWindow({ start: 0, end: 0, lines: [], lineStarts: [] }, 0);
  }

  /** viewport ± overscan 만큼만 실제 노드를 만든다. 구간이 그대로면 no-op. */
  private renderSlice(): void {
    const range = visibleSlice(
      this.scrollEl.scrollTop,
      this.scrollEl.clientHeight,
      this.win.lines.length,
    );
    if (this.slice !== null && this.slice.first === range.first && this.slice.last === range.last) {
      return;
    }
    this.slice = { first: range.first, last: range.last };
    this.linesEl.style.top = `${range.top}px`;
    const nodes: HTMLDivElement[] = [];
    for (let i = range.first; i < range.last; i += 1) {
      const el = document.createElement("div");
      el.className = "text-line";
      el.textContent = this.win.lines[i];
      nodes.push(el);
    }
    this.linesEl.replaceChildren(...nodes);
  }

  private setBanner(text: string | null, error: boolean): void {
    this.bannerEl.textContent = text ?? "";
    this.bannerEl.hidden = text === null;
    this.bannerEl.classList.toggle("error", error);
  }
}
