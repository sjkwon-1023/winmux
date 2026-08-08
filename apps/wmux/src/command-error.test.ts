// formatCommandError 검증 — 4개 CommandError variant 의 한 줄 요약과 계약 밖
// payload(문자열·객체·undefined) 폴백.

import { describe, expect, it } from "vitest";

import { formatCommandError } from "./command-error";

describe("formatCommandError", () => {
  it("formats invalidRatio with the offending value", () => {
    expect(formatCommandError({ type: "invalidRatio", ratio: 1.5 })).toBe(
      "Invalid split ratio 1.5 (must be strictly between 0 and 1)",
    );
  });

  it("formats unknownTarget with the target string", () => {
    expect(formatCommandError({ type: "unknownTarget", target: "SplitId(99)" })).toBe(
      "Target not found: SplitId(99) (stale id?)",
    );
  });

  it("formats lastPane", () => {
    expect(formatCommandError({ type: "lastPane" })).toBe(
      "Cannot close the last pane of a workspace",
    );
  });

  it("formats spawnFailed and collapses multiline messages to one line", () => {
    expect(formatCommandError({ type: "spawnFailed", message: "boom" })).toBe(
      "Shell spawn failed: boom",
    );
    expect(
      formatCommandError({ type: "spawnFailed", message: "line1\n  line2\r\nline3" }),
    ).toBe("Shell spawn failed: line1 line2 line3");
  });

  it("passes through non-contract string payloads", () => {
    expect(formatCommandError("ipc timeout")).toBe("Command failed: ipc timeout");
  });

  it("serializes non-contract object payloads as JSON", () => {
    expect(formatCommandError({ code: 1 })).toBe('Command failed: {"code":1}');
  });

  it("stringifies payloads JSON cannot represent", () => {
    expect(formatCommandError(undefined)).toBe("Command failed: undefined");
    const circular: { self?: unknown } = {};
    circular.self = circular;
    expect(formatCommandError(circular)).toBe("Command failed: [object Object]");
  });
});
