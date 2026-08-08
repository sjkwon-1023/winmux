// CommandError(dispatch reject payload) → 상태 라인용 한 줄 영어 요약 (순수,
// vitest 대상 — 11단계 청크 B).
//
// backend.dispatch 는 실패 시 CommandError 직렬화 객체로 reject 되지만
// (command.rs CommandError — internal tag "type"), IPC 레벨 실패 등 계약 밖
// 값이 올 수도 있어 unknown 을 받아 방어적으로 narrowing 한다. 계약 밖 값도
// 삼키지 않고 "Command failed: ..." 로 그대로 노출한다 (가리지 않기).

import type { CommandError } from "./types";

function isCommandError(e: unknown): e is CommandError {
  if (typeof e !== "object" || e === null) return false;
  const t = (e as { type?: unknown }).type;
  return (
    t === "unknownTarget" || t === "lastPane" || t === "spawnFailed" || t === "invalidRatio"
  );
}

/** 계약 밖 payload 의 표시 문자열 — 문자열은 그대로, 객체는 JSON, 그 외/실패는
 *  String() 폴백. (JSON.stringify 는 undefined·symbol 에서 undefined 를 주고
 *  순환 참조에서 throw 하므로 둘 다 폴백 처리한다.) */
function describeUnknown(e: unknown): string {
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e) ?? String(e);
  } catch {
    return String(e);
  }
}

/** dispatch 실패 payload 의 한 줄 요약 (영어 UI 텍스트 — 상태 라인 표시용). */
export function formatCommandError(e: unknown): string {
  if (isCommandError(e)) {
    switch (e.type) {
      case "invalidRatio":
        return `Invalid split ratio ${e.ratio} (must be strictly between 0 and 1)`;
      case "unknownTarget":
        return `Target not found: ${e.target} (stale id?)`;
      case "lastPane":
        return "Cannot close the last pane of a workspace";
      case "spawnFailed":
        // 한 줄 요약 계약 — 멀티라인 스폰 에러는 공백으로 접는다.
        return `Shell spawn failed: ${e.message.replace(/\s+/g, " ").trim()}`;
    }
  }
  return `Command failed: ${describeUnknown(e)}`;
}
