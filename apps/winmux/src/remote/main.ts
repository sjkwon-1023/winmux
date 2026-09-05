// 폰 페이지의 진입점 — 부트(토큰 수령), 목록/탭 두 화면의 전환, 가시성 게이트.
//
// 토큰은 페어링 URL 의 **fragment**(`#t=…`)로 들어온다. fragment 는 요청에
// 실리지 않으므로 서버 로그·프록시에 남지 않고, 우리는 그것을 localStorage 로
// 옮긴 뒤 `history.replaceState` 로 주소창에서 지운다 — 지우지 않으면 화면 공유나
// 스크린샷 한 장으로 토큰이 새고, 새로고침·뒤로가기마다 다시 붙는다.

import "@xterm/xterm/css/xterm.css";
import "./remote.css";

import { fetchState, HttpError, loadToken, saveToken } from "./api";
import { ListView } from "./list-view";
import { PollSchedule } from "./poller";
import { TabView } from "./tab-view";
import type { TabId } from "../types";

const STATE_POLL_INTERVAL_MS = 2000;

/** URL fragment 의 토큰을 저장소로 옮기고 주소창에서 지운다. */
function claimTokenFromFragment(): void {
  const hash = window.location.hash;
  if (!hash.startsWith("#")) return;
  const token = new URLSearchParams(hash.slice(1)).get("t");
  if (token === null || token === "") return;
  saveToken(token);
  window.history.replaceState(null, "", window.location.pathname + window.location.search);
}

class RemoteApp {
  private readonly listView: ListView;
  private readonly listSchedule: PollSchedule;
  private tabView: TabView | null = null;

  constructor(private readonly root: HTMLElement) {
    this.listView = new ListView({ onOpenTab: (tab, title) => this.openTab(tab, title) });
    this.listSchedule = new PollSchedule({
      intervalMs: STATE_POLL_INTERVAL_MS,
      poll: () => this.pollState(),
      onHalt: (reason) => {
        this.listView.setNotice(
          reason === "unauthorized"
            ? "Not authorized — scan the pairing QR in winmux again."
            : "Too many requests — retrying in a minute.",
        );
      },
    });
    document.addEventListener("visibilitychange", () => this.applyVisibility());
  }

  start(): void {
    this.showList();
    this.listSchedule.start();
    this.applyVisibility();
  }

  private async pollState(): Promise<void> {
    try {
      this.listView.render(await fetchState());
      this.listView.setNotice(null);
    } catch (error) {
      if (error instanceof HttpError) {
        this.listSchedule.noteStatus(error.status);
        if (error.status !== 401 && error.status !== 429) {
          this.listView.setNotice(`winmux replied ${error.status}.`);
        }
        return;
      }
      this.listView.setNotice("Could not reach winmux — retrying.");
    }
  }

  private showList(): void {
    this.tabView?.dispose();
    this.tabView = null;
    this.root.replaceChildren(this.listView.root);
    this.applyVisibility();
  }

  private openTab(tab: TabId, title: string): void {
    this.tabView?.dispose();
    const view = new TabView({ tab, title, onBack: () => this.showList() });
    this.tabView = view;
    this.root.replaceChildren(view.root);
    view.start();
    this.applyVisibility();
  }

  /** 화면에 보이지 않는 쪽은 폴링하지 않는다 — 가려진 문서 전체도 마찬가지다. */
  private applyVisibility(): void {
    const shown = !document.hidden;
    this.listSchedule.setVisible(shown && this.tabView === null);
    this.tabView?.setVisible(shown);
  }
}

function showPairingHint(root: HTMLElement): void {
  const hint = document.createElement("div");
  hint.className = "hint";
  const line = document.createElement("p");
  line.textContent = "Scan the pairing QR in winmux";
  const detail = document.createElement("p");
  detail.className = "hint-detail";
  detail.textContent = "Open the sidebar and press “Pair phone” on the desktop app.";
  hint.append(line, detail);
  root.replaceChildren(hint);
}

const root = document.getElementById("app");
if (root !== null) {
  claimTokenFromFragment();
  if (loadToken() === null) showPairingHint(root);
  else new RemoteApp(root).start();
}
