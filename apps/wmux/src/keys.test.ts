// 키보드 3층 이동 판정의 계약 테스트 (20단계) — 매핑 전수·IME 가드·경계 조건.
// DOM 무의존 (리스너 설치·preventDefault·dispatch 배선은 main.ts 글루 소관).

import { describe, expect, it } from "vitest";

import { keyAction, nextTab, paneInDirection, workspaceAtOrdinal } from "./keys";
import type { KeySpec, PaneRect } from "./keys";

/** 기본 no-modifier·조합 아님 spec — 테스트마다 필요한 것만 덮어쓴다. */
function spec(over: Partial<KeySpec> & { key: string }): KeySpec {
  return { ctrl: false, alt: false, shift: false, isComposing: false, ...over };
}

describe("keyAction", () => {
  it("Ctrl+1~9 는 1-based 워크스페이스 ordinal 로 매핑된다", () => {
    for (let n = 1; n <= 9; n += 1) {
      expect(keyAction(spec({ key: String(n), ctrl: true }))).toEqual({
        type: "switchWorkspace",
        ordinal: n,
      });
    }
  });

  it("Alt+방향키 4종은 pane 이동 방향으로 매핑된다", () => {
    expect(keyAction(spec({ key: "ArrowUp", alt: true }))).toEqual({
      type: "focusPane",
      dir: "up",
    });
    expect(keyAction(spec({ key: "ArrowDown", alt: true }))).toEqual({
      type: "focusPane",
      dir: "down",
    });
    expect(keyAction(spec({ key: "ArrowLeft", alt: true }))).toEqual({
      type: "focusPane",
      dir: "left",
    });
    expect(keyAction(spec({ key: "ArrowRight", alt: true }))).toEqual({
      type: "focusPane",
      dir: "right",
    });
  });

  it("Ctrl+Tab 은 다음 탭, Ctrl+Shift+Tab 은 이전 탭이다", () => {
    expect(keyAction(spec({ key: "Tab", ctrl: true }))).toEqual({ type: "cycleTab", delta: 1 });
    expect(keyAction(spec({ key: "Tab", ctrl: true, shift: true }))).toEqual({
      type: "cycleTab",
      delta: -1,
    });
  });

  it("IME 조합 중(isComposing)에는 어떤 이동 키도 가로채지 않는다", () => {
    expect(keyAction(spec({ key: "1", ctrl: true, isComposing: true }))).toBeNull();
    expect(keyAction(spec({ key: "Tab", ctrl: true, isComposing: true }))).toBeNull();
    expect(keyAction(spec({ key: "ArrowLeft", alt: true, isComposing: true }))).toBeNull();
  });

  it("shift 변형은 Ctrl+Shift+Tab 만 허용한다 — 숫자·방향키의 shift 조합은 null", () => {
    expect(keyAction(spec({ key: "1", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "ArrowRight", alt: true, shift: true }))).toBeNull();
  });

  it("모디파이어가 어긋난 조합은 null — 맨 키·Ctrl+Alt 혼합·Alt+Ctrl 방향키", () => {
    expect(keyAction(spec({ key: "1" }))).toBeNull();
    expect(keyAction(spec({ key: "Tab" }))).toBeNull();
    expect(keyAction(spec({ key: "ArrowLeft" }))).toBeNull();
    expect(keyAction(spec({ key: "1", ctrl: true, alt: true }))).toBeNull();
    expect(keyAction(spec({ key: "Tab", ctrl: true, alt: true }))).toBeNull();
    expect(keyAction(spec({ key: "ArrowLeft", alt: true, ctrl: true }))).toBeNull();
  });

  it("가로채기 목록 밖의 키는 null — Ctrl+0, Ctrl+Shift+R(리로드), Esc, 일반 문자", () => {
    expect(keyAction(spec({ key: "0", ctrl: true }))).toBeNull();
    expect(keyAction(spec({ key: "R", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "Escape" }))).toBeNull();
    expect(keyAction(spec({ key: "c", ctrl: true }))).toBeNull();
    expect(keyAction(spec({ key: "a" }))).toBeNull();
  });
});

describe("workspaceAtOrdinal", () => {
  it("ordinal 은 사이드바 순서를 1-based 로 인덱싱한다", () => {
    expect(workspaceAtOrdinal([10, 20, 30], 1)).toBe(10);
    expect(workspaceAtOrdinal([10, 20, 30], 3)).toBe(30);
  });

  it("범위를 벗어난 ordinal 은 null — 조용한 no-op", () => {
    expect(workspaceAtOrdinal([10, 20, 30], 4)).toBeNull();
    expect(workspaceAtOrdinal([], 1)).toBeNull();
  });
});

describe("paneInDirection", () => {
  // 2x2 격자 — 좌상 1, 우상 2, 좌하 3, 우하 4 (각 100x100).
  const grid: PaneRect[] = [
    { pane: 1, x: 0, y: 0, w: 100, h: 100 },
    { pane: 2, x: 100, y: 0, w: 100, h: 100 },
    { pane: 3, x: 0, y: 100, w: 100, h: 100 },
    { pane: 4, x: 100, y: 100, w: 100, h: 100 },
  ];

  it("2x2 격자에서 4방향 인접 pane 을 고른다", () => {
    expect(paneInDirection(grid, 1, "right")).toBe(2);
    expect(paneInDirection(grid, 1, "down")).toBe(3);
    expect(paneInDirection(grid, 4, "left")).toBe(3);
    expect(paneInDirection(grid, 4, "up")).toBe(2);
  });

  it("방향 반평면에 후보가 없으면 null — 가장자리 pane·단일 pane", () => {
    expect(paneInDirection(grid, 2, "right")).toBeNull();
    expect(paneInDirection(grid, 1, "up")).toBeNull();
    const single: PaneRect[] = [{ pane: 7, x: 0, y: 0, w: 200, h: 200 }];
    expect(paneInDirection(single, 7, "left")).toBeNull();
    expect(paneInDirection(single, 7, "down")).toBeNull();
  });

  it("from 이 목록에 없으면 null", () => {
    expect(paneInDirection(grid, 99, "left")).toBeNull();
    expect(paneInDirection([], 1, "up")).toBeNull();
  });

  it("후보가 여럿이면 중심 유클리드 거리가 최소인 pane 을 고른다", () => {
    // from(중심 250,50) 왼쪽에 둘 — 먼 A(중심 50,50: 200) vs 가까운 B(중심
    // 150,110: 약 117). 배열 순서가 아니라 거리로 뽑혀야 한다.
    const rects: PaneRect[] = [
      { pane: 1, x: 200, y: 0, w: 100, h: 100 },
      { pane: 2, x: 0, y: 0, w: 100, h: 100 },
      { pane: 3, x: 100, y: 60, w: 100, h: 100 },
    ];
    expect(paneInDirection(rects, 1, "left")).toBe(3);
  });
});

describe("nextTab", () => {
  it("delta 방향으로 이웃 탭을 고르고 끝에서 순환한다", () => {
    expect(nextTab([11, 22, 33], 11, 1)).toBe(22);
    expect(nextTab([11, 22, 33], 33, 1)).toBe(11); // wrap
    expect(nextTab([11, 22, 33], 22, -1)).toBe(11);
    expect(nextTab([11, 22, 33], 11, -1)).toBe(33); // wrap
  });

  it("탭이 0~1개면 null — 순환할 이웃이 없다", () => {
    expect(nextTab([], null, 1)).toBeNull();
    expect(nextTab([11], 11, 1)).toBeNull();
    expect(nextTab([11], 11, -1)).toBeNull();
  });

  it("활성 탭이 없거나 목록에 없으면 null", () => {
    expect(nextTab([11, 22], null, 1)).toBeNull();
    expect(nextTab([11, 22], 99, 1)).toBeNull();
  });
});
