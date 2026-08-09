// wmux 앱 엔트리 — store 구독 → 상단 상태 라인 + 좌측 워크스페이스 사이드바
// (13단계) + 우측 활성 워크스페이스 split tree 렌더 (11~12단계). 분할 렌더·
// splitter·탭바·클릭 포커스는 workspace-view/pane-view/splitter 가, 카드 리스트·
// 인라인 폼은 sidebar 가 담당하고, 여기는 부트스트랩과 dispatchUI 래퍼
// (CommandError 상태 라인 표면화 + focus 보상)만 남는다. dev 훅
// window.__wmux 는 유지한다 — 콘솔에서 raw dispatch/getState 를 직접 부르는
// 조작 표면 (주의: dispatchUI 를 우회하므로 focus 보상·에러 표면화가 없다).

import { ActivityPing } from "./activity-ping";
import { dispatch, getState, resetUi, userActivity } from "./backend";
import { formatCommandError } from "./command-error";
import { Sidebar } from "./sidebar";
import { Store } from "./store";
import { SwitchTracer } from "./switch-trace";
import type { SwitchReport } from "./switch-trace";
import { WorkspaceView, activeWorkspace } from "./workspace-view";
import type { Command, CommandOutput, StateSnapshot } from "./types";

declare global {
  interface Window {
    /** dev 조작 표면 — 실 UI 미구현 경로(탭 전환·닫기 등)를 콘솔에서 호출한다. */
    __wmux: {
      dispatch: typeof dispatch;
      getState: typeof getState;
      reload: () => void;
      /** 수동 WebView 리셋 (16단계) — 백엔드 perform_reset 경유라 자동 리셋과
       *  같은 경로를 검증한다. dev 훅 전용 — UI 버튼 금지 (계획 v2 12장). */
      resetUi: typeof resetUi;
      /** 마지막 워크스페이스 전환 계측 report (14단계) — 정착 전에는 null. */
      lastSwitch: SwitchReport | null;
    };
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

/** 활동 핑 배선 (16단계 C-3) — capture 단계 window 리스너로 wheel/mousedown/
 *  keydown 을 잡아 throttled user_activity 를 보낸다 (xterm 포커스 상태에서도
 *  window 까지 버블·캡처가 도달한다). visibilitychange 는 즉시 보조 신호.
 *  invoke 실패는 리셋 안전망의 신호 유실일 뿐 UI 동작과 무관 — 콘솔에만 남긴다.
 *  throttle 로직 자체는 activity-ping.ts (순수, vitest 대상). */
function installActivityPing(): void {
  const ping = new ActivityPing((visible) => {
    userActivity(visible).catch((err) => console.error("user_activity failed", err));
  });
  const onActivity = () => ping.activity(performance.now());
  window.addEventListener("wheel", onActivity, { capture: true, passive: true });
  window.addEventListener("mousedown", onActivity, { capture: true });
  window.addEventListener("keydown", onActivity, { capture: true });
  document.addEventListener("visibilitychange", () => {
    ping.visibility(document.visibilityState === "visible", performance.now());
  });
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
  /** 전환 지연 tracer (14단계) — 정착 시 report 를 콘솔·dev 훅에 노출한다.
   *  wsView 필드 초기화가 참조하므로 선언 순서상 wsView 보다 앞이어야 한다. */
  private readonly tracer = new SwitchTracer((report) => {
    console.debug("[wmux] switch", report);
    window.__wmux.lastSwitch = report;
  });
  private readonly wsView = new WorkspaceView(
    this.viewEl,
    (cmd) => this.dispatchUI(cmd),
    this.tracer,
    // send-mode 상태 라인 위임 (17단계) — 지속 프롬프트는 promptText 슬롯,
    // 캡처·전달 실패는 기존 one-shot 에러 경로를 재사용한다.
    {
      setPrompt: (text) => this.setPrompt(text),
      flashError: (text) => this.showError(text),
    },
  );
  private readonly sidebar = new Sidebar(requireElement("sidebar"), (cmd) => this.dispatchUI(cmd));
  private errorText: string | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;
  /** send-mode 지속 프롬프트 (17단계) — one-shot 에러와 별개 슬롯: 모드 활성
   *  동안 유지되고 해제 시 null 로 기본 상태 라인이 복원된다 (타이머 없음). */
  private promptText: string | null = null;

  async init(): Promise<void> {
    // dev 훅은 부트스트랩 실패와 무관하게 먼저 노출한다 — 실패 시 콘솔에서
    // getState 로 상태를 직접 확인할 수 있어야 한다.
    window.__wmux = {
      dispatch,
      getState,
      reload: () => location.reload(),
      resetUi,
      lastSwitch: null,
    };
    installReloadKey();
    installActivityPing();
    this.store.subscribe((snapshot) => this.render(snapshot));
    await this.store.init();
  }

  /** UI 발 dispatch 공통 경로 — 실패 payload 를 formatCommandError 로 요약해
   *  상태 라인에 one-shot 표시한다 (다음 성공 dispatch 또는 ERROR_TTL_MS
   *  타이머로 소거 — 주기 폴링 없음). 성공 시 CommandOutput, 실패 시 null —
   *  에러는 이미 화면에 노출됐으므로 호출측(splitter 복원 등)은 null 분기만
   *  하면 된다. */
  private async dispatchUI(cmd: Command): Promise<CommandOutput | null> {
    // 전환 계측 t0 = dispatch 시각 (계획 A-2). begin 은 await **앞**이어야 한다 —
    // state-changed 이벤트가 invoke 응답보다 먼저 처리되는 순서(workspace-view
    // 주석의 기지 race)에서 전환 스냅샷 렌더의 markSnapshot 을 놓치지 않기
    // 위해서다. 실패하면 catch 에서 토큰으로 폐기한다. 이미 활성인 워크스페이스로의
    // 전환은 계측하지 않는다 — 새 전환 스냅샷이 없어 이후 무관 렌더의 시각으로
    // 오염된 report 가 나온다 (리뷰 finding).
    let traceToken: number | null = null;
    if (cmd.type === "switchWorkspace") {
      const active = this.store.snapshot?.state.activeWorkspace ?? null;
      if (active !== cmd.workspace) {
        traceToken = this.tracer.begin(cmd.workspace, performance.now());
      }
    }
    try {
      const out = await dispatch(cmd);
      this.clearError();
      this.compensateFocus(cmd, out);
      return out;
    } catch (err) {
      if (traceToken !== null) this.tracer.discard(traceToken);
      console.error("dispatch failed", cmd, err);
      this.showError(formatCommandError(err));
      return null;
    }
  }

  /** focus 보상 경로 (12단계 D7 + 13단계 D5) — attach 자동 focus 를 제거했으므로,
   *  성공한 dispatch 의 결과에 따라 focus 할 뷰를 workspace-view 에 예약한다
   *  (렌더 후 해소). 새 탭 생성(TabCreated / tab 포함 PaneCreated / tab 포함
   *  WorkspaceCreated)은 출력의 새 탭, ActivateTab 은 cmd 의 탭, FocusPane 은
   *  cmd 의 pane 의 표시 중 뷰가 대상이다. */
  private compensateFocus(cmd: Command, out: CommandOutput): void {
    if (out.type === "tabCreated") {
      this.wsView.requestFocus({ kind: "tab", tab: out.tab });
      return;
    }
    if (out.type === "paneCreated" && out.tab !== null) {
      this.wsView.requestFocus({ kind: "tab", tab: out.tab });
      return;
    }
    if (out.type === "workspaceCreated" && out.tab !== null) {
      // 새 워크스페이스는 활성이 된다 (코어 CreateWorkspace 시맨틱) — 원자
      // 생성된 터미널 탭에 focus (13단계 D5, 기존 tabCreated 분기와 동일 패턴).
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
    if (
      cmd.type === "closeTab" ||
      cmd.type === "closePane" ||
      cmd.type === "closeWorkspace" ||
      cmd.type === "switchWorkspace"
    ) {
      // 닫기: 닫힌 xterm 이 DOM 에서 사라지면 focus 가 body 로 떨어져 키 입력이
      // 죽는다 (리뷰 finding) — 닫기 후 어디가 남는지는 스냅샷이 아는 것이므로,
      // 렌더 시점의 activePane 으로 보상한다. 전환(13단계 D5)도 같은 경로다 —
      // 전환 후 활성 pane 은 도착 스냅샷이 알려준다.
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

  /** send-mode 프롬프트 갱신 — null 이면 해제(기본 표시 복원). 무변경 재렌더는
   *  건너뛴다 (arm 덮어쓰기 등에서 같은 문자열이 반복 유입될 수 있다). */
  private setPrompt(text: string | null): void {
    if (this.promptText === text) return;
    this.promptText = text;
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
    // 전환 스냅샷 도착 시각 — wsView.render(attach 시작 기록)보다 먼저 찍는다.
    // 이 콜백은 store 구독이라 offer 채택 직후 동기 호출된다 (스냅샷 시각과 동일).
    this.tracer.markSnapshot(snapshot.state.activeWorkspace, performance.now());
    this.renderStatusLine();
    this.sidebar.render(snapshot);
    this.wsView.render(snapshot);
    // 렌더 말미 봉인 — 이 렌더에서 attach 를 시작한 탭이 없으면(터미널 0개·전부
    // keep-alive 재사용) 여기서 즉시 정착한다 (계획 A-2).
    this.tracer.settle();
  }

  /** 상태 라인 조립 — 기본 텍스트 · [send-mode 프롬프트] · [one-shot 에러]. */
  private renderStatusLine(): void {
    const parts = [statusText(this.store.snapshot)];
    if (this.promptText !== null) parts.push(this.promptText);
    if (this.errorText !== null) parts.push(`ERROR: ${this.errorText}`);
    this.statusEl.textContent = parts.join(" · ");
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
