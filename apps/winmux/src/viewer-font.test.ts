// @vitest-environment happy-dom
//
// 뷰어 글꼴 적용의 두 반쪽을 잠근다.
//
// 1. **TS 쪽** — 설정이 있으면 :root 에 커스텀 프로퍼티를 심고, 없으면 **심지
//    않는다**(지운다). "없으면 심지 않는다"가 곧 "미설정이면 화면이 종전과
//    똑같다"의 근거라, 프로퍼티의 유무 자체가 계약이다.
// 2. **CSS 쪽** — 그 fallback 이 실제로 종전 하드코딩 값인지, 그리고 변수를 읽는
//    셀렉터가 **모노스페이스 콘텐츠 표면뿐**인지. happy-dom 은 var() fallback 을
//    계산해 주지 않으므로 계산 결과 대신 **스타일시트 원문**을 단언한다 —
//    fallback 이 12px 아닌 값으로 바뀌거나(미설정 화면이 달라진다), 사이드바·
//    탭바 같은 UI 크롬이 이 변수를 읽기 시작하면(범위 확장) 여기서 깨진다.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_VIEWER_FONT_SIZE, applyViewerFontSettings, viewerFontSize } from "./viewer-font";
import type { UiSettings } from "./backend";

const FAMILY_VAR = "--viewer-font-family";
const SIZE_VAR = "--viewer-font-size";

function settings(fontFamily: string | null, fontSize: number | null): UiSettings {
  return { fontFamily, fontSize, highlightLanguages: null };
}

function rootValue(name: string): string {
  return document.documentElement.style.getPropertyValue(name);
}

describe("applyViewerFontSettings", () => {
  beforeEach(() => {
    // 미설정 상태로 되돌린다 — 이 함수가 total 이라 이것만으로 충분하다.
    applyViewerFontSettings(settings(null, null));
  });

  it("미설정이면 :root 에 아무 것도 심지 않는다 (CSS fallback 이 산다)", () => {
    expect(rootValue(FAMILY_VAR)).toBe("");
    expect(rootValue(SIZE_VAR)).toBe("");
    expect(viewerFontSize()).toBe(DEFAULT_VIEWER_FONT_SIZE);
  });

  it("설정이 있으면 :root 에 심는다 — 크기는 px 단위로", () => {
    applyViewerFontSettings(settings("Cascadia Code, monospace", 15));
    expect(rootValue(FAMILY_VAR)).toBe("Cascadia Code, monospace");
    expect(rootValue(SIZE_VAR)).toBe("15px");
    expect(viewerFontSize()).toBe(15);
  });

  it("한쪽만 설정하면 다른 쪽은 심기지 않는다", () => {
    applyViewerFontSettings(settings(null, 20));
    expect(rootValue(FAMILY_VAR)).toBe("");
    expect(rootValue(SIZE_VAR)).toBe("20px");
    expect(viewerFontSize()).toBe(20);

    applyViewerFontSettings(settings("Consolas", null));
    expect(rootValue(FAMILY_VAR)).toBe("Consolas");
    expect(rootValue(SIZE_VAR)).toBe("");
    expect(viewerFontSize()).toBe(DEFAULT_VIEWER_FONT_SIZE);
  });

  it("설정을 지우면 프로퍼티도 지워진다 (빈 값으로 남지 않는다)", () => {
    applyViewerFontSettings(settings("Consolas", 18));
    applyViewerFontSettings(settings(null, null));
    // 빈 문자열로 "설정됨" 상태가 되면 var() 의 fallback 이 발동하지 않으므로,
    // 값이 비었는지가 아니라 **선언 자체가 빠졌는지**를 본다.
    const inline = document.documentElement.getAttribute("style") ?? "";
    expect(inline).not.toContain(FAMILY_VAR);
    expect(inline).not.toContain(SIZE_VAR);
  });
});

describe("styles.css 의 뷰어 글꼴 계약", () => {
  /** 주석을 걷어낸 스타일시트 원문 — 주석 안의 변수 언급이 아래 판정에 섞이지
   *  않게 한다 (설명 주석이 변수 이름을 그대로 적고 있다).
   *
   *  파일을 직접 읽는 이유: vitest 는 CSS import 를 빈 모듈로 지워버려(`css:
   *  false` 기본값) `?raw` 로도 원문이 오지 않는다. 경로는 실행 cwd 가 아니라 이
   *  파일 자신에서 잡는다. */
  const css = readFileSync(
    fileURLToPath(import.meta.url).replace(/[^/\\]+$/, "styles.css"),
    "utf8",
  ).replace(/\/\*[\s\S]*?\*\//g, "");

  /** `selector { body }` 쌍 (이 스타일시트는 @media 등 중첩 at-rule 이 없다). */
  function rules(): { selector: string; body: string }[] {
    const out: { selector: string; body: string }[] = [];
    const re = /([^{}]+)\{([^{}]*)\}/g;
    let match = re.exec(css);
    while (match !== null) {
      out.push({ selector: match[1].trim(), body: match[2] });
      match = re.exec(css);
    }
    return out;
  }

  it("스타일시트는 두 변수를 선언하지 않는다 — 선언하면 fallback 이 죽는다", () => {
    for (const rule of rules()) {
      expect(rule.body).not.toMatch(new RegExp(`${FAMILY_VAR}\\s*:`));
      expect(rule.body).not.toMatch(new RegExp(`${SIZE_VAR}\\s*:`));
    }
  });

  it("fallback 은 종전 하드코딩 값 그대로다 (미설정 화면이 달라지지 않는다)", () => {
    // 변수를 읽는 모든 자리에 fallback 이 있어야 한다 — 빠진 자리는 설정이
    // 없을 때 상속값으로 떨어져 화면이 달라진다.
    const familyUses = [...css.matchAll(new RegExp(`var\\(\\s*${FAMILY_VAR}[^)]*\\)`, "g"))];
    const sizeUses = [...css.matchAll(new RegExp(`var\\(\\s*${SIZE_VAR}[^)]*\\)`, "g"))];
    expect(familyUses.length).toBeGreaterThan(0);
    expect(sizeUses.length).toBeGreaterThan(0);
    for (const [use] of familyUses) expect(use).toBe(`var(${FAMILY_VAR}, monospace)`);
    for (const [use] of sizeUses) {
      expect(use).toBe(`var(${SIZE_VAR}, ${DEFAULT_VIEWER_FONT_SIZE}px)`);
    }
  });

  it("변수를 읽는 곳은 모노스페이스 콘텐츠 표면뿐이다 (UI 크롬 비대상)", () => {
    const readers = rules()
      .filter((rule) => rule.body.includes("var(--viewer-font-"))
      .map((rule) => rule.selector);
    expect(new Set(readers)).toEqual(
      new Set([".text-lines", ".text-line", ".folder-row", ".folder-row-size", ".markdown-body code"]),
    );
  });
});
