// needsInput 알림 차임 — WebAudio 로 짧은 2음을 합성해 "에이전트가 사용자 입력을
// 기다린다"를 소리로 알린다 (실기 결함: 알림 소리 경로 자체가 없어 대기 상태를
// 놓쳤다). 이 모듈은 (1) 소리 합성·재생과 (2) 언제 울릴지의 순수 판정
// (detectNeedsInputOnset) 둘을 담고, DOM 배선은 main.ts 몫이다.
//
// 외부 오디오 에셋을 쓰지 않고 오실레이터로 합성하는 이유: 앱은 오프라인·CSP
// 아래서 도는 WebView 라 번들 밖 리소스를 가져올 수 없고, 0.3초짜리 알림음 하나에
// 에셋 파이프라인을 붙일 이유도 없다.
//
// AudioContext 는 **lazy** 다 — 부팅 시 만들면 사용자 제스처 전이라 어차피
// suspended 로 시작해 자원만 잡는다. 브라우저 autoplay 정책상 제스처 없이 만든
// 컨텍스트에 스케줄한 소리는 나지 않으므로, 첫 keydown/mousedown 에서 resume 하는
// unlock 패턴을 installChimeUnlock 이 배선한다. 그럼에도 재생 시점에 컨텍스트가
// 안 돌고 있으면 resume 을 한 번 더 시도하고, 실패는 조용히 넘긴다 — 알림음은
// 보조 신호라서 실패가 UI 동작을 막으면 안 된다 (원인은 console.debug 로만 남긴다).
//
// 백로그: mute/볼륨 설정은 이번 범위 밖이다 (앱에 설정 표면 자체가 아직 없다).
// 설정이 생기면 play() 앞단의 게이트로 붙인다 — 재생 로직은 건드릴 것이 없다.

import type { AgentStatus, WorkspaceId } from "./types";

/** AudioContext 생성기 — 테스트가 가짜 컨텍스트를 주입하는 이음매다
 *  (WebAudio 는 node 환경에 없고, happy-dom 에도 없다). */
export type AudioContextFactory = () => AudioContext;

const defaultFactory: AudioContextFactory = () => new AudioContext();

/** 피크 게인 — 일부러 낮게(0.1) 잡는다. 알림은 존재를 알리는 정도면 충분하고,
 *  작업 중 놀랄 만큼 크면 사용자가 소리를 아예 꺼 버린다. */
const PEAK_GAIN = 0.1;

/** 엔벨로프의 사실상 무음 값 — exponentialRamp 는 0 을 목표로 잡을 수 없어
 *  (0 이면 예외) 이 값으로 대신한다. */
const SILENT_GAIN = 0.0001;

/** 상승 attack 시간(초) — 0 에서 즉시 켜면 클릭 잡음이 난다. */
const ATTACK_S = 0.02;

/** 2음 스케줄(초) — 총 길이 0.3s. A5 → E6 의 상승 5도라 "질문/대기" 로 읽힌다
 *  (하강 음정은 완료·실패로 읽혀 의미가 반대다). */
const TONES: readonly { freq: number; at: number; dur: number }[] = [
  { freq: 880, at: 0, dur: 0.16 },
  { freq: 1320, at: 0.14, dur: 0.16 },
];

export class Chime {
  private ctx: AudioContext | null = null;
  /** 컨텍스트 생성이 실패한 환경 표시 — 재시도하지 않는다 (WebAudio 자체가 없는
   *  환경이면 렌더마다 예외를 다시 만들 이유가 없다). */
  private unavailable = false;

  constructor(private readonly createContext: AudioContextFactory = defaultFactory) {}

  /** 사용자 제스처 훅 — 컨텍스트를 만들고 resume 한다 (autoplay 정책 unlock).
   *  installChimeUnlock 이 첫 keydown/mousedown 에서 부른다. */
  unlock(): void {
    const ctx = this.context();
    if (ctx === null) return;
    this.resume(ctx);
  }

  /** 차임 1회 재생 — 2음을 현재 시각 기준으로 스케줄한다. 컨텍스트가 안 돌고
   *  있으면 resume 을 시도하되 **기다리지 않는다**: 제스처 이력이 있으면 대개
   *  즉시 풀려 스케줄된 소리가 그대로 나고, 아니면 이번 소리는 조용히 사라진다. */
  play(): void {
    const ctx = this.context();
    if (ctx === null) return;
    if (ctx.state !== "running") this.resume(ctx);
    try {
      const now = ctx.currentTime;
      for (const tone of TONES) this.schedule(ctx, tone.freq, now + tone.at, tone.dur);
    } catch (err) {
      console.debug("[winmux] chime scheduling failed", err);
    }
  }

  /** 오실레이터 1음 — sine + 짧은 attack/decay 엔벨로프. 노드는 stop 뒤 자동
   *  해제되므로 별도 정리가 없다. */
  private schedule(ctx: AudioContext, freq: number, startAt: number, duration: number): void {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.setValueAtTime(freq, startAt);
    gain.gain.setValueAtTime(SILENT_GAIN, startAt);
    gain.gain.exponentialRampToValueAtTime(PEAK_GAIN, startAt + ATTACK_S);
    gain.gain.exponentialRampToValueAtTime(SILENT_GAIN, startAt + duration);
    osc.connect(gain);
    gain.connect(ctx.destination);
    osc.start(startAt);
    osc.stop(startAt + duration);
  }

  /** lazy 컨텍스트 — 생성 실패는 이 기능만 끄고 호출측에는 알리지 않는다. */
  private context(): AudioContext | null {
    if (this.ctx !== null) return this.ctx;
    if (this.unavailable) return null;
    try {
      this.ctx = this.createContext();
    } catch (err) {
      this.unavailable = true;
      console.debug("[winmux] AudioContext unavailable — chime disabled", err);
      return null;
    }
    return this.ctx;
  }

  /** resume 시도 — 거부(autoplay 정책)는 삼킨다. 다음 제스처·다음 재생에서 다시
   *  시도되므로 여기서 상태를 기억할 필요가 없다. */
  private resume(ctx: AudioContext): void {
    try {
      void ctx.resume().catch(() => undefined);
    } catch (err) {
      console.debug("[winmux] chime resume failed", err);
    }
  }
}

/** 사용자 제스처 unlock 배선 — 첫 keydown/mousedown 에서 1회 resume 한다.
 *  capture 단계로 다는 이유는 활동 핑과 같다: xterm 이 포커스를 쥐고 있어도 window
 *  까지 도달한다. 1회 뒤 리스너를 떼는 것은 이후 재생 경로가 알아서 resume 을
 *  재시도하기 때문이다 — 상시 리스너를 남길 이유가 없다. */
export function installChimeUnlock(chime: Chime, target: EventTarget = window): void {
  // capture 는 객체 형태로 넘긴다 — 브라우저는 boolean 도 받지만 node 의 EventTarget
  // 은 removeEventListener 에서 boolean 형태의 capture 를 무시해(옵션 객체만 읽는다)
  // 리스너가 떨어지지 않는다. 등록/해제 옵션이 어긋나면 해제가 조용히 실패한다.
  const opts = { capture: true } as const;
  const unlock = (): void => {
    target.removeEventListener("keydown", unlock, opts);
    target.removeEventListener("mousedown", unlock, opts);
    chime.unlock();
  };
  target.addEventListener("keydown", unlock, opts);
  target.addEventListener("mousedown", unlock, opts);
}

/** 전이 판정 입력 — 스냅샷 워크스페이스에서 쓰는 두 필드만 요구한다
 *  (Workspace 전체를 요구하지 않아 테스트가 가볍다). */
export interface AgentStatusEntry {
  id: WorkspaceId;
  agentStatus: AgentStatus;
}

export interface NeedsInputOnset {
  /** 이번 스냅샷에 needsInput 상승 전이가 하나라도 있었나 — 여러 워크스페이스가
   *  동시에 전이해도 소리는 1회다 (소음 방지). `onsets.length > 0` 과 항상 같다. */
  chime: boolean;
  /** needsInput 으로 **새로 전이한** 워크스페이스들 (입력 순서 그대로). 소리는
   *  전체를 1회로 합치지만 OS 토스트는 워크스페이스마다 하나다 — "어느 프로젝트가
   *  기다리는가"가 토스트의 내용 자체라, 합치면 알림의 의미가 사라진다. */
  onsets: WorkspaceId[];
  /** 다음 판정의 기준선이 될 상태 맵 — 사라진 워크스페이스는 빠지므로 맵이 무한히
   *  자라지 않고, 같은 id 가 다시 나타나면 신규로 취급된다. */
  next: Map<WorkspaceId, AgentStatus>;
}

/** needsInput **상승 전이** 판정 (순수) — 어느 워크스페이스든 직전에 needsInput 이
 *  아니었다가 needsInput 이 된 경우에만 chime=true 이고, 그 워크스페이스들이
 *  `onsets` 에 담긴다 (소리는 1회, 토스트는 전이마다 — 판정 규칙은 하나다).
 *
 *  - 같은 상태 반복(needsInput → needsInput)은 무음. 스냅샷은 무관한 변경
 *    (탭 활동·git 등)으로도 자주 오므로, 반복까지 울리면 소음이 된다.
 *  - running·idle 로의 전환은 전부 무음 — 사용자의 개입을 기다리는 상태는
 *    needsInput 하나뿐이다 (sidebar 의 강조 규칙과 같은 판단).
 *  - 신규 워크스페이스의 첫 상태가 needsInput 이면 울린다 (prev 에 없는 id 는
 *    "needsInput 이 아니었다" 로 친다).
 *  - `prev === null` 은 **부팅 첫 스냅샷**이다: 소리 없이 기준선만 채운다. 재시작
 *    복원은 코어 sanitize 가 agent_status 를 Idle 로 초기화하므로 자연히 무음이지만,
 *    WebView 리로드·자동 리셋에서는 살아 있는 세션의 needsInput 이 그대로 첫
 *    스냅샷에 실려 온다 — 그때 울리면 "전이"가 아닌 것에 울리는 셈이라 명시적으로
 *    기준선 취급한다. */
export function detectNeedsInputOnset(
  prev: ReadonlyMap<WorkspaceId, AgentStatus> | null,
  workspaces: readonly AgentStatusEntry[],
): NeedsInputOnset {
  const next = new Map<WorkspaceId, AgentStatus>();
  const onsets: WorkspaceId[] = [];
  for (const ws of workspaces) {
    next.set(ws.id, ws.agentStatus);
    if (prev === null) continue;
    if (ws.agentStatus !== "needsInput") continue;
    if (prev.get(ws.id) === "needsInput") continue;
    onsets.push(ws.id);
  }
  // chime 은 onsets 에서 파생한다 — 두 값을 따로 세면 어긋날 수 있고, "소리는
  // 1회·토스트는 전이마다"라는 규칙 차이는 호출측(main.ts)이 이 둘을 어떻게
  // 쓰느냐로 표현된다.
  return { chime: onsets.length > 0, onsets, next };
}
