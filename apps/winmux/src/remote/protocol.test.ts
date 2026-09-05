import { describe, expect, it } from "vitest";

import {
  encodeInput,
  INITIAL_VIEW_STATE,
  needsRecreate,
  nextRequest,
  parseScreenMeta,
  screenQuery,
} from "./protocol";
import type { ScreenMeta, ViewState } from "./protocol";

function headers(map: Record<string, string>): (name: string) => string | null {
  return (name) => map[name] ?? null;
}

const FULL_HEADERS = {
  "X-Winmux-End-Offset": "4096",
  "X-Winmux-Reset": "1",
  "X-Winmux-Cols": "120",
  "X-Winmux-Rows": "30",
  "X-Winmux-Session": "77:3",
};

function meta(over: Partial<ScreenMeta> = {}): ScreenMeta {
  return { endOffset: 4096, reset: false, cols: 120, rows: 30, session: "77:3", ...over };
}

const READY: ViewState = {
  phase: "ready",
  since: 4096,
  session: "77:3",
  cols: 120,
  rows: 30,
};

describe("parseScreenMeta", () => {
  it("malformed headers are rejected", () => {
    expect(parseScreenMeta(headers(FULL_HEADERS))).toEqual({
      endOffset: 4096,
      reset: true,
      cols: 120,
      rows: 30,
      session: "77:3",
    });
    const broken: Record<string, string>[] = [
      { ...FULL_HEADERS, "X-Winmux-End-Offset": "" },
      { ...FULL_HEADERS, "X-Winmux-End-Offset": "1e3" },
      { ...FULL_HEADERS, "X-Winmux-End-Offset": "-1" },
      { ...FULL_HEADERS, "X-Winmux-Reset": "true" },
      { ...FULL_HEADERS, "X-Winmux-Reset": "" },
      { ...FULL_HEADERS, "X-Winmux-Cols": "0x10" },
      { ...FULL_HEADERS, "X-Winmux-Session": "" },
    ];
    for (const map of broken) expect(parseScreenMeta(headers(map))).toBeNull();
    for (const missing of Object.keys(FULL_HEADERS)) {
      const map = { ...FULL_HEADERS };
      delete map[missing as keyof typeof FULL_HEADERS];
      expect(parseScreenMeta(headers(map))).toBeNull();
    }
  });
});

describe("nextRequest", () => {
  it("a reset reply to a full request is applied", () => {
    const next = nextRequest(INITIAL_VIEW_STATE, meta({ reset: true }));
    expect(next).toEqual({ phase: "ready", since: 4096, session: "77:3", cols: 120, rows: 30 });
    expect(screenQuery(next)).toEqual({ since: 4096, session: "77:3" });
    expect(screenQuery(INITIAL_VIEW_STATE)).toBeNull();
  });

  it("nextSince follows the end offset", () => {
    const next = nextRequest(READY, meta({ endOffset: 5000 }));
    expect(next.phase).toBe("ready");
    expect(next.since).toBe(5000);
    expect(screenQuery(next)).toEqual({ since: 5000, session: "77:3" });
  });

  it("an empty delta keeps the offset", () => {
    expect(nextRequest(READY, meta({ endOffset: 4096 }))).toEqual(READY);
  });

  it("a reset reply to a delta request goes back to full", () => {
    const got = meta({ reset: true, endOffset: 9000 });
    expect(needsRecreate(READY, got)).toBe(true);
    expect(nextRequest(READY, got)).toEqual(INITIAL_VIEW_STATE);
  });

  it("a size change goes back to full", () => {
    const wider = meta({ cols: 100, endOffset: 5000 });
    expect(needsRecreate(READY, wider)).toBe(true);
    expect(nextRequest(READY, wider)).toEqual(INITIAL_VIEW_STATE);
    const shorter = meta({ rows: 24, endOffset: 5000 });
    expect(nextRequest(READY, shorter)).toEqual(INITIAL_VIEW_STATE);
  });

  it("a session change goes back to full", () => {
    const restarted = meta({ session: "77:4", endOffset: 12 });
    expect(needsRecreate(READY, restarted)).toBe(true);
    expect(nextRequest(READY, restarted)).toEqual(INITIAL_VIEW_STATE);
  });
});

describe("encodeInput", () => {
  const off = { bracketedPasteMode: false, applicationCursorKeysMode: false };

  it("paste is bracketed only when the mode is on", () => {
    const action = { type: "paste", text: "ls -al" } as const;
    expect(encodeInput(action, off)).toBe("ls -al");
    expect(encodeInput(action, { ...off, bracketedPasteMode: true })).toBe(
      "\x1b[200~ls -al\x1b[201~",
    );
  });

  it("arrows follow application cursor mode", () => {
    const on = { ...off, applicationCursorKeysMode: true };
    expect(encodeInput({ type: "key", key: "up" }, off)).toBe("\x1b[A");
    expect(encodeInput({ type: "key", key: "up" }, on)).toBe("\x1bOA");
    expect(encodeInput({ type: "key", key: "down" }, off)).toBe("\x1b[B");
    expect(encodeInput({ type: "key", key: "down" }, on)).toBe("\x1bOB");
    expect(encodeInput({ type: "key", key: "right" }, off)).toBe("\x1b[C");
    expect(encodeInput({ type: "key", key: "right" }, on)).toBe("\x1bOC");
    expect(encodeInput({ type: "key", key: "left" }, off)).toBe("\x1b[D");
    expect(encodeInput({ type: "key", key: "left" }, on)).toBe("\x1bOD");
    // 나머지 키는 모드와 무관하다.
    expect(encodeInput({ type: "key", key: "escape" }, on)).toBe("\x1b");
    expect(encodeInput({ type: "key", key: "tab" }, on)).toBe("\t");
    expect(encodeInput({ type: "key", key: "ctrlC" }, on)).toBe("\x03");
    expect(encodeInput({ type: "key", key: "backspace" }, on)).toBe("\x7f");
    expect(encodeInput({ type: "key", key: "enter" }, on)).toBe("\r");
  });
});
