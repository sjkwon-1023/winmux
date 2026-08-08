// OscLog 보관 상한과 formatOscEntry 포맷 검증.

import { describe, expect, it } from "vitest";

import { OscLog, formatOscEntry } from "./osc-log";
import type { OscLogEntry } from "./osc-log";

function entry(overrides: Partial<OscLogEntry> = {}): OscLogEntry {
  return {
    id: 1,
    kind: "osc777",
    title: "",
    body: "",
    // 로컬 시간 03:04:05 고정
    at: new Date(2026, 0, 2, 3, 4, 5),
    ...overrides,
  };
}

describe("formatOscEntry", () => {
  it("formats time, id, and kind", () => {
    expect(formatOscEntry(entry())).toBe("[03:04:05] #1 osc777");
  });

  it("includes title and body when present", () => {
    const line = formatOscEntry(entry({ id: 3, title: "build done", body: "exit 0" }));
    expect(line).toBe('[03:04:05] #3 osc777 title="build done" body="exit 0"');
  });

  it("omits empty title but keeps body", () => {
    const line = formatOscEntry(entry({ kind: "osc9", body: "hello" }));
    expect(line).toBe('[03:04:05] #1 osc9 body="hello"');
  });

  it("escapes control characters so the log stays one line", () => {
    const line = formatOscEntry(entry({ title: "a\nb\x1b[31m" }));
    expect(line).not.toContain("\n");
    expect(line).toContain("\\n");
    expect(line).toContain("\\u001b");
  });

  it("pads single-digit time components", () => {
    const line = formatOscEntry(entry({ at: new Date(2026, 0, 2, 0, 0, 9) }));
    expect(line.startsWith("[00:00:09]")).toBe(true);
  });
});

describe("OscLog", () => {
  it("keeps entries in insertion order", () => {
    const log = new OscLog();
    log.push(entry({ id: 1 }));
    log.push(entry({ id: 2 }));
    expect(log.entries.map((e) => e.id)).toEqual([1, 2]);
  });

  it("evicts the oldest entries beyond the cap", () => {
    const log = new OscLog(3);
    for (let i = 1; i <= 5; i++) log.push(entry({ id: i }));
    expect(log.entries.map((e) => e.id)).toEqual([3, 4, 5]);
  });

  it("caps at 100 entries by default", () => {
    const log = new OscLog();
    for (let i = 1; i <= 150; i++) log.push(entry({ id: i }));
    expect(log.entries.length).toBe(100);
    expect(log.entries[0]?.id).toBe(51);
    expect(log.entries[99]?.id).toBe(150);
  });

  it("rejects a non-positive cap", () => {
    expect(() => new OscLog(0)).toThrow();
  });
});
