// 뷰어 글꼴 — settings.json 의 `fontFamily`/`fontSize` 를 뷰어의 **모노스페이스
// 콘텐츠 표면**에도 적용한다.
//
// 종전에는 이 두 값이 xterm 옵션에만 들어가서(terminal-view.ts
// applyTerminalSettings) 텍스트 뷰어의 행·폴더 목록·마크다운 code 는 styles.css 에
// 하드코딩된 `monospace` 12px 로 남았다 — 글꼴을 바꾼 사용자에게는 터미널만
// 바뀌고 뷰어는 그대로인 것이 결함으로 보인다 (필드 리포트). 설정 한 쌍이
// "이 앱에서 코드·로그를 읽을 때 쓰는 글꼴"을 뜻하도록 소비 범위를 넓힌다.
//
// 적용 방식은 **:root 커스텀 프로퍼티 한 쌍**이다. 부팅 1회 경로가 값을 심고
// styles.css 가 `var(--viewer-font-family, monospace)` 꼴로 읽는다.
//
// - **미설정이면 아무 것도 심지 않는다** — 프로퍼티가 없으면 var() 의 fallback,
//   즉 종전 하드코딩 값이 그대로 쓰여 화면이 한 픽셀도 달라지지 않는다. 기본값을
//   CSS 와 TS 양쪽에 "심는" 구조로 만들지 않는 이유이기도 하다: 화면 기본값의
//   정본은 CSS 한 곳뿐이다.
// - **대상은 모노스페이스 콘텐츠뿐이다** — 사이드바·탭바·버튼·상태줄 같은 UI
//   크롬과 마크다운 산문은 사용자가 고른 "코드 글꼴"의 대상이 아니다. 어떤
//   셀렉터가 이 변수를 읽는지는 styles.css 가 정본이다.
// - **터미널은 이 경로를 타지 않는다** — xterm 은 자기 캔버스에 직접 그려 CSS 가
//   닿지 않으므로 종전대로 applyTerminalSettings 가 옵션에 넣는다. 두 경로가 같은
//   설정값을 읽지만 표면이 겹치지 않아 이중 적용이 아니다.
//
// **Ctrl+= / Ctrl+- 줌은 여기 오지 않는다 (의도적 결정).** 줌은 터미널 전용의
// 세션 한정 조작이고(백로그 2026-08-12), 뷰어는 settings.json 값에 고정된다.
// 뷰어까지 넓히려면 런타임 줌마다 가상 스크롤의 행 격자(text-view 의
// LINE_HEIGHT_PX 배수로 잡힌 spacer 높이·scrollTop)를 열려 있는 모든 뷰에서 다시
// 앉혀야 하는데, 그건 이 결함("설정이 뷰어에 안 먹는다")이 요구하는 범위가 아니다.

import type { UiSettings } from "./backend";

/** 뷰어 모노스페이스 콘텐츠의 기본 글자 크기(px).
 *
 *  **styles.css 미러 계약**: `var(--viewer-font-size, 12px)` 를 쓰는 셀렉터들의
 *  fallback 과 같은 값이어야 한다. 화면의 정본은 CSS 쪽이고(미설정이면 이 모듈은
 *  아무 것도 심지 않는다), 이 상수는 **행 격자 계산**(text-view 의
 *  lineHeightForFontSize)이 "지금 몇 px 로 그려지는가"를 알아야 해서 있다 — 둘이
 *  갈라지면 행 높이가 글자 크기와 어긋나 행이 잘리거나 벌어진다. */
export const DEFAULT_VIEWER_FONT_SIZE = 12;

/** 지금 뷰어가 그려지는 글자 크기(px) — 부팅 시 applyViewerFontSettings 가 정한다. */
let fontSize = DEFAULT_VIEWER_FONT_SIZE;

/** 뷰어 글자 크기(px). 픽셀 격자를 **직접 계산**하는 뷰(text-view 의 가상
 *  스크롤)만 쓴다 — 글꼴만 따르면 되는 표면은 CSS 변수로 충분하다. */
export function viewerFontSize(): number {
  return fontSize;
}

/** settings.json 의 글꼴 설정을 뷰어 표면에 적용한다 (main.ts 부트가 **뷰 생성
 *  전에** 1회 호출 — applyTerminalSettings·applyHighlightSettings 와 같은 자리·
 *  같은 순서 계약: 뷰는 첫 스냅샷 렌더부터 생기므로 그 전에 심어야 모든 탭이 같은
 *  글꼴로 열린다).
 *
 *  값 검증은 백엔드(`get_ui_settings`)가 이미 했다 — 여기서 다시 판정하면 두 곳의
 *  규칙이 갈라진다 (형제 함수들과 같은 규율).
 *
 *  **null 은 "기본값으로"다.** 형제 함수들은 null 을 "현재 값 유지"로 다루지만,
 *  이 호출은 부팅 1회이고 그때의 현재 값이 곧 기본값이라 결과가 같다. 대신 함수가
 *  total 이라 프로퍼티를 심지 않은 상태(= CSS fallback)로 되돌릴 수 있다.
 *
 *  **부팅 뒤에 다시 부르지 말 것** (설정 리로드나 뷰어 줌을 붙일 때 첫 번째로
 *  밟는 지뢰다). 문법적으로는 언제든 다시 부를 수 있지만, 그러면 **이미 떠 있는
 *  TextView 가 조용히 어긋난다**: `--viewer-font-size` 는 라이브 CSS 변수라 글리프
 *  는 즉시 커지는데, 가상 스크롤의 행 격자(text-view.ts 의 `lineHeight`)는 뷰
 *  **생성 시 스냅샷**이라 spacer 높이·scrollTop 이 옛 크기에 남는다 — 글자가 행을
 *  넘치고 스크롤 위치가 밀린다. 런타임 재적용을 지원하려면 살아 있는 뷰에 새
 *  행높이를 다시 앉히는 경로(터미널 줌의 liveViews 레지스트리에 해당하는 것)를
 *  먼저 만들어야 한다. */
export function applyViewerFontSettings(settings: UiSettings): void {
  const root = document.documentElement;
  setVariable(root, "--viewer-font-family", settings.fontFamily);
  setVariable(
    root,
    "--viewer-font-size",
    settings.fontSize === null ? null : `${settings.fontSize}px`,
  );
  fontSize = settings.fontSize ?? DEFAULT_VIEWER_FONT_SIZE;
}

/** 값이 있으면 심고, 없으면 **지운다**. 빈 문자열을 심는 것과 다르다 — 빈 값도
 *  "설정됨"이라 var() 의 fallback 이 발동하지 않고 선언 자체가 무효가 된다. */
function setVariable(root: HTMLElement, name: string, value: string | null): void {
  if (value === null) root.style.removeProperty(name);
  else root.style.setProperty(name, value);
}
