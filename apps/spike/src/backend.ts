// Tauri 백엔드 커맨드·이벤트 계약 래퍼 — spike-plan.md 4.5의 계약을 타입으로 고정한다.
// 커맨드 인자 키는 Tauri v2 기본 규칙(JS camelCase → Rust snake_case)을 따른다.

import { Channel, invoke } from "@tauri-apps/api/core";

/** src-tauri SessionStats 직렬화 형태 (serde 기본 — Rust 필드명 snake_case 그대로). */
export interface SessionStats {
  id: number;
  bytes_out: number;
  pending: number;
  paused: boolean;
  osc_count: number;
  last_osc: string | null;
  alive: boolean;
}

/** "osc-event" 이벤트 payload. title/body는 이벤트 종류에 따라 비어 있거나 없을 수 있다. */
export interface OscEventPayload {
  id: number;
  kind: string;
  title?: string | null;
  body?: string | null;
}

/** "terminal-exit" 이벤트 payload. 종료 코드를 얻지 못하면 code는 null. */
export interface TerminalExitPayload {
  id: number;
  code: number | null;
}

/** 터미널 출력 채널 메시지 — raw channel은 ArrayBuffer를 주지만, 구현 차이에 대비해
 *  Uint8Array도 수용한다. 소비 측에서 Uint8Array로 정규화한다. */
export type OutputChunk = ArrayBuffer | Uint8Array;

/** PTY 세션을 만들고 세션 id를 돌려받는다. 출력은 onOutput 채널로 raw 바이너리 수신. */
export function createTerminal(
  cols: number,
  rows: number,
  onOutput: Channel<OutputChunk>,
): Promise<number> {
  return invoke<number>("create_terminal", { cols, rows, onOutput });
}

/** 사용자 입력(문자열)을 PTY stdin으로 보낸다. */
export function writeStdin(id: number, data: string): Promise<void> {
  return invoke<void>("write_stdin", { id, data });
}

/** 임의 바이트열을 PTY stdin으로 보낸다 (제어 시퀀스 테스트용). */
export function sendRaw(id: number, bytes: number[]): Promise<void> {
  return invoke<void>("send_raw", { id, bytes });
}

/** PTY 크기 변경. */
export function resizeTerminal(id: number, cols: number, rows: number): Promise<void> {
  return invoke<void>("resize", { id, cols, rows });
}

/** flow control ack — 프론트가 소비 완료한 바이트 수를 백엔드에 알린다. */
export function ackOutput(id: number, n: number): Promise<void> {
  return invoke<void>("ack_output", { id, n });
}

/** replay buffer 스냅샷 조회 — 터미널 출력과 같은 raw body(ArrayBuffer) 경로로 받는다.
 *  Spike에서는 teardown/재구성 실험 전 단계로, raw Response 경로 자체의 검증에 쓴다. */
export function replayTerminal(id: number): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("replay", { id });
}

/** 세션 종료·정리. */
export function closeTerminal(id: number): Promise<void> {
  return invoke<void>("close_terminal", { id });
}

/** 전체 세션 stats 조회 (stats 패널 폴링용). */
export function getStats(): Promise<SessionStats[]> {
  return invoke<SessionStats[]>("get_stats");
}
