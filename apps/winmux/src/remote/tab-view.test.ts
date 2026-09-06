// @vitest-environment happy-dom
//
// 탭 화면의 첫 프레임을 실제 headless xterm 으로 끝까지 돌린다 — 스냅샷 응답 →
// 텍스트 렌더 → 입력 활성 → Send 의 두 요청(브래킷 텍스트, 그 뒤 CR).
//
// 회귀 잠금이다. v0.3.18 은 `@xterm/headless` 가 `buffer` 를 `allowProposedApi` 뒤에
// 두는 줄 몰랐고, 그 예외는 xterm 의 write 루프 안에서 터져 어디에도 보고되지
// 않았다 — 폰 화면은 검은 채, 입력은 비활성인 채로 남았다. 순수 판정 테스트는
// 이것을 잡을 수 없다: 실패 지점이 라이브러리 인스턴스와 그 콜백 사이에 있다.

import { afterEach, describe, expect, it, vi } from "vitest";

import { TabView } from "./tab-view";
import type { TabId } from "../types";

// 브래킷 붙여넣기를 켜는 시퀀스가 앞에 있어야 Send 가 텍스트를 감싼다.
const SCREEN = "\x1b[?2004hkwon1@pc:~$ ls\r\napps  crates  docs\r\nkwon1@pc:~$ ";
const TAB = 7 as unknown as TabId;

function screenResponse(url: string): Response {
  const bytes = new TextEncoder().encode(SCREEN);
  const reset = !url.includes("since=");
  const headers = new Headers({
    "X-Winmux-End-Offset": String(bytes.length),
    "X-Winmux-Reset": reset ? "1" : "0",
    "X-Winmux-Cols": "120",
    "X-Winmux-Rows": "30",
    "X-Winmux-Session": "4242:7",
  });
  const body = reset ? bytes : new Uint8Array(0);
  return {
    ok: true,
    status: 200,
    headers,
    arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength),
  } as unknown as Response;
}

async function until(check: () => boolean, what: string, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!check()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

describe("TabView first frame", () => {
  const posts: string[] = [];
  let view: TabView | null = null;

  afterEach(() => {
    view?.dispose();
    view = null;
    posts.length = 0;
    vi.unstubAllGlobals();
  });

  it("renders the snapshot as text, enables input, and sends bracketed text then CR", async () => {
    window.localStorage.setItem("winmux.remoteToken", "test-token");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url.includes("/screen")) return screenResponse(url);
        if (url.includes("/input")) {
          posts.push(String(init?.body));
          return { ok: true, status: 204, headers: new Headers() } as unknown as Response;
        }
        throw new Error(`unexpected request ${url}`);
      }),
    );

    view = new TabView({ tab: TAB, title: "t", onBack: () => undefined });
    document.body.append(view.root);
    const textarea = view.root.querySelector("textarea") as HTMLTextAreaElement;
    const pre = view.root.querySelector("pre.screen-pre") as HTMLPreElement;
    const notice = view.root.querySelector(".notice") as HTMLElement;
    expect(textarea.disabled).toBe(true);

    view.start();
    await until(() => !textarea.disabled, "input to become enabled");

    expect(pre.textContent).toContain("apps  crates  docs");
    expect(notice.hidden).toBe(true);
    for (const button of view.root.querySelectorAll("button.composer-send, button.key")) {
      expect((button as HTMLButtonElement).disabled).toBe(false);
    }

    textarea.value = "hello";
    (view.root.querySelector("button.composer-send") as HTMLButtonElement).click();
    await until(() => posts.length === 2, "text and Enter to be posted");
    expect(posts).toEqual(["\x1b[200~hello\x1b[201~", "\r"]);
    expect(textarea.value).toBe("");
  });
});
