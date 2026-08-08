// Tauri 백엔드 커맨드·이벤트 계약 래퍼 (10단계 계획 3-C).
// 커맨드 인자 키는 Tauri v2 기본 규칙(JS camelCase → Rust snake_case)을 따른다.
// write_stdin/send_raw/resize/ack_output/get_stats 는 spike 글루의 이식이라
// 인자 이름(id)·DTO(snake_case)를 그대로 유지하고, dispatch/get_state/
// attach_terminal 은 10단계 신규 계약이다.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";

import type { Command, CommandOutput, SessionId, StateSnapshot } from "./types";

/** 터미널 출력 채널 메시지 — raw channel 은 ArrayBuffer 를 주지만, 구현 차이에
 *  대비해 Uint8Array 도 수용한다. 소비 측(frame.ts)에서 정규화한다. */
export type OutputChunk = ArrayBuffer | Uint8Array;

/** src-tauri SessionStats DTO (serde 기본 — Rust 필드명 snake_case 그대로). */
export interface SessionStats {
  id: number;
  bytes_out: number;
  pending: number;
  paused: boolean;
  osc_count: number;
  last_osc: string | null;
  alive: boolean;
}

/** 구조 변이 명령을 dispatch 한다. 성공 시 백엔드가 state-changed 를 emit 하므로
 *  호출자는 반환값(생성 id)만 쓰고 상태 갱신은 store 구독으로 받는다.
 *  실패는 CommandError 직렬화 payload 로 reject 된다. */
export function dispatch(cmd: Command): Promise<CommandOutput> {
  return invoke<CommandOutput>("dispatch", { cmd });
}

/** 부트스트랩용 전체 상태 스냅샷. */
export function getState(): Promise<StateSnapshot> {
  return invoke<StateSnapshot>("get_state");
}

/** 기존 PTY 세션에 attach 한다. **호출 전에 onOutput 채널의 onmessage 를 먼저
 *  걸어야 한다** (채널 먼저·reattach 나중 — 코어 session.rs reattach 계약).
 *  응답은 raw body `[u64 LE end_offset][replay bytes]` — frame.ts 로 파싱한다. */
export function attachTerminal(
  session: SessionId,
  onOutput: Channel<OutputChunk>,
): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("attach_terminal", { session, onOutput });
}

/** 출력 채널 분리 — 뷰 dispose(탭 전환 등) 시. 세션은 계속 돌고 출력은 Dropped
 *  (detach 모드)로 replay 에만 쌓인다 — 채널을 남겨두면 Delivered-무ack 로 pending
 *  이 쌓여 백그라운드 세션이 paused 에 고착된다. */
export function detachTerminal(session: SessionId): Promise<void> {
  return invoke<void>("detach_terminal", { session });
}

/** 사용자 입력(문자열)을 PTY stdin 으로 보낸다. */
export function writeStdin(id: SessionId, data: string): Promise<void> {
  return invoke<void>("write_stdin", { id, data });
}

/** 임의 바이트열을 PTY stdin 으로 보낸다 (제어 시퀀스 테스트용). */
export function sendRaw(id: SessionId, bytes: number[]): Promise<void> {
  return invoke<void>("send_raw", { id, bytes });
}

/** PTY 창 크기 변경 (자식에게 SIGWINCH 전달). */
export function resizeTerminal(id: SessionId, cols: number, rows: number): Promise<void> {
  return invoke<void>("resize", { id, cols, rows });
}

/** flow control ack — 프론트가 소비 완료한 바이트 수를 백엔드에 알린다. */
export function ackOutput(id: SessionId, n: number): Promise<void> {
  return invoke<void>("ack_output", { id, n });
}

/** 전체 세션 stats 조회 (진단용). */
export function getStats(): Promise<SessionStats[]> {
  return invoke<SessionStats[]>("get_stats");
}

/** state-changed 구독 헬퍼 — 변이마다 전체 스냅샷(revision 포함)이 온다.
 *  stale 판정(revision 가드)은 store 몫이다. */
export function onStateChanged(
  handler: (snapshot: StateSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<StateSnapshot>("state-changed", (event) => handler(event.payload));
}
