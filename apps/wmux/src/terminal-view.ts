// 터미널 뷰 — 기존 PTY 세션에 attach 하는 xterm 1개 (10단계: 활성 탭 1개 전면 렌더).
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
import { parseFrame } from "./frame";
import type { SessionId } from "./types";

export class TerminalView {
  readonly root: HTMLDivElement;
  private readonly term: Terminal;
  private readonly fitAddon: FitAddon;
  private readonly gate = new AttachGate();
  private readonly batcher: AckBatcher;
  private readonly resizeObserver: ResizeObserver;
  private onDataSub: IDisposable | null = null;
  private onResizeSub: IDisposable | null = null;
  private disposed = false;
  private fitScheduled = false;

  constructor(
    parent: HTMLElement,
    private readonly session: SessionId,
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

    this.resizeObserver = new ResizeObserver(() => {
      // 연쇄 리사이즈를 프레임당 1회 fit 으로 합친다.
      if (this.fitScheduled) return;
      this.fitScheduled = true;
      requestAnimationFrame(() => {
        this.fitScheduled = false;
        if (!this.disposed) this.fit();
      });
    });
  }

  /** attach 수행 — 생성 직후 정확히 1회 호출한다. 실패는 그대로 reject 로
   *  올린다 (호출자가 뷰를 정리하고 에러를 노출한다 — 가리지 않는다). */
  async attach(): Promise<void> {
    this.term.open(this.root);
    // 초기 fit — 여기서 잡힌 실측 cols/rows 를 아래 resize nudge 에 쓴다.
    this.fit();
    this.installCopyPasteKeys();

    // 1) 채널 먼저 — 응답 도착 전에 흘러든 chunk 는 gate 가 큐잉한다.
    const channel = new Channel<OutputChunk>();
    channel.onmessage = (chunk): void => {
      this.onChunk(chunk);
    };

    // 2) attach → raw body [u64 LE end_offset][replay bytes] 파싱.
    const body = await attachTerminal(this.session, channel);
    if (this.disposed) return; // attach 중 뷰가 해제됐으면 여기서 끝 (세션은 유지)
    const { offset: endOffset, bytes: replay } = parseFrame(body);
    if (replay.byteLength > 0) this.term.write(replay);
    // 3) 게이트 개방 — 큐잉분 판정·배출 (폐기분 즉시 ack 포함).
    this.applyGateResult(this.gate.onSnapshot(endOffset));

    // 입력 배선 — 가로채기 키 목록 외 입력은 전부 xterm 기본 onData 경로로 PTY 에 간다.
    this.onDataSub = this.term.onData((data) => {
      writeStdin(this.session, data).catch((err) => console.error("write_stdin failed", err));
    });
    this.onResizeSub = this.term.onResize(({ cols, rows }) => {
      resizeTerminal(this.session, cols, rows).catch((err) =>
        console.error("resize failed", err),
      );
    });
    this.resizeObserver.observe(this.root);

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

    this.term.focus();
  }

  /** 뷰 해제 — 옵저버·구독·xterm·배처를 정리한다. 세션은 죽이지 않는다
   *  (세션 수명은 dispatcher 소유 — 파일 상단 주석 참조). */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    // 백엔드 채널 슬롯도 분리한다 — 채널을 남겨두면 이후 출력이 Delivered 인데
    // ack 는 없는 상태로 pending 이 쌓여 백그라운드 세션이 paused 에 고착된다
    // (리뷰 finding). 분리 후 출력은 Dropped(detach 모드)로 보상 롤백되며 replay
    // 에만 쌓인다. 분리 전 in-flight 잔여 chunk 몇 개의 pending 은 high_water 에
    // 한참 못 미치고 다음 attach 의 flow reset 으로 정리된다.
    void detachTerminal(this.session).catch((err) =>
      console.error("detach_terminal failed", err),
    );
    this.resizeObserver.disconnect();
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
   *  가로채고, 선택 없는 Ctrl+C 는 SIGINT 로 통과시킨다. */
  private installCopyPasteKeys(): void {
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown") return true;
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
