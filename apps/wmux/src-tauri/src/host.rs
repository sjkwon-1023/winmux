//! `SessionHost` 구현 — Dispatcher 의 PTY 부수효과 포트를 `SessionManager` 위에
//! 실현한다. `dispatch` 가 Dispatcher lock 아래에서 호출하므로(스폰 포함 — 계획
//! 0-3 수용) 여기서 Dispatcher lock 을 다시 잡는 코드는 금지다.

use std::cell::Cell;
use std::sync::Arc;

use tauri::AppHandle;
use wmux_core::command::{SessionHost, ShellSpawnReq};
use wmux_core::session::{SessionId, SessionManager, SessionOptions, SpawnSpec};

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
        // distro 우선순위: req.distro → env WMUX_DISTRO → 없음(WSL 기본 배포판).
        // 빈 문자열은 미설정으로 취급한다.
        let distro = req
            .distro
            .clone()
            .filter(|d| !d.is_empty())
            .or_else(|| std::env::var("WMUX_DISTRO").ok().filter(|d| !d.is_empty()));
        if let Some(distro) = distro {
            args.push("-d".to_string());
            args.push(distro);
        }
        args.extend(["--", "bash", "-l"].map(String::from));
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
