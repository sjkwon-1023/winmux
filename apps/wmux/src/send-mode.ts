// 패널 간 텍스트 전달 — 대상 선택 모드 상태 머신 (17단계 D4, 계획 v2 8장).
//
// DOM 무의존 순수 모듈: 소스 캡처(arm) → 대상 확정(resolve)/취소(cancel) 전이와
// 상태 라인 프롬프트 문자열만 담당한다. 클릭·Esc 배선과 실제 전달(대상 뷰의
// paste/submit — bracketed paste 경유, 계획 D1)은 workspace-view/pane-view 가 한다.

import type { PaneId } from "./types";

/** 상태 — idle(평시) | armed(대상 선택 중: 캡처된 텍스트·submit 여부·소스 pane). */
export type SendModeState =
  | { type: "idle" }
  | { type: "armed"; text: string; submit: boolean; source: PaneId };

/** resolve 결과 — deliver=false 는 전달 없음(자기 자신 클릭·idle 호출)이다.
 *  그 경우 text/submit 은 의미 없는 빈 값으로 고정한다 (호출측 분기 단순화). */
export interface SendResolution {
  deliver: boolean;
  text: string;
  submit: boolean;
}

export class SendMode {
  private current: SendModeState = { type: "idle" };

  get state(): SendModeState {
    return this.current;
  }

  /** 대상 선택 모드 활성 여부 — pane mousedown 이 resolve 경로로 갈지 판정한다. */
  get active(): boolean {
    return this.current.type === "armed";
  }

  /** 대상 선택 모드 진입 — 이미 armed 여도 덮어쓴다 (아이콘 재클릭 = 재캡처).
   *  선택 텍스트 캡처·무선택 에러 판정은 호출측(pane-view) 책임이다 — 여기는
   *  캡처가 성공한 뒤에만 불린다. */
  arm(source: PaneId, text: string, submit: boolean): void {
    this.current = { type: "armed", text, submit, source };
  }

  /** 취소 — Esc·호출측 판단 어느 경로든 idle 로 돌아간다 (idle 에서는 no-op). */
  cancel(): void {
    this.current = { type: "idle" };
  }

  /** 대상 확정 — 어떤 결과든 모드는 끝난다(1회성). target === source 는 취소와
   *  동일 처리다 (자기 전달은 무의미 — 계획 D2). idle 에서의 호출도 no-op 취소. */
  resolve(target: PaneId): SendResolution {
    const s = this.current;
    this.current = { type: "idle" };
    if (s.type !== "armed" || target === s.source) {
      return { deliver: false, text: "", submit: false };
    }
    return { deliver: true, text: s.text, submit: s.submit };
  }
}

/** 상태 라인 프롬프트 — armed 동안 지속 표시할 문자열, idle 은 null(표시 없음).
 *  전달(⤷)과 전달 후 실행(⤷⏎)을 문구에서도 구분한다 — 실수 실행 방지의 연장. */
export function sendModePrompt(state: SendModeState): string | null {
  if (state.type === "idle") return null;
  const verb = state.submit ? "send & run" : "send";
  return `${verb}: click a pane to send to (Esc cancels)`;
}
