// folderBrowser 탭의 뷰 (21단계 청크 C1) — fs_list_dir 로 읽은 항목을 dirs-first
// 로 나열하고, 디렉터리 클릭은 NavigateFolder, 파일 클릭은 뷰어 탭 생성
// (확장자에 따라 markdownViewer 또는 textViewer — 청크 D)으로 잇는다.
//
// 탐색이 뷰 내부 상태가 아니라 **dispatcher 명령**인 것이 이 파일의 핵심 계약이다
// (계획 v2 4장 + 21단계 계획): 현재 경로는 모델(TabKind::FolderBrowser.path)이
// 소유하고, 뷰는 스냅샷이 내려준 kind 를 그릴 뿐이다. 그래서 앱을 재시작해도
// 경로가 복원되고(persist), 어떤 탐색이든 revision 을 남긴다. 파일 **내용** 읽기
// (fs_*)만 attach_terminal 류 콘텐츠 플레인 직접 invoke 다.
//
// 행 모델(folderRows)·정렬(sortEntries)·부모 경로(parentPath)·확장자 라우팅
// (viewerTabForPath)은 DOM 무의존 순수 함수로 분리해 vitest 로 잠근다 — 이
// 파일에서 DOM 을 만지는 부분은 그 결과를 그대로 그리는 얇은 층이다.

import { fsListDir } from "./backend";
import type { DirEntry, DirListing } from "./backend";
import type { ViewerKind, ViewerView } from "./viewer-view";
import type { Command, CommandOutput, NewTab, PaneId, TabId } from "./types";

/** UI 발 dispatch — main.ts dispatchUI 래퍼 (실패는 상태 라인에 표면화되고 null). */
type DispatchFn = (cmd: Command) => Promise<CommandOutput | null>;

/** 화면에 그릴 행 1개. path 는 그 행이 가리키는 **절대 경로**(부모 행 포함)이고,
 *  parent 는 `..` 행 표시다. */
export interface FolderRow {
  label: string;
  path: string;
  isDir: boolean;
  size: number | null;
  parent: boolean;
}

/** 부모 디렉터리 경로 — 루트("/")면 null. 빈 세그먼트(`//`·후행 `/`)는 무시한다
 *  (코어 wslpath 의 정규화 규칙과 같은 취급). `..` 를 경로에 넣지 않고 여기서
 *  잘라내는 이유: 코어 validate_linux_path 가 `..` 컴포넌트를 거부한다. */
export function parentPath(path: string): string | null {
  const parts = path.split("/").filter((p) => p.length > 0);
  if (parts.length === 0) return null;
  parts.pop();
  return `/${parts.join("/")}`;
}

/** 디렉터리 경로 + 항목명 → 자식 절대 경로 (후행 `/` 중복 방지). */
export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;
}

/** dirs-first · name asc 정렬 (순수 — 백엔드는 fs 순서 그대로 준다).
 *
 *  localeCompare 는 쓰지 않는다: WebView 의 ICU 로케일에 따라 결과가 달라져
 *  테스트로 고정할 수 없다. 대소문자 무시 비교를 먼저 하고(사람이 기대하는
 *  순서), 같으면 코드포인트 순으로 안정적인 tiebreak 을 둔다. */
export function sortEntries(entries: readonly DirEntry[]): DirEntry[] {
  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    const la = a.name.toLowerCase();
    const lb = b.name.toLowerCase();
    if (la !== lb) return la < lb ? -1 : 1;
    if (a.name !== b.name) return a.name < b.name ? -1 : 1;
    return 0;
  });
}

/** 현재 경로 + 항목 목록 → 행 모델. 루트가 아니면 맨 앞에 `..` 행이 붙는다. */
export function folderRows(path: string, entries: readonly DirEntry[]): FolderRow[] {
  const rows: FolderRow[] = [];
  const parent = parentPath(path);
  if (parent !== null) {
    rows.push({ label: "..", path: parent, isDir: true, size: null, parent: true });
  }
  for (const entry of sortEntries(entries)) {
    rows.push({
      // 디렉터리는 이름 뒤에 `/` 를 붙여 한눈에 구분되게 한다.
      label: entry.is_dir ? `${entry.name}/` : entry.name,
      path: joinPath(path, entry.name),
      isDir: entry.is_dir,
      size: entry.is_dir ? null : entry.size,
      parent: false,
    });
  }
  return rows;
}

/** 파일 클릭이 만드는 뷰어 탭 명세 (순수 — 21단계 청크 D).
 *
 *  `.md`/`.markdown` 만 markdownViewer 로, 나머지는 전부 textViewer 로 연다.
 *  확장자는 **basename 의 마지막 점 뒤**로 본다: 선두 점은 dotfile 표시이지
 *  확장자가 아니므로 `.md` 라는 이름의 파일은 텍스트로 연다. */
export function viewerTabForPath(path: string): NewTab {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot) : "";
  return ext === ".md" || ext === ".markdown"
    ? { type: "markdownViewer", path }
    : { type: "textViewer", path };
}

/** 파일 크기 표시 (1024 진법, 정수는 소수점 없이). null 이면 빈 문자열. */
export function formatSize(size: number | null): string {
  if (size === null) return "";
  const units = ["B", "K", "M", "G", "T"];
  let value = size;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = unit === 0 ? String(value) : value.toFixed(value < 10 ? 1 : 0);
  return `${text}${units[unit]}`;
}

/** 로드 실패 payload 의 표시 문자열 — 글루는 `Result<_, String>` 이라 문자열이
 *  오지만, IPC 레벨 실패 등 계약 밖 값도 삼키지 않는다. */
function describeError(err: unknown): string {
  return typeof err === "string" ? err : String(err);
}

export class FolderView implements ViewerView {
  readonly root: HTMLDivElement;
  private readonly bannerEl: HTMLDivElement;
  private readonly listEl: HTMLDivElement;
  private path: string;
  private disposed = false;
  /** in-flight 로드 토큰 — 늦게 도착한 이전 경로의 응답이 현재 목록을 덮지
   *  않게 한다 (탐색을 빠르게 연타하면 순서가 뒤집힐 수 있다). */
  private loadToken = 0;

  constructor(
    parent: HTMLElement,
    private readonly tab: TabId,
    /** 파일 클릭으로 만드는 textViewer 탭이 들어갈 pane — 같은 pane 이다
     *  (CreateTab 시맨틱상 새 탭이 곧바로 active 가 된다). */
    private readonly pane: PaneId,
    /** 워크스페이스 distro (없으면 null — 글루가 WMUX_DISTRO·기본 배포판 순으로
     *  해석한다). 워크스페이스 생성 후 바뀌지 않는 값이라 생성 시 고정한다. */
    private readonly distro: string | null,
    kind: ViewerKind,
    private readonly dispatch: DispatchFn,
  ) {
    this.path = kind.type === "folderBrowser" ? kind.path : "/";

    this.root = document.createElement("div");
    this.root.className = "folder-view";
    // 프로그램적 focus 대상 (D7 보상 경로) — Tab 순서에는 넣지 않는다.
    this.root.tabIndex = -1;

    this.bannerEl = document.createElement("div");
    this.bannerEl.className = "folder-banner";
    this.bannerEl.hidden = true;

    this.listEl = document.createElement("div");
    this.listEl.className = "folder-list";

    this.root.append(this.bannerEl, this.listEl);
    parent.appendChild(this.root);

    this.load();
  }

  /** 스냅샷 반영 — 경로가 바뀐 경우(NavigateFolder)만 다시 읽는다. 무변경
   *  렌더마다 재목록하면 매 revision 이 9P 왕복이 된다. */
  update(kind: ViewerKind): void {
    if (kind.type !== "folderBrowser") {
      // 탭의 kind 종류는 생성 후 바뀌지 않는다 — 오면 레지스트리 배선 결함이다.
      console.error("[wmux] folder view received a non-folderBrowser kind", kind);
      return;
    }
    if (kind.path === this.path) return;
    this.path = kind.path;
    this.load();
  }

  /** folderBrowser 는 모델에 스크롤 위치가 없다 (setViewerScroll 대상이 되면
   *  kindMismatch) — 기록할 것이 없어 no-op 이다. */
  flushScroll(): void {}

  focus(): void {
    this.root.focus();
  }

  dispose(): void {
    this.disposed = true;
    this.root.remove();
  }

  private load(): void {
    const token = ++this.loadToken;
    this.setBanner("loading…", false);
    // 로드 중에도 이전 목록을 그대로 둔다 — 깜빡임 없이 배너만 바뀐다.
    fsListDir(this.distro, this.path).then(
      (listing) => {
        if (this.disposed || token !== this.loadToken) return;
        this.renderListing(listing);
      },
      (err: unknown) => {
        if (this.disposed || token !== this.loadToken) return;
        this.renderError(err);
      },
    );
  }

  private renderListing(listing: DirListing): void {
    this.setBanner(
      listing.truncated
        ? `showing the first ${listing.entries.length} entries — this directory was truncated`
        : null,
      false,
    );
    this.renderRows(folderRows(this.path, listing.entries));
  }

  /** 로드 실패는 인라인 에러다 — 탭을 닫거나 경로를 되돌리지 않는다 (없는 경로도
   *  모델에 남아 재시도·수정이 가능해야 한다). 막다른 길이 되지 않게 `..` 행은
   *  남긴다. */
  private renderError(err: unknown): void {
    this.setBanner(`cannot list ${this.path}: ${describeError(err)}`, true);
    this.renderRows(folderRows(this.path, []));
  }

  private setBanner(text: string | null, error: boolean): void {
    this.bannerEl.textContent = text ?? "";
    this.bannerEl.hidden = text === null;
    this.bannerEl.classList.toggle("error", error);
  }

  private renderRows(rows: FolderRow[]): void {
    this.listEl.replaceChildren(...rows.map((row) => this.rowButton(row)));
  }

  private rowButton(row: FolderRow): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = row.isDir ? "folder-row dir" : "folder-row";

    const name = document.createElement("span");
    name.className = "folder-row-name";
    name.textContent = row.label;
    name.title = row.path;

    const size = document.createElement("span");
    size.className = "folder-row-size";
    size.textContent = formatSize(row.size);

    btn.append(name, size);
    btn.addEventListener("click", () => this.onRowClick(row));
    return btn;
  }

  /** 디렉터리는 탐색(모델 경유), 파일은 같은 pane 에 뷰어 탭 생성 — 확장자로
   *  markdownViewer / textViewer 를 고른다 (viewerTabForPath).
   *  실패(없는 경로·kindMismatch 등)는 dispatchUI 가 상태 라인에 표면화한다. */
  private onRowClick(row: FolderRow): void {
    if (row.isDir) {
      void this.dispatch({ type: "navigateFolder", tab: this.tab, path: row.path });
      return;
    }
    void this.dispatch({
      type: "createTab",
      pane: this.pane,
      tab: viewerTabForPath(row.path),
    });
  }
}
