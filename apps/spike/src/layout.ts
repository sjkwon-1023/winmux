// 터미널 개수 → 그리드 행·열 결정 (spike-plan.md 4.6: 1 / 2×2 / 4×2 부하 테스트용).
// 8개 초과는 4열을 유지한 채 행을 늘린다.

export interface GridDims {
  cols: number;
  rows: number;
}

export function gridDims(count: number): GridDims {
  if (count <= 1) return { cols: 1, rows: 1 };
  if (count <= 4) return { cols: 2, rows: 2 };
  return { cols: 4, rows: Math.max(2, Math.ceil(count / 4)) };
}
