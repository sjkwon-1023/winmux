//! `SessionHost` 구현 — Dispatcher 의 PTY 부수효과 포트를 `SessionManager` 위에
//! 실현한다. `dispatch` 가 Dispatcher lock 아래에서 호출하므로(스폰 포함 — 계획
//! 0-3 수용) 여기서 Dispatcher lock 을 다시 잡는 코드는 금지다.

use std::cell::Cell;
use std::sync::Arc;

use tauri::AppHandle;
use winmux_core::command::{SessionHost, ShellSpawnReq};
use winmux_core::session::{SessionId, SessionManager, SessionOptions, SpawnSpec};

use crate::router::OscRouter;
use crate::sink::{SinkHandle, TerminalSink};
use crate::state::SinkRegistry;

/// Tauri 앱의 `SessionHost`. Dispatcher 가 소유하며(`Box<dyn SessionHost>`),
/// 세션·sink 레지스트리는 관리 상태(`AppState`)와 `Arc` 로 공유한다.
pub struct TauriHost {
    app: AppHandle,
    sessions: Arc<SessionManager>,
    sinks: Arc<SinkRegistry>,
    /// 새로 만드는 sink 에 물려 줄 OSC 라우터 핸들 (18단계 glue 계약).
    router: Arc<OscRouter>,
}

impl TauriHost {
    pub fn new(
        app: AppHandle,
        sessions: Arc<SessionManager>,
        sinks: Arc<SinkRegistry>,
        router: Arc<OscRouter>,
    ) -> Self {
        Self {
            app,
            sessions,
            sinks,
            router,
        }
    }
}

/// `ShellSpawnReq` → 플랫폼별 `SpawnSpec` 매핑.
///
/// **주의: `req.cwd` 는 Linux(WSL) 경로다.** Windows 에서 `SpawnSpec.cwd`(Windows
/// 프로세스의 cwd)에 넣으면 안 되고 `wsl.exe --cd` 인자로 넘겨 WSL 안에서
/// 해석되게 한다. unix(개발 실행)에서는 프로세스 cwd 그대로가 맞다.
fn spawn_spec(req: &ShellSpawnReq) -> SpawnSpec {
    #[cfg(windows)]
    {
        let mut args: Vec<String> = Vec::new();
        if let Some(cwd) = &req.cwd {
            args.push("--cd".to_string());
            args.push(cwd.clone());
        }
        // distro 우선순위: req.distro → env WINMUX_DISTRO → 없음(WSL 기본 배포판).
        // 빈 문자열은 미설정으로 취급한다.
        let distro = req
            .distro
            .clone()
            .filter(|d| !d.is_empty())
            .or_else(|| std::env::var("WINMUX_DISTRO").ok().filter(|d| !d.is_empty()));
        if let Some(distro) = distro {
            args.push("-d".to_string());
            args.push(distro);
        }
        args.push("--".to_string());
        args.extend(bash_argv(req.history_tab));
        SpawnSpec {
            program: "wsl.exe".to_string(),
            args,
            cwd: None,
            cols: req.cols,
            rows: req.rows,
        }
    }
    #[cfg(not(windows))]
    {
        // unix(개발): $SHELL -l ($SHELL 없으면 bash -l), cwd 는 직접 사용.
        // 탭별 HISTFILE 은 여기서 적용하지 않는다 — $SHELL 이 bash 라는 보장이
        // 없어(zsh·fish 는 HISTFILE 시맨틱이 다르다) 셸 기본 history 를 그대로
        // 쓴다. 탭별 history 는 WSL(bash 고정) 경로 한정 기능이다.
        let program = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bash".to_string());
        SpawnSpec {
            program,
            args: vec!["-l".to_string()],
            cwd: req.cwd.clone(),
            cols: req.cols,
            rows: req.rows,
        }
    }
}

/// `wsl.exe ... --` 뒤에 붙는 **WSL 안 셸 argv**.
///
/// `history_tab` 이 Some 이면 그 탭 전용 HISTFILE 을 물린 로그인 셸을 띄운다 —
/// 탭의 안정 ID 는 재시작을 넘어 유지되므로 재시작 후에도 같은 탭의 history 만
/// 복원된다 (체크포인트 2 UX 요청). 셸 안에서 `mkdir -p` 로 디렉터리를 만드는 이유
/// 는 Windows 쪽에서 WSL 파일시스템 경로를 추측하지 않기 위해서이고, `$HOME` 은
/// 공백이 섞여도 안전하도록 따옴표로 감싼다. `VAR=... exec bash -l` 의 할당은
/// exec 되는 프로세스의 환경으로 전달되며, **Ubuntu 기본 `.bashrc`/`.profile` 은
/// HISTSIZE 류만 손대고 HISTFILE 은 덮지 않으므로** 이 env 상속만으로 충분하다.
/// mkdir 이 실패하면 `&&` 가 끊겨 셸이 그대로 종료된다 — 탭이 exited 로 표시돼
/// 실패가 드러난다 (조용한 fallback 없음).
#[cfg(windows)]
fn bash_argv(history_tab: Option<u64>) -> Vec<String> {
    let Some(tab) = history_tab else {
        return vec!["bash".to_string(), "-l".to_string()];
    };
    let script = format!(
        "mkdir -p \"$HOME/.winmux/history\" \
         && HISTFILE=\"$HOME/.winmux/history/tab-{tab}\" exec bash -l"
    );
    vec!["bash".to_string(), "-c".to_string(), script]
}

impl SessionHost for TauriHost {
    fn spawn_shell(&self, req: ShellSpawnReq) -> anyhow::Result<SessionId> {
        let spec = spawn_spec(&req);
        // sink factory 가 TerminalSink 를 만들어 레지스트리에 등록한다 (id
        // 선발급 계약). 등록 후 스폰이 실패하면 레지스트리 엔트리를 되감아
        // 고아 sink 를 남기지 않는다 — factory 밖으로 id 를 꺼내는 Cell.
        let registered: Cell<Option<SessionId>> = Cell::new(None);
        let result = self.sessions.create(spec, SessionOptions::default(), |id| {
            let sink = Arc::new(TerminalSink::new(
                id,
                self.app.clone(),
                Arc::clone(&self.router),
            ));
            self.sinks.insert(id, Arc::clone(&sink));
            registered.set(Some(id));
            Box::new(SinkHandle(sink))
        });
        if result.is_err() {
            if let Some(id) = registered.take() {
                self.sinks.remove(id);
            }
        }
        result
    }

    fn kill(&self, id: SessionId) {
        // 멱등 계약 (SessionHost rustdoc): 미지·이미 종료된 id 도 무해 —
        // 양쪽 레지스트리 remove 모두 no-op 으로 끝난다. `SessionManager::remove`
        // 는 레지스트리 lock 을 놓은 뒤 kill 신호를 보낸다 (코어 계약).
        self.sinks.remove(id);
        let _ = self.sessions.remove(id);
    }
}

/// 스폰 명령 구성 테스트 — Windows 대상에서만 성립하는 argv 계약이라 그 타깃에서만
/// 컴파일·실행된다 (unix 개발 경로는 `$SHELL -l` 무변경).
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn history_tab_wraps_bash_with_per_tab_histfile() {
        let req = ShellSpawnReq {
            cwd: Some("/home/me/proj".to_string()),
            distro: Some("Ubuntu".to_string()),
            history_tab: Some(7),
            ..ShellSpawnReq::default()
        };
        let spec = spawn_spec(&req);
        assert_eq!(spec.program, "wsl.exe");
        // cwd·distro 전달 방식은 그대로 (--cd / -d), 그 뒤에 셸 argv 가 붙는다.
        assert_eq!(
            spec.args,
            vec![
                "--cd".to_string(),
                "/home/me/proj".to_string(),
                "-d".to_string(),
                "Ubuntu".to_string(),
                "--".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                "mkdir -p \"$HOME/.winmux/history\" \
                 && HISTFILE=\"$HOME/.winmux/history/tab-7\" exec bash -l"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn without_history_tab_the_shell_argv_is_plain_login_bash() {
        let spec = spawn_spec(&ShellSpawnReq::default());
        // history_tab 이 없으면 기존 그대로 `-- bash -l` (셸 기본 history).
        // 앞부분은 검사하지 않는다 — env WINMUX_DISTRO 가 `-d` 를 덧붙일 수 있다.
        assert!(
            spec.args
                .ends_with(&["--".to_string(), "bash".to_string(), "-l".to_string()]),
            "{:?}",
            spec.args
        );
    }
}
