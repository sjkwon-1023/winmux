// 차임의 계약 테스트 — (1) needsInput 상승 전이 판정(순수), (2) 재생 경로의
// lazy 생성·resume·조용한 실패. WebAudio 는 node 환경에 없으므로 가짜 컨텍스트를
// 주입해 스케줄된 음의 개수·주파수·길이를 그대로 관찰한다.

import { describe, expect, it, vi } from "vitest";

import { Chime, detectNeedsInputOnset, installChimeUnlock } from "./chime";
import type { AgentStatus, WorkspaceId } from "./types";

function statuses(entries: [WorkspaceId, AgentStatus][]): Map<WorkspaceId, AgentStatus> {
  return new Map(entries);
}

describe("detectNeedsInputOnset", () => {
  it("부팅 첫 스냅샷(prev=null)은 기준선으로만 쓰고 울리지 않는다", () => {
    // WebView 리로드 직후처럼 살아 있는 needsInput 이 첫 스냅샷에 실려 와도 무음.
    const out = detectNeedsInputOnset(null, [
      { id: 1, agentStatus: "needsInput" },
      { id: 2, agentStatus: "running" },
    ]);
    expect(out.chime).toBe(false);
    expect(out.onsets).toEqual([]);
    expect(out.next).toEqual(statuses([[1, "needsInput"], [2, "running"]]));
  });

  it("idle·running → needsInput 은 울린다", () => {
    const fromIdle = detectNeedsInputOnset(statuses([[1, "idle"]]), [
      { id: 1, agentStatus: "needsInput" },
    ]);
    expect(fromIdle.chime).toBe(true);
    expect(fromIdle.onsets).toEqual([1]);
    const fromRunning = detectNeedsInputOnset(statuses([[1, "running"]]), [
      { id: 1, agentStatus: "needsInput" },
    ]);
    expect(fromRunning.chime).toBe(true);
    expect(fromRunning.onsets).toEqual([1]);
  });

  it("같은 needsInput 이 유지되는 재렌더는 무음이다", () => {
    const out = detectNeedsInputOnset(statuses([[1, "needsInput"]]), [
      { id: 1, agentStatus: "needsInput" },
    ]);
    expect(out.chime).toBe(false);
    expect(out.onsets).toEqual([]);
  });

  it("needsInput 이 아닌 쪽으로 가는 전환은 전부 무음이다", () => {
    const toIdle = detectNeedsInputOnset(statuses([[1, "needsInput"]]), [
      { id: 1, agentStatus: "idle" },
    ]);
    const toRunning = detectNeedsInputOnset(statuses([[1, "idle"]]), [
      { id: 1, agentStatus: "running" },
    ]);
    expect(toIdle.chime).toBe(false);
    expect(toRunning.chime).toBe(false);
  });

  it("신규 워크스페이스의 첫 상태가 needsInput 이면 울린다 (다른 첫 상태는 무음)", () => {
    const prev = statuses([[1, "idle"]]);
    const added = detectNeedsInputOnset(prev, [
      { id: 1, agentStatus: "idle" },
      { id: 2, agentStatus: "needsInput" },
    ]);
    expect(added.chime).toBe(true);
    expect(added.onsets).toEqual([2]);
    const addedRunning = detectNeedsInputOnset(prev, [
      { id: 1, agentStatus: "idle" },
      { id: 2, agentStatus: "running" },
    ]);
    expect(addedRunning.chime).toBe(false);
    expect(addedRunning.onsets).toEqual([]);
  });

  it("동시에 여러 워크스페이스가 전이해도 차임 판정은 1회다 (토스트는 전이마다)", () => {
    const out = detectNeedsInputOnset(statuses([[1, "running"], [2, "idle"]]), [
      { id: 1, agentStatus: "needsInput" },
      { id: 2, agentStatus: "needsInput" },
    ]);
    // chime 은 boolean 이라 호출측이 재생을 1회만 한다 (개수를 세지 않는다).
    // 토스트는 "어느 워크스페이스가 기다리는가"가 내용이라 합칠 수 없어, 전이한
    // 워크스페이스가 입력 순서대로 전부 onsets 에 담긴다.
    expect(out.chime).toBe(true);
    expect(out.onsets).toEqual([1, 2]);
  });

  it("사라진 워크스페이스는 기준선에서 빠지고, 다시 나타나면 신규로 취급된다", () => {
    const closed = detectNeedsInputOnset(statuses([[1, "needsInput"], [2, "idle"]]), [
      { id: 2, agentStatus: "idle" },
    ]);
    expect(closed.chime).toBe(false);
    expect(closed.next).toEqual(statuses([[2, "idle"]]));
    // 같은 id 가 needsInput 으로 되돌아오면 "아니었다가 됐다" 이므로 울린다.
    const reappeared = detectNeedsInputOnset(closed.next, [
      { id: 1, agentStatus: "needsInput" },
      { id: 2, agentStatus: "idle" },
    ]);
    expect(reappeared.chime).toBe(true);
    expect(reappeared.onsets).toEqual([1]);
  });

  it("워크스페이스가 하나도 없으면 무음이고 기준선은 빈 맵이다", () => {
    const out = detectNeedsInputOnset(statuses([[1, "needsInput"]]), []);
    expect(out.chime).toBe(false);
    expect(out.onsets).toEqual([]);
    expect(out.next.size).toBe(0);
  });
});

/** 가짜 AudioContext — 스케줄된 음(주파수·시작·정지)만 기록한다. */
function fakeContext(state: AudioContextState = "running") {
  const tones: { freq: number; start: number; stop: number; peak: number }[] = [];
  const resume = vi.fn(() => Promise.resolve());
  const ctx = {
    currentTime: 10,
    state,
    resume,
    destination: {},
    createOscillator: () => {
      const tone = { freq: 0, start: 0, stop: 0, peak: 0 };
      tones.push(tone);
      return {
        type: "",
        frequency: {
          setValueAtTime: (v: number) => {
            tone.freq = v;
          },
        },
        connect: () => undefined,
        start: (t: number) => {
          tone.start = t;
        },
        stop: (t: number) => {
          tone.stop = t;
        },
      };
    },
    createGain: () => ({
      gain: {
        setValueAtTime: () => undefined,
        exponentialRampToValueAtTime: (v: number) => {
          const tone = tones[tones.length - 1];
          if (tone !== undefined && v > tone.peak) tone.peak = v;
        },
      },
      connect: () => undefined,
    }),
  };
  return { ctx: ctx as unknown as AudioContext, tones, resume };
}

describe("Chime", () => {
  it("play 는 컨텍스트를 lazy 하게 1회만 만들고 2음을 스케줄한다 (총 ~0.3s)", () => {
    const fake = fakeContext();
    const factory = vi.fn(() => fake.ctx);
    const chime = new Chime(factory);
    expect(factory).not.toHaveBeenCalled(); // 생성자에서는 만들지 않는다

    chime.play();
    expect(factory).toHaveBeenCalledTimes(1);
    expect(fake.tones).toHaveLength(2);
    expect(fake.tones.map((t) => t.freq)).toEqual([880, 1320]);
    // 시작은 현재 시각 기준, 총 길이는 0.3s (마지막 정지 - 첫 시작).
    expect(fake.tones[0].start).toBeCloseTo(10);
    expect(fake.tones[1].stop - fake.tones[0].start).toBeCloseTo(0.3);
    // 볼륨은 낮게 — 피크 게인 0.1.
    expect(Math.max(...fake.tones.map((t) => t.peak))).toBeCloseTo(0.1);

    chime.play();
    expect(factory).toHaveBeenCalledTimes(1); // 재사용
    expect(fake.tones).toHaveLength(4);
  });

  it("running 이면 resume 하지 않고, suspended 면 resume 을 시도한다", () => {
    const running = fakeContext("running");
    new Chime(() => running.ctx).play();
    expect(running.resume).not.toHaveBeenCalled();

    const suspended = fakeContext("suspended");
    new Chime(() => suspended.ctx).play();
    expect(suspended.resume).toHaveBeenCalledTimes(1);
    // resume 완료를 기다리지 않고 그대로 스케줄한다 (풀리면 그때 소리가 난다).
    expect(suspended.tones).toHaveLength(2);
  });

  it("컨텍스트 생성 실패는 조용히 무시하고 재시도하지 않는다", () => {
    const factory = vi.fn(() => {
      throw new Error("no WebAudio");
    });
    const chime = new Chime(factory);
    expect(() => chime.play()).not.toThrow();
    expect(() => chime.unlock()).not.toThrow();
    expect(factory).toHaveBeenCalledTimes(1);
  });

  it("resume 거부(autoplay 정책)는 재생 경로를 깨지 않는다", async () => {
    const fake = fakeContext("suspended");
    (fake.resume as unknown as { mockImplementation: (f: () => Promise<void>) => void })
      .mockImplementation(() => Promise.reject(new Error("blocked")));
    const chime = new Chime(() => fake.ctx);
    expect(() => chime.play()).not.toThrow();
    // unhandled rejection 이 남지 않는지 — 마이크로태스크를 한 바퀴 돌린다.
    await Promise.resolve();
    expect(fake.tones).toHaveLength(2);
  });

  it("unlock 은 컨텍스트를 만들고 resume 한다 (소리는 내지 않는다)", () => {
    const fake = fakeContext("suspended");
    const chime = new Chime(() => fake.ctx);
    chime.unlock();
    expect(fake.resume).toHaveBeenCalledTimes(1);
    expect(fake.tones).toHaveLength(0);
  });
});

describe("installChimeUnlock", () => {
  it("첫 keydown 에서 1회 unlock 하고 리스너를 뗀다", () => {
    const fake = fakeContext("suspended");
    const chime = new Chime(() => fake.ctx);
    const target = new EventTarget();
    installChimeUnlock(chime, target);

    target.dispatchEvent(new Event("keydown"));
    expect(fake.resume).toHaveBeenCalledTimes(1);
    // 이후 제스처는 리스너가 없으므로 unlock 을 다시 부르지 않는다.
    target.dispatchEvent(new Event("keydown"));
    target.dispatchEvent(new Event("mousedown"));
    expect(fake.resume).toHaveBeenCalledTimes(1);
  });

  it("mousedown 도 unlock 진입점이다 (키보드 없이 시작하는 경우)", () => {
    const fake = fakeContext("suspended");
    const chime = new Chime(() => fake.ctx);
    const target = new EventTarget();
    installChimeUnlock(chime, target);

    target.dispatchEvent(new Event("mousedown"));
    expect(fake.resume).toHaveBeenCalledTimes(1);
  });
});
