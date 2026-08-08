// 탭바 버튼 모델 계산 (DOM-free — vitest 대상, 12단계 청크 C).
//
// Pane 의 tabs 를 렌더 가능한 버튼 모델 배열로 사영한다. DOM 조립(pane-view)과
// 판정 로직을 분리해 active·exited·notification 조합을 순수 테스트로 잠근다.

import type { Pane, TabId } from "./types";

/** 탭 버튼 1개의 렌더 모델. */
export interface TabButtonModel {
  tab: TabId;
  title: string;
  /** pane 의 activeTab 인지 — 강조 + 클릭 no-op 판정에 쓰인다. */
  active: boolean;
  /** terminal 탭이고 프로세스가 종료됐는지 (Exited 배지 — 계획 1-C). */
  exited: boolean;
  /** 미확인 알림 dot 표시 여부 (NotificationState "unread"). */
  notification: boolean;
}

/** Pane → 탭 버튼 모델 배열 (탭 순서 유지). 탭 0개면 빈 배열. */
export function tabStripModel(pane: Pane): TabButtonModel[] {
  return pane.tabs.map((tab) => ({
    tab: tab.id,
    title: tab.title,
    active: tab.id === pane.activeTab,
    exited: tab.kind.type === "terminal" && tab.kind.status.type === "exited",
    notification: tab.notification === "unread",
  }));
}
