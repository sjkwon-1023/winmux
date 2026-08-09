// ActivityPing throttle 의 계약 테스트 (계획 C-3) — 10초 창당 1회 + visibility
// 즉시 통과. DOM 무의존 (시각은 인자로 주입).

import { describe, expect, it, vi } from "vitest";

import { ActivityPing } from "./activity-ping";

describe("ActivityPing", () => {
  it("첫 활동은 즉시 전송하고, 침묵 창(10초) 안의 반복은 1회로 묶인다", () => {
    const send = vi.fn();
    const ping = new ActivityPing(send);
    ping.activity(0);
    ping.activity(1);
    ping.activity(9_999);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenLastCalledWith(null);
    // 경계값: 창 길이 도달 시점부터 다시 통과.
    ping.activity(10_000);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("연속 사용 중에는 창마다 정확히 1회 나간다 (핑 자체가 스팸이 되지 않는다)", () => {
    const send = vi.fn();
    const ping = new ActivityPing(send);
    // 30초 동안 1초 간격 스크롤 → 0s/10s/20s/30s 의 4회만.
    for (let t = 0; t <= 30_000; t += 1_000) ping.activity(t);
    expect(send).toHaveBeenCalledTimes(4);
  });

  it("visibility 전이는 침묵 창 안에서도 즉시 통과하고 visible 값을 실어 보낸다", () => {
    const send = vi.fn();
    const ping = new ActivityPing(send);
    ping.activity(0);
    ping.visibility(false, 100);
    ping.visibility(true, 200);
    expect(send).toHaveBeenCalledTimes(3);
    expect(send.mock.calls).toEqual([[null], [false], [true]]);
  });

  it("visibility 전송이 침묵 창을 리셋한다 (직후 활동 핑은 중복이라 억제)", () => {
    const send = vi.fn();
    const ping = new ActivityPing(send);
    ping.visibility(true, 5_000);
    // 백엔드는 visibility 전송도 활동으로 쳤으므로 5초 뒤 활동은 창 안 — 억제.
    ping.activity(9_999);
    expect(send).toHaveBeenCalledTimes(1);
    ping.activity(15_000);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("windowMs 를 좁히면 그 주기로 통과한다", () => {
    const send = vi.fn();
    const ping = new ActivityPing(send, 1_000);
    ping.activity(0);
    ping.activity(999);
    ping.activity(1_000);
    expect(send).toHaveBeenCalledTimes(2);
  });
});
