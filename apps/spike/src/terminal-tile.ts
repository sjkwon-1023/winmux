// 터미널 타일 — xterm 인스턴스 1개와 백엔드 PTY 세션 1개를 묶는다 (spike-plan.md 4.6).
// 출력 수신(raw channel) → term.write → 완료 콜백에서 AckBatcher 집계 → ack_output,
// 입력은 xterm 기본 onData 경로 그대로 write_stdin (앱 단축키 가로채기 없음).

import { Terminal } from "@xterm/xterm";
import type { IDisposable } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { WebglAddon } from "@xterm/addon-webgl";
import { Channel } from "@tauri-apps/api/core";
import "@xterm/xterm/css/xterm.css";

import { AckBatcher } from "./ack-batcher";
import {
  ackOutput,
  closeTerminal,
  createTerminal,
  resizeTerminal,
  writeStdin,
} from "./backend";
import type { OutputChunk } from "./backend";

export type RendererKind = "dom" | "webgl";

export interface TerminalTileEvents {
  /** 닫기 버튼으로 dispose가 끝난 뒤 호출 — 앱이 그리드에서 타일을 제거한다. */
  onClosed(tile: TerminalTile): void;
  /** 진단 메시지 — paste 경로 등 Windows 재검증에서 봐야 하는 상태를 UI 로그로 올린다. */
  onDiagnostic(tile: TerminalTile, message: string): void;
}

export class TerminalTile {
  readonly root: HTMLDivElement;
  private readonly host: HTMLDivElement;
  private readonly titleEl: HTMLSpanElement;
  private readonly term: Terminal;
  private readonly fitAddon: FitAddon;
  private readonly batcher: AckBatcher;
  private readonly resizeObserver: ResizeObserver;
  private webgl: WebglAddon | null = null;
  private renderer: RendererKind = "dom";
  private sessionId: number | null = null;
  private ready: Promise<number> | null = null;
  private onDataSub: IDisposable | null = null;
  private onResizeSub: IDisposable | null = null;
  private alive = true;
  private disposed = false;
  private fitScheduled = false;

  /** 타일을 만들고 parent에 붙인 뒤 백엔드 세션까지 연결한다. */
  static async create(
    parent: HTMLElement,
    renderer: RendererKind,
    events: TerminalTileEvents,
  ): Promise<TerminalTile> {
    const tile = new TerminalTile(parent, events);
    await tile.init(renderer);
    return tile;
  }

  private constructor(
    parent: HTMLElement,
    private readonly events: TerminalTileEvents,
  ) {
    this.root = document.createElement("div");
    this.root.className = "tile";

    const header = document.createElement("div");
    header.className = "tile-header";
    this.titleEl = document.createElement("span");
    this.titleEl.className = "tile-title";
    this.titleEl.textContent = "#?";
    const closeBtn = document.createElement("button");
    closeBtn.className = "tile-close";
    closeBtn.textContent = "×";
    closeBtn.title = "close terminal";
    closeBtn.addEventListener("click", () => {
      void this.dispose().then(() => this.events.onClosed(this));
    });
    header.append(this.titleEl, closeBtn);

    this.host = document.createElement("div");
    this.host.className = "term-host";
    this.root.append(header, this.host);
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
      // 연쇄 리사이즈를 프레임당 1회 fit으로 합친다
      if (this.fitScheduled) return;
      this.fitScheduled = true;
      requestAnimationFrame(() => {
        this.fitScheduled = false;
        if (!this.disposed) this.fit();
      });
    });
  }

  private async init(renderer: RendererKind): Promise<void> {
    this.term.open(this.host);
    // 초기 fit — 여기서 잡힌 cols/rows를 create_terminal에 그대로 전달한다
    this.fit();

    // Ctrl+V / Ctrl+Shift+V — WebView2에서 브라우저 기본 paste accelerator가 동작하지
    // 않는 문제(ADR-0001 known issue)의 우회: 클립보드를 직접 읽어 xterm의 paste
    // 경로로 넣는다. term.paste()는 앱이 켠 bracketed paste 모드 상태를 존중한다.
    this.term.attachCustomKeyEventHandler((ev) => {
      if (ev.type === "keydown" && ev.ctrlKey && !ev.altKey && (ev.key === "v" || ev.key === "V")) {
        void this.pasteFromClipboard();
        return false;
      }
      return true;
    });

    // 우클릭/네이티브 paste 진단 — `^[[200~` 노출 원인((a) ConPTY 입력 변형 vs
    // (b) TUI 종료 후 stale mode) 구분용. paste 순간의 모드 상태를 로그로 남긴다.
    this.host.addEventListener("paste", (ev) => {
      const len = ev.clipboardData?.getData("text").length ?? 0;
      this.diag(`paste(native): ${len} chars, bracketed=${String(this.term.modes.bracketedPasteMode)}`);
    });

    const channel = new Channel<OutputChunk>();
    channel.onmessage = (chunk): void => {
      this.onOutput(chunk);
    };

    // 채널 메시지가 invoke 응답보다 먼저 도착할 수 있으므로, ack 배출은
    // sessionId 대신 이 promise를 통해 id를 기다린다 (sendAck 참조).
    const ready = createTerminal(this.term.cols, this.term.rows, channel);
    this.ready = ready;
    const id = await ready;
    this.sessionId = id;
    this.titleEl.textContent = `#${id}`;

    // 입력은 xterm 기본 onData 경로 — Ctrl+C 등 제어 입력도 그대로 PTY로 간다
    this.onDataSub = this.term.onData((data) => {
      if (!this.alive) return;
      writeStdin(id, data).catch((err) => console.error("write_stdin failed", err));
    });
    this.onResizeSub = this.term.onResize(({ cols, rows }) => {
      resizeTerminal(id, cols, rows).catch((err) => console.error("resize failed", err));
    });
    this.resizeObserver.observe(this.root);

    if (renderer === "webgl") await this.setRenderer("webgl");
    this.term.focus();
  }

  /** 백엔드가 부여한 세션 id. init 완료 전에는 null. */
  get id(): number | null {
    return this.sessionId;
  }

  get isAlive(): boolean {
    return this.alive;
  }

  get currentRenderer(): RendererKind {
    return this.renderer;
  }

  /** DOM/WebGL 렌더러 런타임 전환. WebGL addon은 필요 시점에 동적 로드하고,
   *  dispose하면 xterm이 DOM 렌더러로 복귀한다. */
  async setRenderer(kind: RendererKind): Promise<void> {
    if (this.disposed || kind === this.renderer) return;
    if (kind === "webgl") {
      const { WebglAddon } = await import("@xterm/addon-webgl");
      const addon = new WebglAddon();
      addon.onContextLoss(() => {
        // 컨텍스트 유실 시 addon을 버리고 DOM 렌더러로 복귀 (xterm 권장 처리)
        console.error("webgl context lost; falling back to DOM renderer");
        addon.dispose();
        this.webgl = null;
        this.renderer = "dom";
      });
      this.term.loadAddon(addon);
      this.webgl = addon;
    } else {
      this.webgl?.dispose();
      this.webgl = null;
    }
    this.renderer = kind;
  }

  /** "terminal-exit" 이벤트 수신 시 호출 — 세션 종료를 표시하고 입력·ack를 멈춘다. */
  handleExit(code: number | null): void {
    if (!this.alive) return;
    // 세션이 끝났으므로 남은 ack 집계는 버린다 (sendAck의 alive 가드)
    this.alive = false;
    this.batcher.flush();
    const msg =
      code === null
        ? "\r\n[process exited]\r\n"
        : `\r\n[process exited with code ${code}]\r\n`;
    this.term.write(msg);
    this.titleEl.textContent = `${this.titleEl.textContent} (exited)`;
  }

  /** 타일 정리 — 옵저버·addon·xterm·배처를 해제하고 살아 있는 세션은 닫는다. */
  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    const id = this.sessionId;
    const wasAlive = this.alive;
    this.alive = false;
    this.resizeObserver.disconnect();
    this.onDataSub?.dispose();
    this.onDataSub = null;
    this.onResizeSub?.dispose();
    this.onResizeSub = null;
    this.webgl?.dispose();
    this.webgl = null;
    this.batcher.dispose();
    this.term.dispose();
    this.root.remove();
    if (id !== null && wasAlive) {
      try {
        await closeTerminal(id);
      } catch (err) {
        console.error("close_terminal failed", err);
      }
    }
  }

  /** Ctrl+V 경로: 클립보드를 읽어 xterm paste로 주입한다. 실패도 진단 로그로 남긴다 —
   *  WebView2가 clipboard-read 권한을 거부하는 환경이면 여기 로그가 그 증거가 되고,
   *  그 경우 Tauri clipboard-manager 플러그인 경로로 전환한다. */
  private async pasteFromClipboard(): Promise<void> {
    try {
      const text = await navigator.clipboard.readText();
      this.diag(
        `paste(ctrl+v): ${text.length} chars, bracketed=${String(this.term.modes.bracketedPasteMode)}`,
      );
      if (text.length > 0) this.term.paste(text);
    } catch (err) {
      this.diag(`paste(ctrl+v) clipboard read failed: ${String(err)}`);
    }
  }

  private diag(message: string): void {
    this.events.onDiagnostic(this, message);
  }

  private onOutput(chunk: OutputChunk): void {
    if (this.disposed) return;
    // raw channel은 ArrayBuffer를 주지만, 구현 차이에 방어적으로 Uint8Array도 수용
    const bytes = chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
    if (bytes.byteLength === 0) return;
    this.term.write(bytes, () => {
      this.batcher.add(bytes.byteLength);
    });
  }

  private sendAck(n: number): void {
    // 세션이 죽었으면 flow control이 무의미하므로 ack를 보내지 않는다
    if (!this.alive) return;
    const ready = this.ready;
    if (ready === null) return;
    ready
      .then((id) => ackOutput(id, n))
      .catch((err) => console.error("ack_output failed", err));
  }

  private fit(): void {
    // 크기 0인 상태에서 fit하면 잘못된 dims가 잡히므로 보이는 상태에서만 수행
    if (this.host.clientWidth === 0 || this.host.clientHeight === 0) return;
    this.fitAddon.fit();
  }
}
