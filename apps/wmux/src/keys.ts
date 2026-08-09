// 키보드 3층 이동 판정 — DOM 무의존 순수 모듈 (20단계, 계획 v2 "키보드 모델" 장).
//
// 3층 구조(워크스페이스 / pane / 탭)에 이동 키를 한 벌씩 배정하고, 여기서는
// "이 keydown 이 어떤 이동인가"와 "그 이동의 대상이 무엇인가"만 계산한다.
// 스냅샷 해석·dispatch·preventDefault 배선은 main.ts 글루, pane 기하 실측은
// workspace-view 의 paneRects() 가 맡는다.
//
// ## 키보드 가로채기 목록 (canonical — 계획 v2 "키보드 가로채기 목록" 장)
//
// 앱이 가로채는 키의 **전량**이다. 여기 없는 키는 전부 터미널(PTY) 소유다.
// 가로채기를 추가·변경하면 이 표를 같이 갱신한다 (목록과 코드를 한곳에 두기
// 위한 계약 — 표가 정본이다).
//
// | 키 | 동작 | 가로채는 곳 |
// |---|---|---|
// | `Ctrl+1`~`Ctrl+9` | 워크스페이스 전환 (사이드바 순서 1-based) | keys.ts 판정 + main.ts window keydown capture |
// | `Alt+↑` `Alt+↓` `Alt+←` `Alt+→` | pane 포커스 이동 (기하학적 인접) | keys.ts 판정 + main.ts window keydown capture |
// | `Ctrl+Tab` / `Ctrl+Shift+Tab` | 활성 pane 의 탭 순환 (다음/이전, 끝에서 순환) | keys.ts 판정 + main.ts window keydown capture |
// | `Ctrl+Shift+R` | WebView 리로드 (F5 는 쓰지 않는다 — main.ts 주석 참조) | main.ts installReloadKey |
// | `Ctrl+V` / `Ctrl+Shift+V` / `Shift+Insert` | 붙여넣기 (클립보드 → xterm paste) | terminal-view.ts customKeyEventHandler |
// | `Ctrl+C` / `Ctrl+Shift+C` / `Ctrl+Insert` (선택 있을 때만) | 복사 — 선택 없는 `Ctrl+C` 는 SIGINT 로 통과 | terminal-view.ts customKeyEventHandler |
// | `Esc` (send-mode 활성 중에만) | 전달 대상 선택 취소 — 평시 Esc 는 PTY 소유 | workspace-view.ts (모드 활성 중에만 설치) |
//
// shift 규약: 이동 3종 중 shift 를 받는 건 `Ctrl+Shift+Tab` 뿐이다. `Ctrl+Shift+1`·
// `Alt+Shift+←` 같은 변형은 판정 대상이 아니다(null) — shift 는 레이아웃에 따라
// 다른 문자를 만들 수 있어 보수적으로 목록에 명시된 조합만 가로챈다.

import type { PaneId, TabId, WorkspaceId } from "./types";

/** keydown 판정 입력 — KeyboardEvent 의 구조적 부분집합 (DOM 없이 테스트하기
 *  위한 최소 형태). 실코드에서는 이벤트의 key/ctrlKey/altKey/shiftKey/isComposing
 *  을 그대로 옮겨 담으면 된다. */
export interface KeySpec {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  /** IME 조합 중 여부 — 조합 중의 키는 조합기 소유라 가로채지 않는다. */
  isComposing: boolean;
}

/** pane 이동 방향 — 화면 기하 기준 (Alt+방향키). */
export type PaneDirection = "up" | "down" | "left" | "right";

/** 판정 결과. ordinal 은 1-based 사이드바 순서, delta 는 탭 순환 방향이다.
 *  대상 해석(그 ordinal 의 워크스페이스가 있는가 등)은 글루의 몫 — 여기서는
 *  키가 무슨 이동을 뜻하는지까지만 정한다. */
export type KeyAction =
  | { type: "switchWorkspace"; ordinal: number }
  | { type: "focusPane"; dir: PaneDirection }
  | { type: "cycleTab"; delta: 1 | -1 };

const ARROW_DIRS: Record<string, PaneDirection | undefined> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
};

/** 워크스페이스 전환 키의 ordinal 범위 (`Ctrl+1`~`Ctrl+9` — `Ctrl+0` 은 미배정). */
const DIGIT_KEY = /^[1-9]$/;

/** keydown → 이동 액션. 가로채기 목록에 없는 조합은 전부 null 이고, 그때 글루는
 *  이벤트에 손대지 않는다 (터미널로 그대로 흘려보낸다). */
export function keyAction(spec: KeySpec): KeyAction | null {
  // IME 조합 중의 키는 전부 통과시킨다 — 한글 입력 중의 조합 키가 이동으로
  // 오판돼 조합을 깨뜨리면 안 된다.
  if (spec.isComposing) return null;
  if (spec.ctrl && !spec.alt && spec.key === "Tab") {
    return { type: "cycleTab", delta: spec.shift ? -1 : 1 };
  }
  // 여기부터는 shift 변형을 받지 않는다 (파일 상단 shift 규약).
  if (spec.shift) return null;
  if (spec.ctrl && !spec.alt && DIGIT_KEY.test(spec.key)) {
    return { type: "switchWorkspace", ordinal: Number(spec.key) };
  }
  if (spec.alt && !spec.ctrl) {
    const dir = ARROW_DIRS[spec.key];
    if (dir !== undefined) return { type: "focusPane", dir };
  }
  return null;
}

/** pane 의 화면 기하 — DOMRect 의 구조적 부분집합. workspace-view 가
 *  getBoundingClientRect 로 채운다 (레이아웃 트리 대신 실측을 쓰는 이유는
 *  paneRects() 주석 참조). */
export interface PaneRect {
  pane: PaneId;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 방향 이동 대상 — from 중심을 기준으로 그 방향 반평면에 중심이 있는 pane 중
 *  중심 간 유클리드 거리가 최소인 것. 후보가 없거나(가장자리 pane·단일 pane)
 *  from 이 목록에 없으면 null 이고, 글루는 조용한 no-op 으로 끝낸다.
 *
 *  반평면 판정은 엄격 부등호다 — 중심 좌표가 같은 pane 은 그 방향의 후보가
 *  아니다. 거리 동률은 목록에서 먼저 나온 pane 이 이긴다 (렌더 순서 = 문서 순서). */
export function paneInDirection(
  rects: PaneRect[],
  from: PaneId,
  dir: PaneDirection,
): PaneId | null {
  const origin = rects.find((r) => r.pane === from);
  if (origin === undefined) return null;
  const ox = origin.x + origin.w / 2;
  const oy = origin.y + origin.h / 2;
  let best: PaneId | null = null;
  let bestDist = Infinity;
  for (const r of rects) {
    if (r.pane === from) continue;
    const cx = r.x + r.w / 2;
    const cy = r.y + r.h / 2;
    const inDir =
      dir === "left" ? cx < ox : dir === "right" ? cx > ox : dir === "up" ? cy < oy : cy > oy;
    if (!inDir) continue;
    const dist = Math.hypot(cx - ox, cy - oy);
    if (dist < bestDist) {
      bestDist = dist;
      best = r.pane;
    }
  }
  return best;
}

/** 1-based ordinal → 워크스페이스 id. 사이드바 카드 순서 = `state.workspaces`
 *  배열 순서라 인덱싱으로 충분하다. 범위를 벗어나면 null (조용한 no-op). */
export function workspaceAtOrdinal(
  workspaces: WorkspaceId[],
  ordinal: number,
): WorkspaceId | null {
  const found = workspaces[ordinal - 1];
  return found === undefined ? null : found;
}

/** 탭 순환 대상 — tabs 는 pane 의 탭 순서, active 는 그 pane 의 활성 탭.
 *  끝에서 순환하고, 탭이 0~1개거나 active 가 목록에 없으면 null (no-op). */
export function nextTab(tabs: TabId[], active: TabId | null, delta: 1 | -1): TabId | null {
  if (tabs.length < 2 || active === null) return null;
  const idx = tabs.indexOf(active);
  if (idx < 0) return null;
  return tabs[(idx + delta + tabs.length) % tabs.length];
}
