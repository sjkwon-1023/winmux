// 터미널 뷰 — 기존 PTY 세션에 attach 하는 xterm 1개 (12단계: 탭별 keep-alive).
//
// keep-alive 수명 (계획 D3·D7): 뷰는 탭 단위 앱 수준 레지스트리(workspace-view 의
// Map<TabId, TerminalView>)가 소유하고, 탭 전환은 dispose 가 아니라 setVisible
// (display 토글)로 처리한다 — 숨은 뷰도 채널을 유지하며 계속 ack 한다 (xterm 은
// IntersectionObserver 로 렌더만 멈추고, 재표시 시 full refresh 한다). fit 은 뷰당
// ResizeObserver 대신 pane 당 1개(pane-view 소유)가 표시 중인 뷰의 scheduleFit
// 을 부른다. attach 말미 자동 focus 는 제거됐다 — 보상 경로(부트 리컨실·생성/
// 활성화 성공 직후)는 workspace-view 의 pendingFocus 가 담당한다.
//
// attach 프로토콜 (계약은 코어 session.rs `PtySession::reattach` rustdoc):
//   1) Channel 을 **attach_terminal 호출 전에** 만들어 onmessage 큐잉을 시작한다
//      (채널 먼저·reattach 나중 — 순서를 어기면 유실 창이 생긴다).
//   2) 응답 raw body `[u64 LE end_offset][replay bytes]` 를 파싱해 replay 를
//      term.write 한다. replay 는 invoke 응답 경로라 flow 계정 대상이 아니다 —
//      ack 하지 않는다 (reattach 가 flow 를 리셋했다).
//   3) 큐잉분·이후 chunk 는 AttachGate 로 판정: offset < end_offset 폐기(dedup),
//      **폐기분 포함 전량 ack** (AckBatcher 경유 — ack 누락 시 paused 고착).
//   4) 직후 resize nudge — 항상 rows-1 → rows 2단 호출로 SIGWINCH 를 강제한다
//      (10단계 계획 0-2). 프론트는 PTY 현재 크기를 모르고, 특히 F5 리로드에서는
//      실측 == PTY 현재 크기라 동일 크기 재설정이 no-op 이 되기 때문이다.
//
// 복사/붙여넣기 키 처리는 spike terminal-tile.ts 에서 그대로 가져왔다 (Windows
// Terminal 컨벤션 — 이미 Windows 검증 완료된 코드):
// - Ctrl+V, Ctrl+Shift+V, Shift+Insert: 붙여넣기 (클립보드 → term.paste 단일 경로)
// - Ctrl+C·Ctrl+Shift+C·Ctrl+Insert (선택 있을 때만): 복사. 선택 없는 Ctrl+C 는
//   그대로 SIGINT.
// - Shift+Enter: ESC CR 재작성 (Claude Code 줄바꿈 관례 — 아래 핸들러 주석).
// - Esc 는 여기서 가로채지 않는다 — 단 **send-mode(전달 대상 선택) 활성 중에만**
//   workspace-view 의 window keydown capture 가 취소용으로 선점한다 (17단계,
//   계획 v2 3장 가로채기 목록의 의도적 확장). 평시 Esc 는 그대로 PTY 로 간다.
//   (현재 send-mode 는 UI 진입점이 없어 휴면이라 이 선점은 실제로 일어나지
//   않는다 — keys.ts 가로채기 표 참조.)
//
// 세션 수명은 dispatcher(CloseTab/ClosePane/CloseWorkspace) 소유다 — dispose 는
// 뷰만 해제하고 세션을 죽이지 않는다.

import { Terminal } from "@xterm/xterm";
import type { IDisposable } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Channel } from "@tauri-apps/api/core";
import "@xterm/xterm/css/xterm.css";

import { AckBatcher } from "./ack-batcher";
import { AttachGate } from "./attach-gate";
import type { GateResult } from "./attach-gate";
import { ackOutput, attachTerminal, detachTerminal, resizeTerminal, writeStdin } from "./backend";
import type { OutputChunk } from "./backend";
import { parseAttachBody, parseFrame } from "./frame";
import type { SessionId } from "./types";

export class TerminalView {
  readonly root: HTMLDivElement;
  private readonly term: Terminal;
  private readonly fitAddon: FitAddon;
  private readonly gate = new AttachGate();
  private readonly batcher: AckBatcher;
  private onDataSub: IDisposable | null = null;
  private onResizeSub: IDisposable | null = null;
  private disposed = false;
  private fitScheduled = false;
  private visible = true;
  private opened = false;
  /** attach(term.open) 전에 focus() 가 요청된 경우의 보류 플래그 —
   *  textarea 가 아직 없어 term.focus() 가 조용히 무시되기 때문이다. */
  private focusPending = false;
  /** replay 재생 완료 전의 onData 억제 플래그 — 낡은 단말 질의에 대한 xterm
   *  자동 응답이 PTY 로 새는 것을 막는다 (attach() 의 onData 배선 주석 참조). */
  private replayDone = false;
  /** 세션별 stdin write 직렬화 큐 (17단계 리뷰 finding) — write_stdin 은 async
   *  커맨드라 invoke 마다 별도 blocking task 로 돌아 back-to-back 쓰기(paste
   *  텍스트 + submit CR)가 역전될 수 있다. 모든 stdin write 를 이 체인으로
   *  직렬화해 발행 순서 = 도착 순서를 보장한다 (타이핑은 간격이 있어 체감 지연
   *  없음 — 왕복 1회가 겹치지 않게 될 뿐이다). */
  private writeQueue: Promise<void> = Promise.resolve();

  /** stdin write 직렬화 진입점 — onData·submit 공용. */
  private enqueueWrite(data: string): void {
    this.writeQueue = this.writeQueue
      .then(() => writeStdin(this.session, data))
      .catch((err) => console.error("write_stdin failed", err));
  }

  constructor(
    parent: HTMLElement,
    private readonly session: SessionId,
    /** 전환 계측 훅 (14단계) — replay write 완료 + rAF 1회 뒤 replay 바이트 수로
     *  1회 호출한다 (페인트 근사 완료점). 계측 중이 아닌 attach 에는 undefined —
     *  그 경우 콜백·rAF 를 아예 걸지 않아 오버헤드가 없다. */
    private readonly onTraceReplayDone?: (bytes: number) => void,
  ) {
    this.root = document.createElement("div");
    this.root.className = "term-host";
    parent.appendChild(this.root);

    this.term = new Terminal({
      scrollback: 5000,
      fontSize: 13,
      fontFamily: "Consolas, 'Cascadia Mono', monospace",
      theme: { background: "#1e1e1e" },
    });
    this.fitAddon = new FitAddon();
    this.term.loadAddon(this.fitAddon);

    this.batcher = new AckBatcher((n) => {
      this.sendAck(n);
    });
  }

  /** keep-alive 가시성 토글 — display 만 바꾼다 (채널·ack 은 계속 돈다).
   *  숨김→표시 전환 시에만 scheduleFit: 숨어 있는 동안 fit 이 크기 0 가드로
   *  스킵돼 stale 해진 dims 를 여기서 따라잡는다. */
  setVisible(v: boolean): void {
    if (this.visible === v) return;
    this.visible = v;
    this.root.style.display = v ? "" : "none";
    if (v) this.scheduleFit();
  }

  /** fit 요청 — rAF 로 프레임당 1회로 코얼레싱한다. 호출자는 pane-view 의
   *  ResizeObserver(pane 당 1개 — 계획 D7)와 setVisible 전환이다. */
  scheduleFit(): void {
    if (this.fitScheduled) return;
    this.fitScheduled = true;
    requestAnimationFrame(() => {
      this.fitScheduled = false;
      if (!this.disposed) this.fit();
    });
  }

  /** 명시적 focus (D7 보상 경로 전용 — attach 자동 focus 는 없다).
   *  term.open 전이면 보류했다가 attach 가 open 직후 적용한다. */
  focus(): void {
    if (!this.opened) {
      this.focusPending = true;
      return;
    }
    this.term.focus();
  }

  /** 현재 선택 텍스트 — 없으면 빈 문자열 (17단계 전달 소스 캡처).
   *  무선택 에러 판정은 호출측(pane-view) 몫이다.
   *
   *  이 메서드와 아래 4개는 pane 간 텍스트 전달(send-mode)의 터미널 측 표면이다.
   *  **현재 휴면 상태다** — 헤더의 ⤷/⤷⏎ 버튼을 뺀 뒤로 arm 진입점이 UI 에 없어
   *  아무도 이 경로를 부르지 않는다. 차기 agent-facing 채널이 여기에 재배선될
   *  예정이라 구현을 그대로 둔다 (pane-view 의 armSend 주석 참조). */
  getSelection(): string {
    return this.term.getSelection();
  }

  /** 패널 간 텍스트 전달 수신 경로 (17단계 D1 — 계획 v2 8장 sendText).
   *  반드시 xterm 의 term.paste 를 경유한다: bracketed paste 모드 추적을 xterm
   *  이 담당하므로 `ESC[200~` 를 raw 로 PTY 에 쓰지 않는다 (수신 앱이 paste
   *  mode 를 안 켰으면 시퀀스가 입력으로 그대로 들어가는 사고 방지). 결과
   *  바이트는 기존 onData → write_stdin 으로 흐른다.
   *
   *  onData 는 replayDone 게이트를 지나므로, 게이트가 아직 닫혀 있으면(재-attach
   *  replay 재생 중) 이 전달은 실제로 유실된다. 전달 대상은 표시 중(attach 완료)
   *  뷰뿐이라 실질적으로는 극초기 경합 창에서만 가능하지만, 조용한 유실은 금지 —
   *  발생하면 로그로 드러낸다. */
  paste(text: string): void {
    if (!this.replayDone) {
      console.warn(
        "[winmux] send-mode paste before replay done — text dropped by onData gate",
        { session: this.session, length: text.length },
      );
    }
    this.term.paste(text);
  }

  /** 전달 후 실행의 submit 절반 (계획 v2 8장 sendTextAndSubmit) — CR 1회를
   *  stdin 직렬화 큐로 보낸다. paste 텍스트(onData 경유 — 같은 큐)보다 뒤에
   *  발행되므로 순서 역전이 없다 (리뷰 finding). "전달"만으로는 절대 실행되지
   *  않는다 (실수 실행 방지). */
  submit(): void {
    this.enqueueWrite("\r");
  }

  /** 전달 수신 가능 여부 (17단계 리뷰 finding) — replay 게이트가 닫혀 있으면
   *  paste 가 onData 에서 유실되는데 submit CR 만 통과하는 비대칭이 생긴다.
   *  resolveSend 가 전달 전에 이걸로 판정해 둘 다 스킵 + 에러 표면화한다. */
  canAcceptSend(): boolean {
    return this.opened && !this.disposed && this.replayDone;
  }

  /** 대상 앱의 bracketed paste 모드 (xterm 이 추적) — 여러 줄 전달의 안전 판정.
   *  모드가 꺼진 대상(예: cmd)에 여러 줄을 paste 하면 중간 라인들이 그대로
   *  실행된다 (리뷰 finding — paste 경로가 막는 것은 stray ESC[200~ 이지 라인
   *  실행이 아니다). */
  bracketedPaste(): boolean {
    return this.term.modes.bracketedPasteMode;
  }

  /** attach 수행 — 생성 직후 정확히 1회 호출한다. 실패는 그대로 reject 로
   *  올린다 (호출자가 뷰를 정리하고 에러를 노출한다 — 가리지 않는다). */
  async attach(): Promise<void> {
    this.term.open(this.root);
    this.opened = true;
    // 초기 fit — 여기서 잡힌 실측 cols/rows 를 아래 resize nudge 에 쓴다.
    this.fit();
    this.installCopyPasteKeys();

    // 1) 채널 먼저 — 응답 도착 전에 흘러든 chunk 는 gate 가 큐잉한다.
    const channel = new Channel<OutputChunk>();
    channel.onmessage = (chunk): void => {
      this.onChunk(chunk);
    };

    // 입력 배선은 replay write 전에 건다 — 단 **replay 재생 완료 전의 onData 는
    // PTY 로 보내지 않는다** (체크포인트 1 버그 2). replay 에는 ConPTY 가 동기화용
    // 으로 넣는 단말 질의(ESC[6n 등)가 보존돼 있고, xterm 은 파싱하며 CPR
    // (`ESC[..R`) 같은 자동 응답을 onData 로 낸다 — 낡은 질의에 응답하면 그
    // 바이트가 셸 입력 줄에 stray `R` 로 남는다 (전환·리로드마다 1개씩 누적).
    // 라이브 스트림의 새 질의는 replay 파싱 완료 후 도착하므로 정상 응답된다
    // (우리 resize nudge 가 유발하는 재동기화 질의 포함).
    this.onDataSub = this.term.onData((data) => {
      if (!this.replayDone) {
        console.debug("[winmux] dropped stale terminal auto-response", data.length);
        return;
      }
      this.enqueueWrite(data);
    });

    // 2) attach → raw body [u64 LE end_offset][u8 first_attach][replay] 파싱.
    const body = await attachTerminal(this.session, channel);
    if (this.disposed) return; // attach 중 뷰가 해제됐으면 여기서 끝 (세션은 유지)
    const { endOffset, firstAttach, replay } = parseAttachBody(body);
    // 최초 attach 의 replay 질의는 라이브 질의 — 응답을 억제하면 conhost 가 CPR
    // 을 기다리며 셸이 멈춘다 (체크포인트 1 재시작 빈 화면: bytes_out=4 =
    // ESC[6n 만 출력된 채 정지, ESC[1;1R 수동 주입으로 해소된 실기 증거).
    // 재-attach 의 질의만 낡은 것이므로 그때만 replayDone 게이트로 억제한다.
    if (firstAttach) this.replayDone = true;
    const traceDone = this.onTraceReplayDone;
    if (replay.byteLength > 0) {
      const bytes = replay.byteLength;
      this.term.write(replay, () => {
        // 파싱 완료 — 이 시점부터의 onData 는 라이브 응답·사용자 입력이다.
        this.replayDone = true;
        // 전환 계측 완료점 — replay write 완료 콜백 + rAF 1회 보정 (계획 A-2:
        // 실제 페인트가 아닌 근사, report 의 approximatePaint 가 명시).
        if (traceDone !== undefined) requestAnimationFrame(() => traceDone(bytes));
      });
    } else {
      this.replayDone = true;
      // replay 가 비어도 완료점은 알린다 (rAF 보정만) — trace 가 이 탭을 기다린다.
      if (traceDone !== undefined) requestAnimationFrame(() => traceDone(0));
    }
    // 3) 게이트 개방 — 큐잉분 판정·배출 (폐기분 즉시 ack 포함).
    this.applyGateResult(this.gate.onSnapshot(endOffset));
    this.onResizeSub = this.term.onResize(({ cols, rows }) => {
      resizeTerminal(this.session, cols, rows).catch((err) =>
        console.error("resize failed", err),
      );
    });

    // 4) resize nudge — SIGWINCH 재그리기 유도 (계획 0-2). 프론트는 PTY 의 현재
    //    크기를 모른다: 신규 스폰(80×24)이든 F5 리로드(직전 attach 의 실측 크기
    //    그대로)든 "실측 == PTY 현재 크기"인 경우 동일 크기 재설정은 no-op 이라
    //    SIGWINCH 가 나가지 않는다. 그래서 조건 없이 항상 rows-1 → rows 2단으로
    //    강제한다 — 리로드 후 TUI 재그리기가 이 nudge 의 존재 이유다.
    //    Exited 세션의 replay 표시에서는 resize 가 실패할 수 있으므로 attach 전체를
    //    무효화하지 않고 에러 로그만 남긴다 (다른 배선은 이미 정상).
    try {
      const { cols, rows } = this.term;
      await resizeTerminal(this.session, cols, Math.max(1, rows - 1));
      await resizeTerminal(this.session, cols, rows);
    } catch (err) {
      console.error("resize nudge failed", err);
    }

    // 자동 focus 없음 (D7) — attach 전에 focus() 가 요청된 경우만 여기서 적용한다.
    // 숨은 상태로 attach 가 끝났으면 focus 는 버린다 (사용자가 이미 떠났다).
    if (this.focusPending) {
      this.focusPending = false;
      if (this.visible) this.term.focus();
    }
  }

  /** 뷰 해제 — 구독·xterm·배처를 정리한다. 세션은 죽이지 않는다
   *  (세션 수명은 dispatcher 소유 — 파일 상단 주석 참조). */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    // 백엔드 채널 슬롯도 분리한다 — 채널을 남겨두면 이후 출력이 Delivered 인데
    // ack 는 없는 상태로 pending 이 쌓여 백그라운드 세션이 paused 에 고착된다
    // (리뷰 finding). 분리 후 출력은 Dropped(detach 모드)로 보상 롤백되며 replay
    // 에만 쌓인다. keep-alive 에서 dispose 는 탭이 스냅샷에서 사라졌을 때
    // (view-reconcile 의 dispose 목록)와 워크스페이스 이탈 시에만 호출된다 —
    // 탭 전환은 setVisible 로 처리되므로 "dispose 직후 같은 세션 재-attach"
    // (stale detach 가 새 attach 를 밟는 경합) 표면은 사실상 사라졌다. 남는
    // 유일한 재-attach 경로는 워크스페이스 복귀인데, 그때도 invoke 는 발행 순서
    // (detach 먼저 → 이후 렌더의 attach)로 처리되고 reattach 의 flow reset 이
    // 잔여를 정리한다.
    void detachTerminal(this.session).catch((err) =>
      console.error("detach_terminal failed", err),
    );
    this.onDataSub?.dispose();
    this.onDataSub = null;
    this.onResizeSub?.dispose();
    this.onResizeSub = null;
    // 남은 ack 집계는 배출하고 끝낸다 — 백엔드 flow 계정을 맞춘 채 떠난다.
    this.batcher.dispose();
    this.term.dispose();
    this.root.remove();
  }

  private onChunk(chunk: OutputChunk): void {
    if (this.disposed) return;
    const frame = parseFrame(chunk);
    this.applyGateResult(this.gate.push(frame));
  }

  private applyGateResult(result: GateResult): void {
    for (const bytes of result.deliver) {
      if (bytes.byteLength === 0) continue;
      // ack 은 write 완료 콜백에서 집계 — 렌더 소비 속도가 flow 에 반영된다.
      this.term.write(bytes, () => {
        this.batcher.add(bytes.byteLength);
      });
    }
    // 폐기분도 수신은 했으므로 즉시 ack 집계 — 전량 ack 계약 (flow 계정 일치).
    this.batcher.add(result.discardedBytes);
  }

  private sendAck(n: number): void {
    ackOutput(this.session, n).catch((err) => console.error("ack_output failed", err));
  }

  /** 복사/붙여넣기 키 처리 — spike terminal-tile.ts 에서 그대로 이식.
   *  붙여넣기는 반드시 preventDefault 로 네이티브 paste 를 막고 클립보드 →
   *  term.paste() 단일 경로로 보낸다 — xterm 기본 처리(\x16 전송)와 네이티브
   *  paste 가 겹치면 무반응 또는 이중 붙여넣기가 된다. 복사는 선택이 있을 때만
   *  가로채고, 선택 없는 Ctrl+C 는 SIGINT 로 통과시킨다.
   *  여기에 Shift+Enter 재작성도 함께 산다 (아래 분기 주석 — 가로채기 목록은
   *  keys.ts 상단 표가 정본이다). */
  private installCopyPasteKeys(): void {
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown") return true;
      // Shift+Enter → ESC CR("\x1b\r") 재작성. 터미널 프로토콜에는 Enter 와
      // Shift+Enter 를 구분할 방법이 없어 둘 다 CR 로 나가므로, Claude Code 는
      // 줄바꿈 삽입을 별도 시퀀스로 받는다 — 그 관례가 ESC CR 이고, Claude Code
      // 의 /terminal-setup 이 VS Code·iTerm2 에 심는 키 매핑과 같은 것을 여기서
      // 터미널 자체가 제공한다. plain Enter 는 건드리지 않으므로 vim·셸 등 일반
      // 앱의 개행·실행은 그대로다 (Shift+Enter 를 따로 쓰는 앱은 사실상 없다).
      if (ev.key === "Enter" && ev.shiftKey && !ev.ctrlKey && !ev.altKey) {
        ev.preventDefault();
        this.enqueueWrite("\x1b\r");
        return false; // xterm 기본 CR 전송 차단
      }
      if (ev.key === "Insert") {
        // Windows 고전 조합 — 터미널 앱과 충돌하지 않는다.
        if (ev.shiftKey && !ev.ctrlKey && !ev.altKey) {
          ev.preventDefault();
          void this.pasteFromClipboard();
          return false;
        }
        if (ev.ctrlKey && !ev.shiftKey && !ev.altKey && this.term.hasSelection()) {
          ev.preventDefault();
          void this.copySelection();
          return false;
        }
        return true;
      }
      if (!ev.ctrlKey || ev.altKey) return true;
      const key = ev.key.toLowerCase();
      if (key === "v") {
        ev.preventDefault();
        void this.pasteFromClipboard();
        return false;
      }
      if (key === "c" && this.term.hasSelection()) {
        ev.preventDefault();
        void this.copySelection();
        return false;
      }
      return true;
    });
  }

  /** 선택 영역을 클립보드로 복사하고 선택을 해제한다 — 해제 덕에 곧바로 한 번 더
   *  누르는 Ctrl+C 는 SIGINT 로 나간다 (Windows Terminal 과 같은 동작). */
  private async copySelection(): Promise<void> {
    const text = this.term.getSelection();
    if (text.length === 0) return;
    try {
      await navigator.clipboard.writeText(text);
      this.term.clearSelection();
    } catch (err) {
      console.error("clipboard write failed", err);
    }
  }

  /** 붙여넣기 경로: 클립보드를 읽어 xterm paste 로 주입한다. WebView2 가
   *  clipboard-read 권한을 거부하는 환경이면 여기 로그가 그 증거가 되고, 그 경우
   *  Tauri clipboard-manager 플러그인 경로로 전환한다 (spike 검증 메모 승계). */
  private async pasteFromClipboard(): Promise<void> {
    try {
      const text = await navigator.clipboard.readText();
      if (text.length > 0) this.term.paste(text);
    } catch (err) {
      console.error("clipboard read failed", err);
    }
  }

  private fit(): void {
    // 크기 0인 상태에서 fit 하면 잘못된 dims 가 잡히므로 보이는 상태에서만 수행.
    if (this.root.clientWidth === 0 || this.root.clientHeight === 0) return;
    this.fitAddon.fit();
  }
}
