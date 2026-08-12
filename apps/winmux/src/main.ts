// winmux 앱 엔트리 — store 구독 → 좌측 워크스페이스 사이드바
// (13단계) + 우측 활성 워크스페이스 split tree 렌더 (11~12단계). 분할 렌더·
// splitter·탭바·클릭 포커스는 workspace-view/pane-view/splitter 가, 카드 리스트·
// 이름 인라인 편집은 sidebar 가 담당하고, 여기는 부트스트랩과 dispatchUI 래퍼
// (CommandError 상태 라인 표면화 + focus 보상), 새 워크스페이스 흐름(폴더 선택
// 대화상자 → CreateWorkspace — 버튼·단축키 공용), 그리고 키보드 3층 이동 글루
// (20단계 — 판정은 순수 모듈 keys.ts, 가로채기 목록도 거기가 정본)만 남는다. dev 훅
// window.__winmux 는 유지한다 — 콘솔에서 raw dispatch/getState 를 직접 부르는
// 조작 표면 (주의: dispatchUI 를 우회하므로 focus 보상·에러 표면화가 없다).

import { ActivityPing } from "./activity-ping";
import {
  dispatch,
  getState,
  getUiSettings,
  notifyToast,
  pickWorkspaceFolder,
  resetUi,
  userActivity,
} from "./backend";
import { Chime, detectNeedsInputOnset, installChimeUnlock } from "./chime";
import { formatCommandError } from "./command-error";
import {
  activeTerminalCwd,
  keyAction,
  nextTab,
  nextWorkspace,
  paneInDirection,
  pathBasename,
  workspaceAtOrdinal,
} from "./keys";
import type { KeyAction } from "./keys";
import { Sidebar } from "./sidebar";
import { Store } from "./store";
import { SwitchTracer } from "./switch-trace";
import type { SwitchReport } from "./switch-trace";
import { adjustFontSize, applyTerminalSettings, resetFontSize } from "./terminal-view";
import { applyHighlightSettings } from "./text-view";
import { initWindowVisibility } from "./window-visibility";
import { WorkspaceView, activeWorkspace } from "./workspace-view";
import type { AgentStatus, Command, CommandOutput, StateSnapshot, WorkspaceId } from "./types";

declare global {
  interface Window {
    /** dev 조작 표면 — 실 UI 미구현 경로(탭 전환·닫기 등)를 콘솔에서 호출한다. */
    __winmux: {
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

/** needsInput 토스트 본문 — 에이전트의 마지막 메시지 **첫 줄**이다. 여러 줄 메시지를
 *  통째로 넣으면 OS 가 어차피 잘라 내며 첫 줄이 가장 정보가 많고, 메시지가 없거나
 *  공백뿐이면 무엇을 알리는 알림인지는 남겨야 하므로 고정 문구로 대체한다. */
const TOAST_FALLBACK_BODY = "agent needs your input";

function toastBody(lastAgentMessage: string | null): string {
  const firstLine = (lastAgentMessage ?? "").split("\n", 1)[0].trim();
  return firstLine === "" ? TOAST_FALLBACK_BODY : firstLine;
}

function requireElement(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`missing #${id} element`);
  return el;
}

class App {
  private readonly statusEl = requireElement("status-line");
  private readonly viewEl = requireElement("view");
  private readonly store = new Store();
  /** 전환 지연 tracer (14단계) — 정착 시 report 를 콘솔·dev 훅에 노출한다.
   *  wsView 필드 초기화가 참조하므로 선언 순서상 wsView 보다 앞이어야 한다. */
  private readonly tracer = new SwitchTracer((report) => {
    console.debug("[winmux] switch", report);
    window.__winmux.lastSwitch = report;
  });
  private readonly wsView = new WorkspaceView(
    this.viewEl,
    (cmd) => this.dispatchUI(cmd),
    this.tracer,
    // send-mode 상태 라인 위임 (17단계) — 지속 프롬프트는 promptText 슬롯,
    // 캡처·전달 실패는 기존 one-shot 에러 경로를 재사용한다. (send-mode 는 arm
    // 진입점이 UI 에서 빠져 휴면이라 실제 유입은 지금 없다 — pane-view 주석.)
    {
      setPrompt: (text) => this.setPrompt(text),
      flashError: (text) => this.showError(text),
    },
  );
  private readonly sidebar = new Sidebar(
    requireElement("sidebar"),
    (cmd) => this.dispatchUI(cmd),
    // 버튼도 단축키와 같은 동작 (사용자 버그 리포트 2026-08-11: 버튼만 픽커를
    // 여는 비일관 제거) — 픽커는 createWorkspaceHere 의 무워크스페이스 폴백뿐이다.
    () => this.createWorkspaceHere(),
  );
  /** needsInput 알림음 (chime.ts) — AudioContext 는 첫 제스처 unlock 까지 lazy 다. */
  private readonly chime = new Chime();
  /** 직전 스냅샷의 워크스페이스별 agentStatus — needsInput 상승 전이 판정 기준선.
   *  부팅 첫 렌더 전까지 null 이고, 그 첫 스냅샷은 소리 없이 기준선으로만
   *  채워진다 (notifyNeedsInput 주석). */
  private agentStatuses: Map<WorkspaceId, AgentStatus> | null = null;
  private errorText: string | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;
  /** 폴더 선택 in-flight 가드 — 대화상자가 떠 있는 동안 버튼 연타·키 반복으로
   *  두 번째 대화상자가 뜨지 않게 한다. */
  private picking = false;
  /** send-mode 지속 프롬프트 (17단계) — one-shot 에러와 별개 슬롯: 모드 활성
   *  동안 유지되고 해제 시 null 이 되어 상태 라인이 다시 접힌다 (타이머 없음). */
  private promptText: string | null = null;

  async init(): Promise<void> {
    // dev 훅은 부트스트랩 실패와 무관하게 먼저 노출한다 — 실패 시 콘솔에서
    // getState 로 상태를 직접 확인할 수 있어야 한다.
    window.__winmux = {
      dispatch,
      getState,
      reload: () => location.reload(),
      resetUi,
      lastSwitch: null,
    };
    installReloadKey();
    installActivityPing();
    // 알림음 unlock — 브라우저 autoplay 정책상 사용자 제스처 전에는 소리가 나지
    // 않으므로, 첫 keydown/mousedown 에서 AudioContext 를 resume 해 둔다.
    installChimeUnlock(this.chime);
    // 창 숨김(최소화) 신호 구독 — WebView2 가 주지 않는 visibility 를 글루 창
    // 이벤트로 대체한다 (window-visibility.ts). 실패는 폴링 게이팅 신호의
    // 유실일 뿐 UI 동작과 무관하므로(최소화 중에도 폴링이 도는 기존 동작으로
    // 되돌아갈 뿐) 부트스트랩을 막지 않고 콘솔에만 남긴다 — 활동 핑과 같은 규율.
    initWindowVisibility().catch((err: unknown) => {
      console.error("window visibility listen failed", err);
    });
    this.installNavKeys();
    // UI 설정(터미널 폰트 + 뷰어 하이라이트 언어) — **store.init() 앞**이어야
    // 한다. 뷰는 첫 스냅샷 렌더부터 생기므로 그 전에 모듈 기본값을 갈아 끼워야
    // 모든 탭이 같은 설정으로 열린다 (applyTerminalSettings·applyHighlightSettings
    // 의 순서 계약). 읽기 실패(파싱 오류·범위 밖 값·모르는 언어 이름)는 사유를
    // 상태 라인에 띄우고 **기본값으로 진행한다** — 설정 하나로 부트를 막지 않는다
    // (활동 핑·창 가시성과 같은 규율). 파일이 없는 경우는 에러가 아니라 전부
    // null 인 기본값으로 온다.
    try {
      const settings = await getUiSettings();
      applyTerminalSettings(settings);
      applyHighlightSettings(settings);
    } catch (err) {
      console.error("get_ui_settings failed", err);
      this.showError(formatCommandError(err));
    }
    this.store.subscribe((snapshot) => this.render(snapshot));
    await this.store.init();
  }

  /** 키보드 3층 이동 배선 (20단계) — capture 단계 window 리스너라 xterm 보다
   *  먼저 잡는다. 가로채기 목록(keys.ts)에 든 조합이면 preventDefault +
   *  stopPropagation 으로 터미널까지 새지 않게 막고, 목록 밖이면 이벤트에 손대지
   *  않는다. 해석 결과가 없어도(범위 밖 ordinal 등) 가로채기 자체는 유지한다 —
   *  같은 키가 어떤 때는 셸에 문자를 남기는 비일관을 만들지 않기 위해서다.
   *  stopPropagation 은 같은 window 의 다른 리스너(활동 핑·send-mode Esc)를 막지
   *  않으므로 설치 순서 불변식과 무관하다 (stopImmediatePropagation 아님). */
  private installNavKeys(): void {
    window.addEventListener(
      "keydown",
      (ev) => {
        const action = keyAction({
          key: ev.key,
          ctrl: ev.ctrlKey,
          alt: ev.altKey,
          shift: ev.shiftKey,
          isComposing: ev.isComposing,
        });
        if (action === null) return;
        ev.preventDefault();
        ev.stopPropagation();
        this.runNavAction(action);
      },
      { capture: true },
    );
  }

  /** 키 액션 해석 — 최신 채택 스냅샷 기준. 대상이 없으면(스냅샷 미도착, 범위
   *  밖 ordinal, 이미 활성인 워크스페이스, 워크스페이스 1개 이하, 인접 pane 없음,
   *  탭 0~1개, 닫을 탭 없음, 이름 변경할 활성 카드 없음) 전부 조용한 no-op 이다:
   *  키보드 조작은 에러를 띄우지 않는다 (누른 키가 안 먹는 것 자체가 피드백 —
   *  단 폴더 선택은 대화상자를 여는 외부 호출이라 실패를 상태 라인에 알린다).
   *  실제 전환·포커스 보상은 dispatchUI 의 기존 경로를 그대로 탄다 —
   *  switchWorkspace 는 사이드바 클릭과 같은 activePane 보상,
   *  focusPane/activateTab 은 cmd 대상 보상, createTab/splitPane 은 새 탭 보상,
   *  closeTab 은 닫은 뒤 남는 activePane 보상. */
  private runNavAction(action: KeyAction): void {
    // 새 워크스페이스 키는 스냅샷 가드보다 앞에 둔다 — 워크스페이스가 하나도
    // 없을 때가 이 키를 가장 필요로 하는 순간이라, 그 경우엔 현재 경로가 없어
    // 픽커로 폴백한다 (사용자 결정 2026-08-11: 평시엔 대화상자 없이 활성
    // 터미널의 현재 경로로 즉시 생성).
    if (action.type === "newWorkspaceHere") {
      this.createWorkspaceHere();
      return;
    }
    if (action.type === "renameWorkspace") {
      this.sidebar.beginRename();
      return;
    }
    // 워크스페이스 닫기는 사이드바 × 버튼의 구현을 그대로 부른다 — confirm 조건
    // (실행 중인 터미널 세션 유무)과 문구가 두 벌로 갈라지지 않게 하기 위해서다.
    // 활성 워크스페이스가 없으면 사이드바 쪽에서 조용한 no-op 이 된다.
    if (action.type === "closeWorkspace") {
      this.sidebar.closeActive();
      return;
    }
    // 줌도 스냅샷 가드 앞이다 — 글꼴 크기는 모델 상태가 아니라 terminal-view 의
    // 모듈 상태라 대상 해석(활성 워크스페이스·pane)이 필요 없고, 스냅샷이 아직
    // 없는 부트 직후에도 그냥 걸린다. 세션 한정이라 dispatch 도 하지 않는다.
    if (action.type === "zoom") {
      adjustFontSize(action.delta);
      return;
    }
    if (action.type === "zoomReset") {
      resetFontSize();
      return;
    }
    const snapshot = this.store.snapshot;
    if (snapshot === null) return;
    if (action.type === "switchWorkspace") {
      const target = workspaceAtOrdinal(
        snapshot.state.workspaces.map((w) => w.id),
        action.ordinal,
      );
      if (target === null || target === snapshot.state.activeWorkspace) return;
      void this.dispatchUI({ type: "switchWorkspace", workspace: target });
      return;
    }
    if (action.type === "cycleWorkspace") {
      // 사이드바 순서 기준 이전/다음 (끝에서 순환). 워크스페이스가 1개 이하면
      // null 이라 무변경 전환을 보내지 않는다.
      const target = nextWorkspace(
        snapshot.state.workspaces.map((w) => w.id),
        snapshot.state.activeWorkspace,
        action.delta,
      );
      if (target === null) return;
      void this.dispatchUI({ type: "switchWorkspace", workspace: target });
      return;
    }
    const ws = activeWorkspace(snapshot);
    if (ws === null) return;
    if (action.type === "focusPane") {
      const target = paneInDirection(this.wsView.paneRects(), ws.activePane, action.dir);
      if (target === null) return;
      void this.dispatchUI({ type: "focusPane", pane: target });
      return;
    }
    if (action.type === "newTab") {
      // 헤더의 새 탭·폴더 아이콘과 같은 명령 — cwd/path 는 null 로 두고 코어가
      // 워크스페이스 rootPath 로 해석한다 (pane-view 주석 참조).
      const tab =
        action.kind === "terminal"
          ? ({ type: "terminal", cwd: null } as const)
          : ({ type: "folderBrowser", path: null } as const);
      void this.dispatchUI({ type: "createTab", pane: ws.activePane, tab });
      return;
    }
    if (action.type === "splitPane") {
      // 헤더의 분할 아이콘과 같은 원자 SplitPane (새 pane + 터미널 탭 동시 생성).
      void this.dispatchUI({
        type: "splitPane",
        pane: ws.activePane,
        direction: action.direction,
        tab: { type: "terminal", cwd: null },
      });
      return;
    }
    // panes 맵 키는 문자열 숫자 (JSON object 키 제약 — types.ts 참조).
    const pane = ws.panes[String(ws.activePane)];
    if (pane === undefined) return;
    if (action.type === "closeTab") {
      // 탭 × 버튼과 같은 명령 — 활성 탭이 없는 빈 pane 은 조용한 no-op.
      if (pane.activeTab === null) return;
      void this.dispatchUI({ type: "closeTab", tab: pane.activeTab });
      return;
    }
    const target = nextTab(
      pane.tabs.map((t) => t.id),
      pane.activeTab,
      action.delta,
    );
    if (target === null) return;
    void this.dispatchUI({ type: "activateTab", tab: target });
  }

  /** 새 워크스페이스 흐름 (사이드바 버튼 · Ctrl+Shift+N 공용) — Windows 네이티브
   *  폴더 선택 대화상자를 열고, 고른 폴더로 CreateWorkspace 를 원자 dispatch
   *  한다 (터미널 탭 동반 — 계획 13-D1). 이름은 폴더명, rootPath 는 변환된 리눅스
   *  경로, distro 는 UNC 선택이면 그 배포판(드라이브 선택이면 null → 백엔드
   *  기본값 해석)이다.
   *
   *  취소는 조용한 no-op(null 반환)이고, 대화상자 실패·경로 변환 실패는 상태 라인
   *  one-shot 에러다. dispatch 실패는 dispatchUI 가 이미 표면화하므로 여기서 다시
   *  다루지 않는다. */
  /** `Ctrl+Shift+N` — 활성 터미널의 현재 경로로 새 워크스페이스 즉시 생성.
   *  경로는 activeTerminalCwd(탭 cwd → 워크스페이스 rootPath) 로 해석하고,
   *  distro 는 지금 있는 워크스페이스 것을 상속한다 (같은 배포판에서 이어서
   *  일한다는 뜻이므로). 워크스페이스가 아직 없으면 픽커로 폴백 — 그때는
   *  "현재 경로" 자체가 없다. */
  private createWorkspaceHere(): void {
    const snapshot = this.store.snapshot;
    const ws = snapshot === null ? null : activeWorkspace(snapshot);
    if (ws === null) {
      void this.openWorkspacePicker();
      return;
    }
    const cwd = activeTerminalCwd(ws);
    if (cwd === null) {
      this.showError("cannot create a workspace here: current directory unknown");
      return;
    }
    // Windows 스토리지(/mnt)는 워크스페이스 루트가 될 수 없다 (사용자 결정
    // 2026-08-11 — 드라이브는 데이터를 가져올 때 뷰어로만 쓴다). 코어도 같은
    // 규칙으로 거부하지만, 여기서 먼저 잡아야 문구가 상황에 맞는다.
    if (cwd === "/mnt" || cwd.startsWith("/mnt/")) {
      this.showError(
        "cannot create a workspace under /mnt: Windows drives are data-only — cd into the WSL filesystem first",
      );
      return;
    }
    void this.dispatchUI({
      type: "createWorkspace",
      name: pathBasename(cwd),
      rootPath: cwd,
      distro: ws.distro,
      tab: { type: "terminal", cwd: null },
    });
  }

  private async openWorkspacePicker(): Promise<void> {
    if (this.picking) return;
    this.picking = true;
    try {
      const picked = await pickWorkspaceFolder();
      if (picked === null) return;
      await this.dispatchUI({
        type: "createWorkspace",
        name: picked.name,
        rootPath: picked.linux_path,
        distro: picked.distro,
        tab: { type: "terminal", cwd: null },
      });
    } catch (err) {
      // 이 커맨드는 문자열로 reject 하지만(CommandError 가 아니다) 포맷터가
      // 계약 밖 payload 도 그대로 노출하므로 같은 경로를 쓴다.
      console.error("pick_workspace_folder failed", err);
      this.showError(formatCommandError(err));
    } finally {
      this.picking = false;
    }
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

  /** send-mode 프롬프트 갱신 — null 이면 해제(바가 다시 접힌다). 무변경 재렌더는
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
    this.notifyNeedsInput(snapshot);
    this.sidebar.render(snapshot);
    this.wsView.render(snapshot);
    // 렌더 말미 봉인 — 이 렌더에서 attach 를 시작한 탭이 없으면(터미널 0개·전부
    // keep-alive 재사용) 여기서 즉시 정착한다 (계획 A-2).
    this.tracer.settle();
  }

  /** needsInput 알림 트리거 (실기 결함: 에이전트가 입력을 기다리는데 소리가 전혀
   *  없어 대기를 놓친다) — 어느 워크스페이스든 needsInput 이 **아니었다가**
   *  needsInput 이 된 순간에만 1회 울린다. 판정은 순수 함수 detectNeedsInputOnset
   *  몫이고(같은 상태 반복·running/idle 전환은 무음 — 소음 방지) 여기는 기준선
   *  보관과 재생·토스트 배선만 한다.
   *
   *  같은 전이에 신호가 둘이고 조건이 다르다: **차임은 항상**(포커스 여부 무관,
   *  여러 워크스페이스가 동시에 전이해도 1회), **토스트는 비포커스일 때만**
   *  (워크스페이스마다 1건). 아래 분기의 주석이 그 이유를 담는다.
   *
   *  부팅 첫 스냅샷은 기준선으로만 쓴다(prev=null → 무음). 재시작 복원은 코어
   *  sanitize 가 agent_status 를 Idle 로 되돌리므로 자연히 무음이지만, WebView
   *  리로드·자동 리셋에서는 살아 있는 세션의 needsInput 이 그대로 첫 스냅샷에
   *  실려 오므로 "전이"가 아닌 것에 울리지 않게 명시적으로 기준선 취급한다. */
  private notifyNeedsInput(snapshot: StateSnapshot): void {
    const { chime, onsets, next } = detectNeedsInputOnset(
      this.agentStatuses,
      snapshot.state.workspaces,
    );
    this.agentStatuses = next;
    if (chime) this.chime.play();
    // OS 토스트는 **창이 비포커스일 때만** (백로그 2026-08-11). 창을 보고 있는
    // 사용자에게는 사이드바 강조 + 차임이면 충분하고, 그 위에 토스트까지 뜨면
    // 방해다 — 토스트의 값은 "winmux 를 안 보고 있을 때 불러 준다"에 있다.
    // document.hasFocus() 를 쓰는 이유: 백엔드의 창 포커스 신호는 자동 리셋 정책
    // 전용 경로(main.rs on_window_event)라 프론트로 오지 않고, 사실 자체는 WebView
    // 안에서 그대로 읽힌다. 차임과 달리 워크스페이스마다 1건이다 — 어느 프로젝트가
    // 기다리는지가 알림의 내용이라 합칠 수 없다.
    if (onsets.length === 0 || document.hasFocus()) return;
    const pending = new Set(onsets);
    for (const ws of snapshot.state.workspaces) {
      if (!pending.has(ws.id)) continue;
      // 실패는 console 로만 — 알림은 차임과 같은 부가 신호라 UI 동작을 막지 않는다
      // (알림 차단·플러그인 실패 어느 쪽이든 앱은 그대로 돈다).
      notifyToast(`winmux — ${ws.name}`, toastBody(ws.lastAgentMessage)).catch((err) => {
        console.debug("[winmux] needsInput toast failed", err);
      });
    }
  }

  /** 상태 라인 조립 — [send-mode 프롬프트] · [one-shot 에러]. **일시 표시**다:
   *  둘 다 없으면 바를 hidden 으로 접어 그만큼 터미널 영역이 넓어진다 (상시
   *  로그를 띄우지 않는다). 에러가 섞이면 error 클래스로 색을 구분한다. */
  private renderStatusLine(): void {
    const parts: string[] = [];
    if (this.promptText !== null) parts.push(this.promptText);
    if (this.errorText !== null) parts.push(`ERROR: ${this.errorText}`);
    this.statusEl.textContent = parts.join(" · ");
    this.statusEl.classList.toggle("error", this.errorText !== null);
    this.statusEl.hidden = parts.length === 0;
  }
}

async function main(): Promise<void> {
  const app = new App();
  await app.init();
}

main().catch((err) => {
  console.error("app bootstrap failed", err);
  // 부트스트랩 실패를 화면에도 노출한다 — 빈 화면으로 가리지 않는다. 상태 라인은
  // 평시 접혀 있으므로(index.html 의 hidden) 여기서 직접 펼친다.
  const el = document.getElementById("status-line");
  if (el !== null) {
    el.textContent = `bootstrap failed: ${String(err)}`;
    el.classList.add("error");
    el.hidden = false;
  }
});
