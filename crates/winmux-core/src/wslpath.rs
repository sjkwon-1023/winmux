//! WSL 경로 형태 검증 + `\\wsl.localhost` UNC 매핑 (21단계 계획 core 계약).
//!
//! 뷰어 탭(folderBrowser·textViewer)의 파일 접근은 Windows 쪽 Rust 가
//! `\\wsl.localhost\<distro>\...` UNC 경로로 수행한다 — Windows→WSL 방향이라
//! interop 을 잠근 배포판에서도 동작한다 (계획 v2 5장). 이 모듈은 그 매핑의
//! **형태 계약**만 담당하는 순수 함수 모음이다: 파일 실존·권한은 확인하지 않고
//! (코어 무 I/O), 없는 경로는 뷰 로드 실패로 표면화한다.
//!
//! # 거부 규칙
//!
//! [`validate_linux_path`] 는 "사용자가 의도한 것과 다른 Windows 경로가
//! 조립되는 것"을 막는다. 거부 사유와 근거:
//!
//! - **비절대 경로** — 기준 디렉터리가 없어 조립 결과가 정의되지 않는다.
//! - **NUL** — Win32 문자열이 조기 절단돼 다른 경로를 가리킨다.
//! - **백슬래시** — UNC 구분자 밀수. `/a\..\..\Windows` 는 리눅스에선 파일명
//!   하나지만 조립 후 Windows 에서는 공유 루트 밖으로 나간다.
//! - **`.` / `..` 컴포넌트** — 위와 같은 이유의 정공법 버전.
//! - **`:` 를 포함한 컴포넌트** — Windows 가 대체 데이터 스트림(`file:stream`)·
//!   드라이브 지정으로 해석한다.
//! - **점·공백으로 끝나는 컴포넌트** — Win32 가 조용히 절삭해 다른 파일을
//!   가리키는 alias 가 된다 (`foo.` → `foo`).
//!
//! 빈 세그먼트(`//`·후행 `/`)는 거부하지 않고 **정규화**한다 — POSIX 의미가
//! 같은 표기 차이일 뿐이다.
//!
//! 이 규칙 때문에 `\`·`:`·후행 점/공백을 이름에 가진 리눅스 파일은 뷰어로 열 수
//! 없다 — 밀수 차단을 위한 의도된 희생이다 (계획 21단계 리스크 [low]).
//!
//! # 위협 모델 — 심볼릭 링크
//!
//! 링크 해석은 9P 서버(WSL 쪽)가 하므로 이 검증만으로는 링크가 최종적으로
//! 가리키는 대상을 알 수 없다. 뷰어는 **읽기 전용**이고 대상은 사용자 본인
//! 머신의 자기 배포판이라 "링크를 통한 탈출"은 위협 모델이 아니다. 거부 규칙의
//! 목적은 어디까지나 경로 **조립**이 사용자 의도와 어긋나지 않게 하는 것이다.
//!
//! # 알려진 한계 — 경로 길이
//!
//! `\\wsl.localhost\<distro>\...` 는 MAX_PATH(260) 제약을 받는 API 에서 **약
//! 247자**를 넘으면 실패할 수 있다. verbatim 접두(`\\?\UNC\...`)는 MVP 에서
//! 채택하지 않았다 — verbatim 은 Win32 정규화를 통째로 건너뛰어 위 거부 규칙과
//! 별개의 검증 부담을 만든다. 긴 경로 실패는 뷰 로드 에러로 드러난다.

/// WSL2 파일시스템의 UNC 접두 (Windows 11 / 최신 Windows 10 의 표준 이름).
const UNC_PREFIX: &str = r"\\wsl.localhost";

/// 리눅스 절대 경로의 형태를 검증한다. 실패 시 사람이 읽을 사유 문자열
/// (`CommandError::InvalidPath` 의 message 로 그대로 실린다).
///
/// 규칙과 근거는 모듈 rustdoc 참조. 통과한 경로는 [`to_unc`] 로 안전하게 조립할
/// 수 있다는 뜻이고, 그 경로가 **존재한다는 뜻은 아니다**.
pub fn validate_linux_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!("path must be absolute: {path:?}"));
    }
    if path.contains('\0') {
        return Err("path must not contain a NUL byte".to_owned());
    }
    if path.contains('\\') {
        return Err(format!(
            "path must not contain a backslash (Windows separator): {path:?}"
        ));
    }
    for component in path.split('/') {
        // 빈 세그먼트는 정규화 대상 — `//` 와 후행 `/` 를 거부하지 않는다.
        if component.is_empty() {
            continue;
        }
        if component == "." || component == ".." {
            return Err(format!(
                "path must not contain a '.' or '..' component: {path:?}"
            ));
        }
        if component.contains(':') {
            return Err(format!(
                "path component must not contain ':' (Windows stream syntax): {component:?}"
            ));
        }
        if component.ends_with('.') || component.ends_with(' ') {
            return Err(format!(
                "path component must not end with a dot or space (Win32 truncates it): {component:?}"
            ));
        }
    }
    Ok(())
}

/// 검증된 리눅스 절대 경로를 `\\wsl.localhost\<distro>\...` UNC 로 조립한다.
///
/// `distro` 도 검증한다 — 빈 문자열·`/`·`\`·NUL 은 공유 이름 자리를 벗어나므로
/// 거부한다. 빈 세그먼트는 여기서 정규화되고, 루트(`"/"`)는 공유 루트 자체를
/// 가리키는 `\\wsl.localhost\<distro>\` 가 된다 (후행 구분자를 남긴다 —
/// 공유 루트를 디렉터리로 여는 Win32 관례).
pub fn to_unc(distro: &str, linux_path: &str) -> Result<String, String> {
    validate_distro(distro)?;
    validate_linux_path(linux_path)?;
    let mut unc = format!(r"{UNC_PREFIX}\{distro}");
    let mut any = false;
    for component in linux_path.split('/').filter(|c| !c.is_empty()) {
        unc.push('\\');
        unc.push_str(component);
        any = true;
    }
    if !any {
        unc.push('\\');
    }
    Ok(unc)
}

/// UNC 의 공유 이름 자리에 그대로 들어가는 값이라 구분자·NUL·빈 이름을 거부하고,
/// 경로 컴포넌트와 같은 규칙(`.`/`..`, `:`, 후행 점·공백)도 적용한다 — 출처가
/// 본인 설정(env·워크스페이스 필드)이라 위협은 아니지만, 밀수 가드의 커버리지가
/// 경로에만 있고 공유 이름에는 없는 비대칭을 남기지 않는다 (21단계 리뷰 finding).
fn validate_distro(distro: &str) -> Result<(), String> {
    if distro.is_empty() {
        return Err("distro must not be empty".to_owned());
    }
    if distro.contains('/') || distro.contains('\\') || distro.contains('\0') {
        return Err(format!(
            "distro must not contain '/', '\\' or a NUL byte: {distro:?}"
        ));
    }
    if distro == "." || distro == ".." {
        return Err(format!("distro must not be a dot component: {distro:?}"));
    }
    if distro.contains(':') {
        return Err(format!("distro must not contain ':': {distro:?}"));
    }
    if distro.ends_with('.') || distro.ends_with(' ') {
        return Err(format!(
            "distro must not end with a dot or a space (Win32 trims them): {distro:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_absolute_paths() {
        for path in [
            "/",
            "/home",
            "/home/dev/code/winmux",
            "/home/dev/my proj/notes.txt",
            // 컴포넌트 안(끝이 아닌 곳)의 점·공백, 한글·유니코드 이름은 정상.
            "/home/dev/.config/winmux/state.json",
            "/home/dev/보고서 초안.md",
        ] {
            assert_eq!(validate_linux_path(path), Ok(()), "거부됨: {path}");
        }
    }

    #[test]
    fn normalizes_empty_segments_instead_of_rejecting() {
        // POSIX 의미가 같은 표기 차이 — 검증은 통과하고 조립에서 접힌다.
        for path in ["//home//dev/", "/home/dev/", "///"] {
            assert_eq!(validate_linux_path(path), Ok(()), "거부됨: {path}");
        }
        assert_eq!(
            to_unc("Ubuntu", "//home//dev/").unwrap(),
            r"\\wsl.localhost\Ubuntu\home\dev"
        );
        assert_eq!(
            to_unc("Ubuntu", "///").unwrap(),
            r"\\wsl.localhost\Ubuntu\"
        );
    }

    #[test]
    fn rejects_relative_paths() {
        for path in ["", "home/dev", "./x", "../x", "~/code"] {
            let err = validate_linux_path(path).unwrap_err();
            assert!(err.contains("absolute"), "{path:?} → {err}");
        }
    }

    #[test]
    fn rejects_nul_byte() {
        let err = validate_linux_path("/home/dev\0/x").unwrap_err();
        assert!(err.contains("NUL"), "{err}");
    }

    #[test]
    fn rejects_backslash_anywhere() {
        // UNC 구분자 밀수 — 리눅스에선 파일명 한 글자지만 조립 후엔 경로가 된다.
        for path in [r"/home\dev", r"/a\..\..\Windows", r"/trailing\"] {
            let err = validate_linux_path(path).unwrap_err();
            assert!(err.contains("backslash"), "{path:?} → {err}");
        }
    }

    #[test]
    fn rejects_dot_components() {
        for path in ["/home/./dev", "/home/../etc", "/..", "/home/dev/.."] {
            let err = validate_linux_path(path).unwrap_err();
            assert!(err.contains("'.' or '..'"), "{path:?} → {err}");
        }
        // 이름의 **일부**인 점은 이 규칙의 대상이 아니다 — 컴포넌트 전체가
        // `.`/`..` 일 때만 거부한다 (`..hidden` 은 후행 점도 없어 통과).
        assert_eq!(validate_linux_path("/home/..hidden"), Ok(()));
    }

    #[test]
    fn rejects_colon_in_component() {
        // Windows 의 대체 데이터 스트림 문법.
        let err = validate_linux_path("/home/dev/file:stream").unwrap_err();
        assert!(err.contains("':'"), "{err}");
        let err = validate_linux_path("/C:/Windows").unwrap_err();
        assert!(err.contains("':'"), "{err}");
    }

    #[test]
    fn rejects_trailing_dot_or_space_components() {
        // Win32 가 조용히 절삭해 다른 파일의 alias 가 되는 형태.
        for path in ["/home/dev/foo.", "/home/dev /x", "/home/dev/foo "] {
            let err = validate_linux_path(path).unwrap_err();
            assert!(err.contains("dot or space"), "{path:?} → {err}");
        }
    }

    #[test]
    fn to_unc_assembles_share_path() {
        assert_eq!(
            to_unc("Ubuntu-24.04", "/home/dev/code/winmux").unwrap(),
            r"\\wsl.localhost\Ubuntu-24.04\home\dev\code\winmux"
        );
        // 루트는 공유 루트 자체 — 후행 구분자를 남긴다.
        assert_eq!(to_unc("Ubuntu", "/").unwrap(), r"\\wsl.localhost\Ubuntu\");
        // 공백을 포함한 이름은 그대로 (인용은 호출자·API 몫이 아니라 불필요).
        assert_eq!(
            to_unc("Ubuntu", "/home/my proj/a.txt").unwrap(),
            r"\\wsl.localhost\Ubuntu\home\my proj\a.txt"
        );
    }

    #[test]
    fn to_unc_rejects_bad_distro() {
        // 경로 컴포넌트와 같은 밀수 가드를 공유 이름에도 적용한다 (커버리지 대칭).
        for distro in [
            "",
            "Ubuntu/x",
            r"Ubuntu\x",
            "Ubu\0ntu",
            ".",
            "..",
            "Ubuntu:22",
            "Ubuntu.",
            "Ubuntu ",
        ] {
            let err = to_unc(distro, "/home").unwrap_err();
            assert!(err.contains("distro"), "{distro:?} → {err}");
        }
    }

    #[test]
    fn to_unc_propagates_path_rejection() {
        let err = to_unc("Ubuntu", "home/dev").unwrap_err();
        assert!(err.contains("absolute"), "{err}");
    }
}
