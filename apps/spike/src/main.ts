// spike 앱 엔트리 — 툴바·터미널 그리드·stats 패널·OSC 로그를 배선한다 (spike-plan.md 4.6).
// 앱 수준 키보드 가로채기는 0개가 기준선 — 입력은 전부 xterm 기본 경로로 PTY에 전달된다.

import { listen } from "@tauri-apps/api/event";

import { getStats, replayTerminal } from "./backend";
import type { OscEventPayload, SessionStats, TerminalExitPayload } from "./backend";
import { gridDims } from "./layout";
import { OscLog, formatOscEntry } from "./osc-log";
import { StatsPoller, formatStatsRow } from "./stats";
import { TerminalTile } from "./terminal-tile";
import type { RendererKind } from "./terminal-tile";

function requireElement(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (el === null) throw new Error(`missing #${id} element`);
  return el;
}

class App {
  private readonly toolbar = requireElement("toolbar");
  private readonly grid = requireElement("grid");
  private readonly statsEl = requireElement("status-panel");
  private readonly oscLogEl = requireElement("osc-log");
  private readonly tiles: TerminalTile[] = [];
  private readonly oscLog = new OscLog();
  private readonly statsPoller: StatsPoller;
  private renderer: RendererKind = "dom";
  private statsVisible = false;
  private rendererBtn!: HTMLButtonElement;

  constructor() {
    this.statsPoller = new StatsPoller(
      () => getStats(),
      (stats) => this.renderStats(stats),
      (err) => {
        console.error("get_stats failed", err);
        this.statsEl.textContent = `get_stats failed: ${String(err)}`;
      },
    );
  }

  async init(): Promise<void> {
    this.buildToolbar();
    // stats 패널은 닫힌 상태로 시작 — 폴링도 토글 전에는 돌지 않는다
    this.statsEl.classList.add("hidden");
    this.oscLogEl.textContent = "(no osc events)";

    await listen<OscEventPayload>("osc-event", (event) => {
      const p = event.payload;
      this.oscLog.push({
        id: p.id,
        kind: p.kind,
        title: p.title ?? "",
        body: p.body ?? "",
        at: new Date(),
      });
      this.renderOscLog();
    });
    await listen<TerminalExitPayload>("terminal-exit", (event) => {
      const tile = this.tiles.find((t) => t.id === event.payload.id);
      tile?.handleExit(event.payload.code);
    });

    // 첫 터미널 1개 자동 생성
    await this.addTerminal();
  }

  private buildToolbar(): void {
    const newBtn = document.createElement("button");
    newBtn.textContent = "New Terminal";
    newBtn.addEventListener("click", () => {
      void this.addTerminal().catch((err) => console.error("create_terminal failed", err));
    });

    this.rendererBtn = document.createElement("button");
    this.rendererBtn.textContent = "Renderer: DOM";
    this.rendererBtn.addEventListener("click", () => {
      void this.toggleRenderer().catch((err) => console.error("renderer switch failed", err));
    });

    const statsBtn = document.createElement("button");
    statsBtn.textContent = "Stats";
    statsBtn.addEventListener("click", () => {
      this.toggleStats();
    });

    // replay 커맨드(raw Response 경로)의 수동 검증 버튼 — 결과는 OSC 로그 패널에 표시.
    const replayBtn = document.createElement("button");
    replayBtn.textContent = "Replay Check";
    replayBtn.addEventListener("click", () => {
      void this.checkReplay().catch((err) => console.error("replay check failed", err));
    });

    this.toolbar.append(newBtn, this.rendererBtn, statsBtn, replayBtn);
  }

  /** 타일마다 replay 스냅샷을 받아 바이트 수를 OSC 로그에 남긴다 —
   *  raw Response 경로가 실제로 동작하는지 Windows 체크리스트에서 확인하는 용도. */
  private async checkReplay(): Promise<void> {
    for (const tile of this.tiles) {
      // 세션 생성이 끝나기 전(id 미확정) 타일은 건너뛴다.
      const id = tile.id;
      if (id === null) continue;
      const buf = await replayTerminal(id);
      this.oscLog.push({
        id,
        kind: "replay",
        title: "",
        body: `${buf.byteLength} bytes`,
        at: new Date(),
      });
    }
    this.renderOscLog();
  }

  private async addTerminal(): Promise<void> {
    this.applyGrid(this.tiles.length + 1);
    const tile = await TerminalTile.create(this.grid, this.renderer, {
      onClosed: (t) => this.removeTile(t),
      // paste 진단 등 타일의 상태 메시지를 OSC 로그 패널에 함께 표시한다
      onDiagnostic: (t, message) => {
        this.oscLog.push({ id: t.id ?? 0, kind: "diag", title: "", body: message, at: new Date() });
        this.renderOscLog();
      },
    });
    this.tiles.push(tile);
  }

  private removeTile(tile: TerminalTile): void {
    const i = this.tiles.indexOf(tile);
    if (i >= 0) this.tiles.splice(i, 1);
    this.applyGrid(Math.max(1, this.tiles.length));
  }

  /** 타일 개수에 따라 1 / 2×2 / 4×2 그리드를 적용한다. */
  private applyGrid(count: number): void {
    const { cols, rows } = gridDims(count);
    this.grid.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
    this.grid.style.gridTemplateRows = `repeat(${rows}, 1fr)`;
  }

  /** 전체 타일의 렌더러를 일괄 전환한다 (DOM ↔ WebGL). */
  private async toggleRenderer(): Promise<void> {
    this.renderer = this.renderer === "dom" ? "webgl" : "dom";
    this.rendererBtn.textContent = `Renderer: ${this.renderer === "dom" ? "DOM" : "WebGL"}`;
    await Promise.all(this.tiles.map((t) => t.setRenderer(this.renderer)));
  }

  private toggleStats(): void {
    this.statsVisible = !this.statsVisible;
    this.statsEl.classList.toggle("hidden", !this.statsVisible);
    if (this.statsVisible) {
      this.statsPoller.start();
    } else {
      this.statsPoller.stop();
    }
  }

  private renderStats(stats: SessionStats[]): void {
    this.statsEl.textContent =
      stats.length === 0 ? "no sessions" : stats.map(formatStatsRow).join("\n");
  }

  private renderOscLog(): void {
    // 최신 이벤트가 위로 오게 뒤집어 표시한다
    const lines = this.oscLog.entries.map(formatOscEntry);
    lines.reverse();
    this.oscLogEl.textContent = lines.join("\n");
  }
}

async function main(): Promise<void> {
  const app = new App();
  await app.init();
}

main().catch((err) => {
  console.error("app bootstrap failed", err);
});
