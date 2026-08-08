// stats 패널 로직 — 포맷터(순수)와 1초 폴링 컨트롤러.
// 폴링은 패널이 열려 있고 문서가 보일 때만 돈다 (document.hidden이면 중단 — idle CPU 0% 원칙).

import type { SessionStats } from "./backend";

/** 바이트 수를 사람이 읽기 좋은 단위로 표기한다. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`;
  return `${(n / (1024 * 1024)).toFixed(1)}MB`;
}

/** SessionStats 한 건을 stats 패널 한 줄로 포맷한다. */
export function formatStatsRow(s: SessionStats): string {
  const last = s.last_osc === null ? "-" : s.last_osc;
  return (
    `#${s.id} ${s.alive ? "alive" : "exited"}` +
    ` out=${formatBytes(s.bytes_out)} pending=${formatBytes(s.pending)}` +
    ` paused=${s.paused} osc=${s.osc_count} last=${last}`
  );
}

export const DEFAULT_STATS_INTERVAL_MS = 1000;

export class StatsPoller {
  private timer: ReturnType<typeof setInterval> | null = null;
  private enabled = false;
  private inFlight = false;
  private readonly onVisibility = (): void => {
    this.sync();
  };

  // fetch/render/에러 처리를 주입받아 DOM 배선(main.ts)과 폴링 정책을 분리한다.
  constructor(
    private readonly fetchStats: () => Promise<SessionStats[]>,
    private readonly render: (stats: SessionStats[]) => void,
    private readonly onError: (err: unknown) => void,
    private readonly intervalMs: number = DEFAULT_STATS_INTERVAL_MS,
  ) {}

  /** 패널이 열릴 때 호출 — 보이는 상태면 즉시 1회 조회 후 주기 폴링을 시작한다. */
  start(): void {
    if (this.enabled) return;
    this.enabled = true;
    document.addEventListener("visibilitychange", this.onVisibility);
    this.sync();
  }

  /** 패널이 닫힐 때 호출 — 폴링과 visibility 구독을 멈춘다. */
  stop(): void {
    if (!this.enabled) return;
    this.enabled = false;
    document.removeEventListener("visibilitychange", this.onVisibility);
    this.sync();
  }

  /** enabled && !document.hidden 일 때만 interval이 살아 있도록 상태를 맞춘다. */
  private sync(): void {
    const shouldRun = this.enabled && !document.hidden;
    if (shouldRun && this.timer === null) {
      this.tick();
      this.timer = setInterval(() => this.tick(), this.intervalMs);
    } else if (!shouldRun && this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  private tick(): void {
    // 이전 조회가 끝나기 전에 다음 tick이 오면 건너뛴다 (겹침 방지)
    if (this.inFlight) return;
    this.inFlight = true;
    this.fetchStats()
      .then((stats) => this.render(stats))
      .catch((err) => this.onError(err))
      .finally(() => {
        this.inFlight = false;
      });
  }
}
