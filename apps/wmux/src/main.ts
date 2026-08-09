// wmux 앱 엔트리 — store 구독 → 상단 상태 라인 + 활성 워크스페이스 split tree
// 렌더 (11~12단계). 분할 렌더·splitter·탭바·클릭 포커스는 workspace-view/
// pane-view/splitter 가 담당하고, 여기는 부트스트랩과 dispatchUI 래퍼
// (CommandError 상태 라인 표면화 + D7 focus 보상)만 남는다. dev 훅
// window.__wmux 는 유지한다 — 콘솔에서 raw dispatch/getState 를 직접 부르는
// 조작 표면 (주의: dispatchUI 를 우회하므로 focus 보상·에러 표면화가 없다).

import { dispatch, getState } from "./backend";
import { formatCommandError } from "./command-error";
import { Store } from "./store";
import { WorkspaceView, activeWorkspace } from "./workspace-view";
import type { Command, CommandOutput, StateSnapshot } from "./types";

declare global {
  interface Window {
    /** dev 조작 표면 — 실 UI 미구현 경로(탭 전환·닫기 등)를 콘솔에서 호출한다. */
    __wmux: { dispatch: typeof dispatch; getState: typeof getState; reload: () => void };
  }
}

/** WebView 리로드 단축키 — Ctrl+Shift+R (브라우저 hard-reload 관례).
 *  F5 는 쓰지 않는다: 터미널에 포커스가 있으면 xterm 이 F5 를 TUI 앱용 시퀀스
 *  (`ESC[15~`)로 PTY 에 전달하는 게 올바른 동작이라(htop 등이 사용), F5 를
 *  가로채면 터미널 기능을 깬다. Ctrl+Shift+R 는 터미널 앱과 충돌하지 않는다.
 *  capture 단계 window 리스너라 xterm 포커스 상태에서도 먼저 잡힌다.
 *  세션·레이아웃은 전부 Rust 소유라 리로드는 attach 프로토콜로 복원된다 —
 *  계획 v2 12장 "WebView 리셋 안전망"의 수동 검증 경로이기도 하다. */
function installReloadKey(): void {
  window.addEventListener(
    "keydown",
    (ev) => {
      if (ev.ctrlKey && ev.shiftKey && !ev.altKey && ev.code === "KeyR") {
        ev.preventDefault();
        location.reload();
      }
    },
    { capture: true },
  );
}

/** one-shot 에러 표시 유지 시간 — 이 뒤엔 타이머로 소거한다 (폴링 금지). */
const ERROR_TTL_MS = 5000;

function requireElement(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`missing #${id} element`);
  return el;
}

/** 상태 라인 기본 텍스트 — 활성 워크스페이스 이름·pane 수·revision. */
function statusText(snapshot: StateSnapshot | null): string {
  if (snapshot === null) return "booting…";
  const ws = activeWorkspace(snapshot);
  if (ws === null) return `no workspace · rev ${snapshot.revision}`;
  const paneCount = Object.keys(ws.panes).length;
  return `workspace: ${ws.name} · panes: ${paneCount} · rev ${snapshot.revision}`;
}

class App {
  private readonly statusEl = requireElement("status-line");
  private readonly viewEl = requireElement("view");
  private readonly store = new Store();
  private readonly wsView = new WorkspaceView(this.viewEl, (cmd) => this.dispatchUI(cmd));
  private errorText: string | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;

  async init(): Promise<void> {
    // dev 훅은 부트스트랩 실패와 무관하게 먼저 노출한다 — 실패 시 콘솔에서
    // getState 로 상태를 직접 확인할 수 있어야 한다.
    window.__wmux = { dispatch, getState, reload: () => location.reload() };
    installReloadKey();
    this.store.subscribe((snapshot) => this.render(snapshot));
    await this.store.init();
  }

  /** UI 발 dispatch 공통 경로 — 실패 payload 를 formatCommandError 로 요약해
   *  상태 라인에 one-shot 표시한다 (다음 성공 dispatch 또는 ERROR_TTL_MS
   *  타이머로 소거 — 주기 폴링 없음). 성공 시 CommandOutput, 실패 시 null —
   *  에러는 이미 화면에 노출됐으므로 호출측(splitter 복원 등)은 null 분기만
   *  하면 된다. */
  private async dispatchUI(cmd: Command): Promise<CommandOutput | null> {
    try {
      const out = await dispatch(cmd);
      this.clearError();
      this.compensateFocus(cmd, out);
      return out;
    } catch (err) {
      console.error("dispatch failed", cmd, err);
      this.showError(formatCommandError(err));
      return null;
    }
  }

  /** D7 focus 보상 경로 — attach 자동 focus 를 제거했으므로, 성공한 dispatch 의
   *  결과에 따라 focus 할 뷰를 workspace-view 에 예약한다 (렌더 후 해소).
   *  새 탭 생성(TabCreated / tab 포함 PaneCreated)은 출력의 새 탭, ActivateTab
   *  은 cmd 의 탭, FocusPane 은 cmd 의 pane 의 표시 중 뷰가 대상이다. */
  private compensateFocus(cmd: Command, out: CommandOutput): void {
    if (out.type === "tabCreated") {
      this.wsView.requestFocus({ kind: "tab", tab: out.tab });
      return;
    }
    if (out.type === "paneCreated" && out.tab !== null) {
      this.wsView.requestFocus({ kind: "tab", tab: out.tab });
      return;
    }
    if (cmd.type === "activateTab") {
      this.wsView.requestFocus({ kind: "tab", tab: cmd.tab });
      return;
    }
    if (cmd.type === "focusPane") {
      this.wsView.requestFocus({ kind: "pane", pane: cmd.pane });
      return;
    }
    if (cmd.type === "closeTab" || cmd.type === "closePane" || cmd.type === "closeWorkspace") {
      // 닫힌 xterm 이 DOM 에서 사라지면 focus 가 body 로 떨어져 키 입력이 죽는다
      // (리뷰 finding) — 닫기 후 어디가 남는지는 스냅샷이 아는 것이므로, 렌더
      // 시점의 activePane 으로 보상한다.
      this.wsView.requestFocus({ kind: "activePane" });
    }
  }

  private showError(text: string): void {
    this.errorText = text;
    if (this.errorTimer !== null) clearTimeout(this.errorTimer);
    this.errorTimer = setTimeout(() => {
      this.errorTimer = null;
      this.clearError();
    }, ERROR_TTL_MS);
    this.renderStatusLine();
  }

  private clearError(): void {
    if (this.errorTimer !== null) {
      clearTimeout(this.errorTimer);
      this.errorTimer = null;
    }
    if (this.errorText === null) return; // 표시 중이 아니면 재렌더 불필요
    this.errorText = null;
    this.renderStatusLine();
  }

  private render(snapshot: StateSnapshot): void {
    this.renderStatusLine();
    this.wsView.render(snapshot);
  }

  private renderStatusLine(): void {
    const base = statusText(this.store.snapshot);
    this.statusEl.textContent =
      this.errorText === null ? base : `${base} · ERROR: ${this.errorText}`;
  }
}

async function main(): Promise<void> {
  const app = new App();
  await app.init();
}

main().catch((err) => {
  console.error("app bootstrap failed", err);
  // 부트스트랩 실패를 화면에도 노출한다 — 빈 화면으로 가리지 않는다.
  const el = document.getElementById("status-line");
  if (el !== null) el.textContent = `bootstrap failed: ${String(err)}`;
});
