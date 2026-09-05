// 탭 화면 — 한 터미널 탭의 현재 화면과 그 탭으로 보내는 입력.
//
// xterm 을 **`disableStdin: true`** 로 띄우는 것이 이 화면의 핵심 결정이다.
// 우리가 받는 바이트는 데스크톱과 같은 replay/델타이고, 거기에는 프로그램이
// 남긴 단말 질의(`ESC[6n` 등)가 그대로 들어 있다. xterm 은 그 질의를 보면
// 자동으로 응답(`ESC[..R`)을 만들어 데이터 이벤트로 흘리는데, 그것이 PTY 로
// 들어가면 셸의 입력줄에 `;1R` 같은 쓰레기가 남는다. 데스크톱은 replay 구간
// 동안 그 응답을 막아 두고(`terminal-view.ts` 의 replay 게이트) 실제 응답은
// 자기가 한다 — 폰은 같은 세션을 **동시에** 보고 있으므로 델타에서도 답하면
// 안 된다. `disableStdin` 은 xterm 의 데이터 이벤트를 통째로 끊는다.
//
// 그래서 입력은 전부 우리 인코더(`protocol.ts`)가 만들고, 그 인코딩이 읽는
// `term.modes` 는 `term.write` 의 콜백이 돌아야 반영된다 — 그 전까지 입력
// 컨트롤을 비활성으로 두는 이유다.

import { Terminal } from "@xterm/xterm";

import { fetchScreen, HttpError, postInput } from "./api";
import type { ScreenReply } from "./api";
import { ENTER_DELAY_MS, InputQueue } from "./input-queue";
import type { InputItem } from "./input-queue";
import { DEFAULT_MODES } from "./modes";
import type { TerminalModes } from "./modes";
import { PollSchedule } from "./poller";
import {
  encodeInput,
  INITIAL_VIEW_STATE,
  needsRecreate,
  nextRequest,
  screenQuery,
} from "./protocol";
import type { InputAction, InputKey } from "./protocol";
import type { TabId } from "../types";

const POLL_INTERVAL_MS = 2000;
const FONT_SIZE_PX = 11;

/** 키 버튼 한 줄. 라벨은 사용자 노출 문자열이라 영어다. */
const KEY_BUTTONS: { key: InputKey; label: string }[] = [
  { key: "escape", label: "Esc" },
  { key: "tab", label: "Tab" },
  { key: "ctrlC", label: "Ctrl+C" },
  { key: "up", label: "↑" },
  { key: "down", label: "↓" },
  { key: "left", label: "←" },
  { key: "right", label: "→" },
  { key: "backspace", label: "⌫" },
  { key: "enter", label: "Enter" },
];

export interface TabViewOptions {
  tab: TabId;
  title: string;
  onBack: () => void;
}

export class TabView {
  readonly root: HTMLElement;
  private readonly hostEl: HTMLDivElement;
  private readonly noticeEl: HTMLDivElement;
  private readonly textEl: HTMLTextAreaElement;
  private readonly controls: HTMLButtonElement[] = [];
  private readonly schedule: PollSchedule;
  private readonly queue: InputQueue;

  private term: Terminal | null = null;
  private state = { ...INITIAL_VIEW_STATE };
  /** `term.write` 콜백이 돌았나 — 입력 컨트롤의 활성 조건이다. 프로토콜
   *  단계(`state.phase`)와 다르다: 단계는 응답이 오는 즉시 넘어가야 다음
   *  요청이 델타로 나가지만, `term.modes` 는 write 가 끝나야 값이 맞는다. */
  private inputReady = false;
  /** 전송 중인 Send 의 텍스트 항목과 그 원문 — 실패하면 원문을 입력칸에
   *  되돌린다. 폰에서 손으로 친 것이라 실패 한 번에 사라지면 다시 칠 수밖에
   *  없다. 인코딩된 `data` 를 되돌릴 수는 없다 (브래킷 시퀀스가 딸려 온다). */
  private pendingPaste: { item: InputItem; text: string } | null = null;

  constructor(private readonly options: TabViewOptions) {
    this.root = document.createElement("div");
    this.root.className = "screen tab-screen";

    const header = document.createElement("header");
    header.className = "bar";
    const back = document.createElement("button");
    back.type = "button";
    back.className = "bar-back";
    back.textContent = "‹ Back";
    back.addEventListener("click", () => this.options.onBack());
    const title = document.createElement("span");
    title.className = "bar-title";
    title.textContent = options.title;
    header.append(back, title);

    this.noticeEl = document.createElement("div");
    this.noticeEl.className = "notice";
    this.noticeEl.hidden = true;

    const scroll = document.createElement("div");
    scroll.className = "screen-scroll";
    this.hostEl = document.createElement("div");
    this.hostEl.className = "screen-host";
    scroll.append(this.hostEl);

    const composer = document.createElement("div");
    composer.className = "composer";
    this.textEl = document.createElement("textarea");
    this.textEl.className = "composer-text";
    this.textEl.rows = 2;
    this.textEl.placeholder = "Text to send";
    this.textEl.autocapitalize = "off";
    this.textEl.spellcheck = false;
    const send = document.createElement("button");
    send.type = "button";
    send.className = "composer-send";
    send.textContent = "Send";
    send.addEventListener("click", () => this.send());
    this.controls.push(send);
    composer.append(this.textEl, send);

    const keys = document.createElement("div");
    keys.className = "keys";
    for (const spec of KEY_BUTTONS) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "key";
      button.textContent = spec.label;
      button.addEventListener("click", () => this.enqueue([{ type: "key", key: spec.key }]));
      this.controls.push(button);
      keys.append(button);
    }

    this.root.append(header, this.noticeEl, scroll, composer, keys);
    this.setInputEnabled(false);

    this.schedule = new PollSchedule({
      intervalMs: POLL_INTERVAL_MS,
      poll: (generation) => this.poll(generation),
      onHalt: (reason) => {
        this.setNotice(
          reason === "unauthorized"
            ? "Not authorized — scan the pairing QR in winmux again."
            : "Too many requests — retrying in a minute.",
        );
      },
    });
    this.queue = new InputQueue({
      send: (data) => this.sendOne(data),
      onError: (error, item) => this.reportInputError(error, item),
      onIdle: () => {
        this.pendingPaste = null;
      },
    });
  }

  start(): void {
    this.schedule.start();
  }

  setVisible(visible: boolean): void {
    this.schedule.setVisible(visible);
  }

  dispose(): void {
    this.schedule.stop();
    this.queue.clear();
    this.destroyTerminal();
  }

  private setNotice(text: string | null): void {
    this.noticeEl.textContent = text ?? "";
    this.noticeEl.hidden = text === null;
  }

  private setInputEnabled(enabled: boolean): void {
    this.inputReady = enabled;
    this.textEl.disabled = !enabled;
    for (const control of this.controls) control.disabled = !enabled;
  }

  private async poll(generation: number): Promise<void> {
    try {
      const reply = await fetchScreen(this.options.tab, screenQuery(this.state));
      // 늦게 도착한 이전 세대의 응답은 지금 화면과 무관하다.
      if (!this.schedule.isCurrent(generation)) return;
      this.apply(reply);
      this.setNotice(null);
    } catch (error) {
      if (!this.schedule.isCurrent(generation)) return;
      this.handleScreenError(error);
    }
  }

  private apply(reply: ScreenReply): void {
    const { meta, bytes } = reply;
    if (this.state.phase === "full") {
      // 서버 계약상 `since` 없는 요청의 응답은 항상 reset 이다. 아니면 화면을
      // 세울 수 없으므로 상태를 그대로 두고 다음 폴에서 다시 요청한다.
      if (meta.reset) {
        this.createTerminal(meta.cols, meta.rows, bytes);
      }
    } else if (needsRecreate(this.state, meta)) {
      // 이어 붙일 수 없는 응답이다 — 받은 바이트를 버리고 인스턴스를 접는다.
      // 다음 폴이 `since` 없이 나가 새 스냅샷으로 다시 세운다.
      this.destroyTerminal();
    } else {
      this.term?.write(bytes);
    }
    this.state = nextRequest(this.state, meta);
  }

  private createTerminal(cols: number, rows: number, bytes: Uint8Array): void {
    this.destroyTerminal();
    const term = new Terminal({
      cols,
      rows,
      // 파일 상단: 폰이 단말 질의에 답하면 그 응답이 PTY 로 샌다.
      disableStdin: true,
      fontSize: FONT_SIZE_PX,
      cursorBlink: false,
      convertEol: false,
    });
    term.open(this.hostEl);
    this.term = term;
    const generation = this.schedule.generation;
    term.write(bytes, () => {
      if (!this.schedule.isCurrent(generation)) return;
      this.setInputEnabled(true);
    });
  }

  private destroyTerminal(): void {
    // 세대를 올려 이 인스턴스로 향하던 응답·write 콜백을 전부 무효화한다.
    this.schedule.bumpGeneration();
    this.setInputEnabled(false);
    this.term?.dispose();
    this.term = null;
    this.hostEl.replaceChildren();
  }

  private modes(): TerminalModes {
    const modes = this.term?.modes;
    if (modes === undefined) return DEFAULT_MODES;
    return {
      bracketedPasteMode: modes.bracketedPasteMode,
      applicationCursorKeysMode: modes.applicationCursorKeysMode,
    };
  }

  /** Send = 텍스트 한 번, 그 응답 뒤 CR 한 번 (ADR-0016 결정 7). 두 요청으로 나누고
   *  사이를 벌리는 이유는 `input-queue.ts` 모듈 주석에 있다. */
  private send(): void {
    const text = this.textEl.value;
    if (text === "") return;
    const items = this.enqueue([
      { type: "paste", text },
      { type: "key", key: "enter" },
    ]);
    if (items.length === 0) return;
    this.pendingPaste = { item: items[0], text };
    this.textEl.value = "";
  }

  /** 액션을 인코딩해 큐에 넣고, 넣은 항목을 돌려준다 (입력 컨트롤이 아직
   *  비활성이면 빈 배열). 두 번째 이후 항목의 지연이 CR 을 텍스트에서 떼어
   *  놓는 간격이다. */
  private enqueue(actions: InputAction[]): InputItem[] {
    if (!this.inputReady) return [];
    const modes = this.modes();
    const items = actions.map((action, index) => ({
      data: encodeInput(action, modes),
      delayBeforeMs: index === 0 ? undefined : ENTER_DELAY_MS,
    }));
    this.queue.push(...items);
    return items;
  }

  private async sendOne(data: string): Promise<void> {
    const session = this.state.session;
    if (session === null) throw new Error("no session for this tab");
    await postInput(this.options.tab, session, data);
  }

  /** 들고 있던 인스턴스를 버리고 처음부터 다시 받는다. 이미 아무것도 없으면
   *  건드리지 않는다 — 실패가 2초마다 반복되는 동안 세대만 계속 올리게 된다. */
  private resetToFull(): void {
    if (this.term === null) return;
    this.destroyTerminal();
    this.state = { ...INITIAL_VIEW_STATE };
  }

  private reportInputError(error: unknown, item: InputItem): void {
    // 실패한 것이 Send 의 텍스트였다면 원문을 입력칸에 되돌린다 — 그 사이
    // 사용자가 다른 것을 치고 있으면 덮어쓰지 않는다.
    const pending = this.pendingPaste;
    this.pendingPaste = null;
    if (pending !== null && pending.item === item && this.textEl.value === "") {
      this.textEl.value = pending.text;
    }
    if (error instanceof HttpError) {
      this.schedule.noteStatus(error.status);
      // 409 는 이 탭의 셸이 갈렸다는 뜻이다 — 보고 있던 화면이 더는 그 셸이
      // 아니므로 인스턴스를 접고 다음 폴이 새 스냅샷을 받게 한다.
      if (error.status === 409) this.resetToFull();
      this.setNotice(inputErrorText(error.status));
      return;
    }
    this.setNotice("Could not reach winmux — check the connection.");
  }

  private handleScreenError(error: unknown): void {
    if (error instanceof HttpError) {
      this.schedule.noteStatus(error.status);
      // 탭이 사라졌거나 셸이 없다 — 들고 있던 화면은 더는 유효하지 않다.
      if (error.status === 409 || error.status === 404) this.resetToFull();
      if (error.status !== 401 && error.status !== 429) {
        this.setNotice(screenErrorText(error.status));
      }
      return;
    }
    this.setNotice("Could not reach winmux — retrying.");
  }
}

function screenErrorText(status: number): string {
  switch (status) {
    case 404:
      return "This tab is gone.";
    case 409:
      return "This tab has no running shell.";
    default:
      return `winmux replied ${status}.`;
  }
}

function inputErrorText(status: number): string {
  switch (status) {
    case 401:
      return "Not authorized — scan the pairing QR in winmux again.";
    case 409:
      return "The shell restarted — input was not sent.";
    case 413:
      return "That text is too long to send.";
    case 429:
      return "Too many requests — try again in a minute.";
    default:
      return `Input failed (${status}).`;
  }
}
