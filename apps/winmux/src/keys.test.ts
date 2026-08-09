// 키보드 판정의 계약 테스트 (20단계 + Ctrl+Shift 전역 단축키) — 매핑 전수·
// IME 가드·경계 조건. DOM 무의존 (리스너 설치·preventDefault·dispatch 배선은
// main.ts 글루 소관).

import { describe, expect, it } from "vitest";

import { keyAction, nextTab, paneInDirection, shortcutLabel, workspaceAtOrdinal } from "./keys";
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

  it("shift 변형은 Ctrl+Shift+Tab 과 Ctrl+Shift+<문자>만 허용한다 — 숫자·방향키의 shift 조합은 null", () => {
    expect(keyAction(spec({ key: "1", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "ArrowRight", alt: true, shift: true }))).toBeNull();
  });

  it("Ctrl+Shift+<문자> 6종은 각 전역 단축키로 매핑된다", () => {
    expect(keyAction(spec({ key: "W", ctrl: true, shift: true }))).toEqual({ type: "closeTab" });
    expect(keyAction(spec({ key: "T", ctrl: true, shift: true }))).toEqual({
      type: "newTab",
      kind: "terminal",
    });
    expect(keyAction(spec({ key: "B", ctrl: true, shift: true }))).toEqual({
      type: "newTab",
      kind: "folderBrowser",
    });
    // 방향 규약(split-layout.ts): "vertical" = 상/하, "horizontal" = 좌|우.
    expect(keyAction(spec({ key: "D", ctrl: true, shift: true }))).toEqual({
      type: "splitPane",
      direction: "vertical",
    });
    expect(keyAction(spec({ key: "E", ctrl: true, shift: true }))).toEqual({
      type: "splitPane",
      direction: "horizontal",
    });
    expect(keyAction(spec({ key: "N", ctrl: true, shift: true }))).toEqual({
      type: "focusNewWorkspace",
    });
  });

  it("Ctrl+Shift+<문자> 매칭은 대소문자를 무시한다 — CapsLock·레이아웃 차이 흡수", () => {
    expect(keyAction(spec({ key: "w", ctrl: true, shift: true }))).toEqual({ type: "closeTab" });
    expect(keyAction(spec({ key: "n", ctrl: true, shift: true }))).toEqual({
      type: "focusNewWorkspace",
    });
  });

  it("Ctrl+Shift+C / Ctrl+Shift+V 는 절대 매칭되지 않는다 — 복사·붙여넣기 관례", () => {
    expect(keyAction(spec({ key: "C", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "c", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "V", ctrl: true, shift: true }))).toBeNull();
    expect(keyAction(spec({ key: "v", ctrl: true, shift: true }))).toBeNull();
  });

  it("Ctrl+Shift+<문자> 는 Alt 가 섞이거나 Shift 가 빠지면 매칭되지 않는다", () => {
    // plain Ctrl 조합은 셸 소유다 (Ctrl+W 단어 삭제·Ctrl+D EOF).
    expect(keyAction(spec({ key: "w", ctrl: true }))).toBeNull();
    expect(keyAction(spec({ key: "d", ctrl: true }))).toBeNull();
    expect(keyAction(spec({ key: "W", ctrl: true, shift: true, alt: true }))).toBeNull();
    expect(keyAction(spec({ key: "W", shift: true }))).toBeNull();
  });

  it("IME 조합 중에는 Ctrl+Shift+<문자>도 가로채지 않는다", () => {
    expect(keyAction(spec({ key: "W", ctrl: true, shift: true, isComposing: true }))).toBeNull();
    expect(keyAction(spec({ key: "N", ctrl: true, shift: true, isComposing: true }))).toBeNull();
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

describe("shortcutLabel", () => {
  const ids = [
    "closeTab",
    "newTerminalTab",
    "newFolderTab",
    "splitTopBottom",
    "splitLeftRight",
    "newWorkspace",
  ] as const;

  it("툴팁 라벨은 Ctrl+Shift+<대문자> 형식이다", () => {
    expect(shortcutLabel("closeTab")).toBe("Ctrl+Shift+W");
    expect(shortcutLabel("newTerminalTab")).toBe("Ctrl+Shift+T");
    expect(shortcutLabel("newFolderTab")).toBe("Ctrl+Shift+B");
    expect(shortcutLabel("splitTopBottom")).toBe("Ctrl+Shift+D");
    expect(shortcutLabel("splitLeftRight")).toBe("Ctrl+Shift+E");
    expect(shortcutLabel("newWorkspace")).toBe("Ctrl+Shift+N");
  });

  it("라벨이 가리키는 키를 실제로 누르면 keyAction 이 매칭된다 — 표시·판정 표류 방지", () => {
    for (const id of ids) {
      const label = shortcutLabel(id);
      const key = label.slice("Ctrl+Shift+".length);
      expect(label.startsWith("Ctrl+Shift+")).toBe(true);
      expect(keyAction(spec({ key, ctrl: true, shift: true }))).not.toBeNull();
    }
  });

  it("라벨은 서로 겹치지 않는다 — 한 키에 두 동작이 붙지 않는다", () => {
    const labels = ids.map((id) => shortcutLabel(id));
    expect(new Set(labels).size).toBe(labels.length);
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
