// OSC 이벤트 로그 — 최근 N건(기본 100)만 보관하는 순수 모델과 표시용 포맷터.
// DOM 렌더링은 main.ts가 담당하고, 이 모듈은 vitest로 검증 가능한 순수 로직만 둔다.

export const DEFAULT_OSC_LOG_CAP = 100;

export interface OscLogEntry {
  id: number;
  kind: string;
  title: string;
  body: string;
  at: Date;
}

export class OscLog {
  private readonly items: OscLogEntry[] = [];

  constructor(private readonly cap: number = DEFAULT_OSC_LOG_CAP) {
    if (cap <= 0) throw new Error("OscLog cap must be positive");
  }

  /** 항목을 추가하고, cap을 넘으면 오래된 것부터 버린다. */
  push(entry: OscLogEntry): void {
    this.items.push(entry);
    if (this.items.length > this.cap) {
      this.items.splice(0, this.items.length - this.cap);
    }
  }

  /** 보관 중인 항목 — 오래된 것부터 최신 순. */
  get entries(): readonly OscLogEntry[] {
    return this.items;
  }
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** "[HH:MM:SS] #id kind title="…" body="…"" 형태로 포맷한다.
 *  title/body는 비어 있으면 생략하고, 제어 문자가 로그를 깨지 않도록
 *  JSON 문자열 이스케이프를 그대로 쓴다. */
export function formatOscEntry(entry: OscLogEntry): string {
  const time = `${pad2(entry.at.getHours())}:${pad2(entry.at.getMinutes())}:${pad2(entry.at.getSeconds())}`;
  let line = `[${time}] #${entry.id} ${entry.kind}`;
  if (entry.title !== "") line += ` title=${JSON.stringify(entry.title)}`;
  if (entry.body !== "") line += ` body=${JSON.stringify(entry.body)}`;
  return line;
}
