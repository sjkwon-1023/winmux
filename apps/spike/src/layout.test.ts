// gridDims — 타일 개수별 그리드 결정 검증 (1 / 2×2 / 4×2).

import { describe, expect, it } from "vitest";

import { gridDims } from "./layout";

describe("gridDims", () => {
  it("uses a single cell for zero or one terminal", () => {
    expect(gridDims(0)).toEqual({ cols: 1, rows: 1 });
    expect(gridDims(1)).toEqual({ cols: 1, rows: 1 });
  });

  it("uses a 2x2 grid for two to four terminals", () => {
    expect(gridDims(2)).toEqual({ cols: 2, rows: 2 });
    expect(gridDims(3)).toEqual({ cols: 2, rows: 2 });
    expect(gridDims(4)).toEqual({ cols: 2, rows: 2 });
  });

  it("uses a 4x2 grid for five to eight terminals", () => {
    expect(gridDims(5)).toEqual({ cols: 4, rows: 2 });
    expect(gridDims(8)).toEqual({ cols: 4, rows: 2 });
  });

  it("grows rows past eight terminals while keeping four columns", () => {
    expect(gridDims(9)).toEqual({ cols: 4, rows: 3 });
    expect(gridDims(12)).toEqual({ cols: 4, rows: 3 });
  });
});
