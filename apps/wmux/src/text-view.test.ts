// textViewer 뷰의 순수 계산 검증 (21단계 청크 C2) — 윈도우 절삭(부분행·UTF-8
// 파단 바이트), 가상 스크롤 슬라이스, byte offset ↔ 행 매핑, 창 이동 계산,
// 스크롤 에코 가드, settle 디바운스. DOM·IPC 는 이 파일의 대상이 아니다 (뷰는
// 이 결과를 그대로 그리는 얇은 층이다).

import { describe, expect, it } from "vitest";

import {
  LINE_HEIGHT_PX,
  ScrollSettle,
  decodeWindow,
  formatByteRange,
  lineIndexForOffset,
  nextWindowStart,
  shouldAdoptScroll,
  visibleSlice,
} from "./text-view";
import type { TimerHost } from "./ack-batcher";

const encoder = new TextEncoder();

function bytes(text: string): Uint8Array {
  return encoder.encode(text);
}

/** 수동 진행 가짜 타이머 — 등록된 콜백을 tick 으로 직접 발화시킨다. */
class FakeTimers implements TimerHost {
  private next = 1;
  private readonly pending = new Map<number, { fn: () => void; ms: number }>();

  setTimeout(fn: () => void, ms: number): unknown {
    const handle = this.next++;
    this.pending.set(handle, { fn, ms });
    return handle;
  }

  clearTimeout(handle: unknown): void {
    this.pending.delete(handle as number);
  }

  get armed(): number {
    return this.pending.size;
  }

  /** 등록된 마지막 타이머의 지연(ms) — 디바운스 창 확인용. */
  get lastDelay(): number | null {
    let delay: number | null = null;
    for (const entry of this.pending.values()) delay = entry.ms;
    return delay;
  }

  /** 보류 중인 콜백 전부 발화. */
  fire(): void {
    const entries = [...this.pending.values()];
    this.pending.clear();
    for (const entry of entries) entry.fn();
  }
}

describe("decodeWindow", () => {
  it("keeps every line when the window is the whole file", () => {
    const win = decodeWindow(bytes("alpha\nbeta\ngamma\n"), 0, true);
    expect(win.lines).toEqual(["alpha", "beta", "gamma"]);
    expect(win.lineStarts).toEqual([0, 6, 11]);
    expect(win.start).toBe(0);
    expect(win.end).toBe(17);
  });

  it("keeps a trailing line without a final newline at EOF", () => {
    const win = decodeWindow(bytes("alpha\nbeta"), 0, true);
    expect(win.lines).toEqual(["alpha", "beta"]);
    expect(win.lineStarts).toEqual([0, 6]);
  });

  it("drops the leading partial line when the window starts mid-file", () => {
    // 창이 "pha\nbeta\ngamma\n" 에서 시작 — 첫 조각 "pha" 는 이전 창의 꼬리다.
    const win = decodeWindow(bytes("pha\nbeta\ngamma\n"), 100, true);
    expect(win.lines).toEqual(["beta", "gamma"]);
    // 전역 offset: 창 시작 100 + "pha\n" 4바이트.
    expect(win.lineStarts).toEqual([104, 109]);
    expect(win.start).toBe(104);
  });

  it("drops the trailing partial line when the window stops before EOF", () => {
    const win = decodeWindow(bytes("alpha\nbeta\ngam"), 0, false);
    expect(win.lines).toEqual(["alpha", "beta"]);
    expect(win.end).toBe(11);
  });

  it("trims both ends for a middle window", () => {
    const win = decodeWindow(bytes("pha\nbeta\ngam"), 100, false);
    expect(win.lines).toEqual(["beta"]);
    expect(win.start).toBe(104);
    expect(win.end).toBe(109);
  });

  it("strips a carriage return from display without shifting offsets", () => {
    const win = decodeWindow(bytes("alpha\r\nbeta\r\n"), 0, true);
    expect(win.lines).toEqual(["alpha", "beta"]);
    expect(win.lineStarts).toEqual([0, 7]);
  });

  it("shows the fragment when the window holds no newline at all", () => {
    // 창보다 긴 행 — 전부 잘라내 빈 화면을 만드는 대신 조각을 보여준다.
    const win = decodeWindow(bytes("no-newline-here"), 100, false);
    expect(win.lines).toEqual(["no-newline-here"]);
    expect(win.start).toBe(100);
  });

  it("drops a split multi-byte sequence at the leading edge", () => {
    // "가나" = 3바이트 x 2. 창이 첫 글자 중간(1바이트 뒤)에서 시작한다.
    const full = bytes("가나");
    const win = decodeWindow(full.subarray(1), 100, true);
    // 잘린 continuation 2바이트는 U+FFFD 가 아니라 아예 사라진다.
    expect(win.lines).toEqual(["나"]);
    expect(win.start).toBe(102);
  });

  it("drops a split multi-byte sequence at the trailing edge", () => {
    const full = bytes("가나"); // 6바이트
    const win = decodeWindow(full.subarray(0, 4), 0, false);
    expect(win.lines).toEqual(["가"]);
    expect(win.end).toBe(3);
  });

  it("keeps whole multi-byte characters inside a line-trimmed window", () => {
    const win = decodeWindow(bytes("가\n나다\n라"), 0, false);
    expect(win.lines).toEqual(["가", "나다"]);
    // "가\n" = 4바이트, "나다\n" = 7바이트.
    expect(win.lineStarts).toEqual([0, 4]);
    expect(win.end).toBe(11);
  });

  it("yields nothing when the window is empty", () => {
    const win = decodeWindow(new Uint8Array(0), 512, true);
    expect(win.lines).toEqual([]);
    expect(win.lineStarts).toEqual([]);
    expect(win.start).toBe(512);
    expect(win.end).toBe(512);
  });
});

describe("visibleSlice", () => {
  it("renders the viewport plus overscan on both sides", () => {
    // scrollTop 1600px = 100번째 행, viewport 320px = 20행(+1 걸침).
    const slice = visibleSlice(1600, 320, 1000, LINE_HEIGHT_PX, 20);
    expect(slice.first).toBe(80);
    expect(slice.last).toBe(141);
    expect(slice.top).toBe(80 * LINE_HEIGHT_PX);
  });

  it("clamps at the top and the bottom of the document", () => {
    const top = visibleSlice(0, 320, 1000, LINE_HEIGHT_PX, 20);
    expect(top.first).toBe(0);
    expect(top.top).toBe(0);

    const bottom = visibleSlice(1000 * LINE_HEIGHT_PX, 320, 1000, LINE_HEIGHT_PX, 20);
    expect(bottom.last).toBe(1000);
    expect(bottom.first).toBe(979);
  });

  it("returns an empty range for an empty document", () => {
    expect(visibleSlice(0, 320, 0)).toEqual({ first: 0, last: 0, top: 0 });
  });

  it("survives a zero-height viewport (pane not laid out yet)", () => {
    const slice = visibleSlice(0, 0, 100, LINE_HEIGHT_PX, 20);
    expect(slice.first).toBe(0);
    expect(slice.last).toBe(21);
  });
});

describe("lineIndexForOffset", () => {
  const starts = [0, 10, 20, 30];

  it("finds the line containing the offset", () => {
    expect(lineIndexForOffset(starts, 0)).toBe(0);
    expect(lineIndexForOffset(starts, 9)).toBe(0);
    expect(lineIndexForOffset(starts, 10)).toBe(1);
    expect(lineIndexForOffset(starts, 25)).toBe(2);
    expect(lineIndexForOffset(starts, 30)).toBe(3);
  });

  it("clamps offsets outside the window", () => {
    expect(lineIndexForOffset(starts, -5)).toBe(0);
    expect(lineIndexForOffset(starts, 9999)).toBe(3);
    expect(lineIndexForOffset([], 42)).toBe(0);
  });

  it("handles a window that does not start at the file start", () => {
    expect(lineIndexForOffset([1000, 1010], 1005)).toBe(0);
    expect(lineIndexForOffset([1000, 1010], 500)).toBe(0);
  });
});

describe("shouldAdoptScroll", () => {
  it("adopts once at mount", () => {
    expect(shouldAdoptScroll(null, "/var/log/syslog")).toBe(true);
  });

  it("never re-adopts for the same file (echo guard)", () => {
    // 우리가 보낸 setViewerScroll 이 스냅샷으로 되돌아오는 매 렌더가 이 경로다.
    expect(shouldAdoptScroll({ path: "/var/log/syslog" }, "/var/log/syslog")).toBe(false);
  });

  it("adopts again when the tab points at another file", () => {
    expect(shouldAdoptScroll({ path: "/var/log/syslog" }, "/etc/hosts")).toBe(true);
  });
});

describe("nextWindowStart", () => {
  const size = 10_000;
  const win = { start: 2_000, end: 3_000 };

  it("jumps to the file ends", () => {
    expect(nextWindowStart("first", win, size, 1_000)).toBe(0);
    expect(nextWindowStart("last", win, size, 1_000)).toBe(9_000);
  });

  it("steps back by one window and forward from the retained end", () => {
    expect(nextWindowStart("prev", win, size, 1_000)).toBe(1_000);
    // next 는 유지 구간 끝에서 이어 붙는다 — 바이트가 빠지지 않는다.
    expect(nextWindowStart("next", win, size, 1_000)).toBe(3_000);
  });

  it("clamps at both ends", () => {
    expect(nextWindowStart("prev", { start: 500, end: 1_500 }, size, 1_000)).toBe(0);
    expect(nextWindowStart("next", { start: 9_500, end: 10_000 }, size, 1_000)).toBe(9_000);
  });

  it("keeps the only window at 0 for a file smaller than the window", () => {
    const small = { start: 0, end: 100 };
    expect(nextWindowStart("last", small, 100, 1_000)).toBe(0);
    expect(nextWindowStart("next", small, 100, 1_000)).toBe(0);
  });
});

describe("formatByteRange", () => {
  it("groups digits without depending on the ICU locale", () => {
    expect(formatByteRange(0, 524_288, 12_345_678)).toBe("bytes 0–524,288 of 12,345,678");
  });
});

describe("ScrollSettle", () => {
  function make(): { settle: ScrollSettle; timers: FakeTimers; sent: number[] } {
    const timers = new FakeTimers();
    const sent: number[] = [];
    const settle = new ScrollSettle((offset) => sent.push(offset), 500, timers);
    return { settle, timers, sent };
  }

  it("sends only the last position after the settle window", () => {
    const { settle, timers, sent } = make();
    settle.observe(100);
    settle.observe(200);
    settle.observe(300);
    // 디바운스 — 스크롤이 멎기 전에는 아무것도 나가지 않는다.
    expect(sent).toEqual([]);
    expect(timers.armed).toBe(1);
    expect(timers.lastDelay).toBe(500);
    timers.fire();
    expect(sent).toEqual([300]);
  });

  it("restarts the settle window on every scroll event", () => {
    const { settle, timers, sent } = make();
    settle.observe(100);
    const first = timers.armed;
    settle.observe(200);
    // 이전 타이머는 취소되고 새 것 하나만 남는다 (throttle 이 아니라 debounce).
    expect(first).toBe(1);
    expect(timers.armed).toBe(1);
    timers.fire();
    expect(sent).toEqual([200]);
  });

  it("flushes the pending position immediately (unmount path)", () => {
    const { settle, timers, sent } = make();
    settle.observe(700);
    settle.flush();
    expect(sent).toEqual([700]);
    // 타이머는 취소돼 이중 전송이 없다.
    expect(timers.armed).toBe(0);
    timers.fire();
    expect(sent).toEqual([700]);
  });

  it("is a no-op when there is nothing pending", () => {
    const { settle, sent } = make();
    settle.flush();
    expect(sent).toEqual([]);
  });

  it("never re-sends the position the model already holds (echo guard)", () => {
    const { settle, timers, sent } = make();
    // 마운트 복원 — scrollTop 대입이 발화시킨 scroll 이벤트가 되돌아오는 경로.
    settle.markSynced(4_096);
    settle.observe(4_096);
    expect(timers.armed).toBe(0);
    timers.fire();
    expect(sent).toEqual([]);

    // 실제로 움직이면 나간다. 그 뒤 원래 위치로 돌아오면 다시 조용해진다.
    settle.observe(8_192);
    timers.fire();
    expect(sent).toEqual([8_192]);
    settle.observe(8_192);
    expect(timers.armed).toBe(0);
  });

  it("drops the pending position when the model position is re-fixed", () => {
    const { settle, timers, sent } = make();
    settle.observe(100);
    // 창 이동·경로 변경으로 좌표계가 갈린 경우 — 옛 보류분은 버린다.
    settle.markSynced(null);
    expect(timers.armed).toBe(0);
    timers.fire();
    expect(sent).toEqual([]);
  });

  it("stops the timer on dispose without sending", () => {
    const { settle, timers, sent } = make();
    settle.observe(100);
    settle.dispose();
    expect(timers.armed).toBe(0);
    timers.fire();
    expect(sent).toEqual([]);
  });
});
