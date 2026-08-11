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
        // `--` 가 아니라 `--exec` 인 이유 (실기 버그 2026-08-12): `--` 는 명령을 WSL
        // **기본 셸을 한 번 거쳐** 실행해 래퍼 스크립트가 셸 평가를 두 번 받는다.
        // $HOME/$PATH 같은 환경 변수는 바깥 평가에서도 같은 값이라 티가 안 났지만,
        // 스크립트 안에서 정의하는 변수($RESUME/$cmd)는 바깥 평가가 빈 값으로
        // 선확장해 resume 블록이 통째로 스킵됐다. --exec 는 argv 를 셸 경유 없이
        // 그대로 실행한다 (평가 1회) — 기본 셸이 zsh/fish 인 배포판에서 스크립트가
        // 다른 문법으로 파싱되는 잠재 버그 계열도 함께 제거된다.
        args.push("--exec".to_string());
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

/// `wsl.exe ... --exec` 뒤에 붙는 **WSL 안 셸 argv** (셸 경유 없는 직접 실행 — spawn_spec 주석).
///
/// 모든 경로가 `WINMUX=1` 을 물고 로그인 셸을 exec 한다 — 탭 안에서 도는 에이전트·
/// 스크립트가 "여기가 winmux 다"를 스스로 알아야 send 채널·알림 계약을 쓸지 판단할
/// 수 있기 때문이다 (`winmux-send` 스킬의 자가 인지 조건). `history_tab` 이 Some
/// 이면 `WINMUX_TAB` 으로 자기 탭 id 까지 알려 주고, 그 탭 전용 HISTFILE 을 물린다
/// — 탭의 안정 ID 는 재시작을 넘어 유지되므로 재시작 후에도 같은 탭의 history 만
/// 복원된다 (체크포인트 2 UX 요청). 셸 안에서 `mkdir -p` 로 디렉터리를 만드는 이유
/// 는 Windows 쪽에서 WSL 파일시스템 경로를 추측하지 않기 위해서이고, `$HOME` 은
/// 공백이 섞여도 안전하도록 따옴표로 감싼다. `VAR=... exec bash -l` 의 할당은
/// exec 되는 프로세스의 환경으로 전달되며(= 그 셸의 자식들에게도 상속된다),
/// **Ubuntu 기본 `.bashrc`/`.profile` 은 HISTSIZE 류만 손대고 HISTFILE 은 덮지
/// 않으므로** 이 env 상속만으로 충분하다. mkdir 이 실패하면 `&&` 가 끊겨 셸이
/// 그대로 종료된다 — 탭이 exited 로 표시돼 실패가 드러난다 (조용한 fallback 없음).
///
/// **에이전트 세션 resume 힌트**: 재시작 후 respawn 되는 셸은 새 셸이라 그 탭에서 돌던
/// 에이전트 세션이 화면에서 사라진다. Claude Code hook 이 매 호출마다
/// `~/.winmux/resume/tab-<id>` 에 1행 = resume 명령, 2행 = 기록 시각(epoch)을 남기므로
/// (`provision.rs` 의 `winmux-notify.sh` — 계약은 `scripts/wsl/claude-hook-example.md`),
/// exec 직전에 그 1행을 읽어 ① 탭의 HISTFILE 끝에 덧붙이고(↑ 한 번에 나온다)
/// ② 흐린 안내 한 줄을 찍는다. **자동 실행은 하지 않는다** — 재개할지는 사용자가 정한다.
/// 파일이 없으면 아무 출력도 없다. 기록 시각으로 신선도를 판단하지 않는 것은 의도된
/// 단순화다 (오래된 힌트인지는 사용자가 안다). 이 블록은 `&&` 사슬 안에 있지만 파일
/// 부재·읽기 실패·append 실패 어느 쪽으로도 0 으로 끝난다(조건 거짓인 `if` 는 0,
/// `read` 실패는 `|| true` 가 흡수, append 실패는 뒤따르는 printf 가 가린다) — 힌트가
/// 셸 자체를 못 띄우게 만드는 일은 없어야 한다. 유일하게 새는 status 는 **안내 printf
/// 자체가 이 pane 의 tty 에 못 쓰는** 경우인데, 그건 애초에 쓸 수 있는 탭이 아니다.
/// `RESUME`·`cmd` 는 여기서 처음 대입되는 평범한 셸 변수라 exec 로 프로세스 이미지가
/// 갈릴 때 사라진다 (로그인 셸 환경 무오염). 엄밀히는 같은 이름이 **export 된 채로
/// 상속돼 들어오면** bash 가 export 속성을 유지하지만, spawn 환경은 winmux 가 이
/// 함수에서 통째로 정하므로 그런 값은 오지 않는다.
///
/// 힌트 1행은 **읽는 쪽에서도 형태를 검증한다** (리뷰 finding): 기록 형식과 대칭인
/// `claude --resume <영숫자·-·_ 토큰>` 정확 일치만 통과시키고, 그 외(escape 시퀀스·
/// 셸 메타문자·다른 명령)는 조용히 무시한다 — 같은 uid 가 파일을 바꿔치기해도
/// "↑+Enter 를 유도하는 임의 명령 표면"이 되지 않는다. 같은 uid 는 어차피
/// `~/.bashrc` 를 고칠 수 있으니 권한 경계가 아니라 오발 방지이며, 쓰는 쪽
/// (`winmux-notify.sh`)의 session_id charset 가드와 짝이다. 또 힌트는 표시로
/// 소비되지 않으므로, 세션이 바뀌지 않은 채 재시작을 N 번 하면 같은 줄이
/// HISTFILE 에 N 개 쌓인다 (인접하므로 ↑ 한 번은 그대로).
///
/// `PATH` 프리펜드는 프로비저닝이 까는 `winmux` CLI(`~/.winmux/bin/winmux`)를 탭 안에서
/// 경로 없이 부르기 위한 것이다 (`provision.rs` 2절). 로그인 셸이 이 값을 덮지 않는
/// 이유는 Debian/Ubuntu 계열의 `/etc/profile`·`~/.profile` 관례가 PATH 를 **재대입이
/// 아니라 `PATH="...:$PATH"` 로 프리펜드**하기 때문이다 — 우리 항목은 앞에서 밀릴 뿐
/// 사라지지 않는다. 디렉터리가 아직 없어도 무해하다 (PATH 의 없는 항목은 그냥 건너뛴다).
///
/// **ConPTY 색 테이블 동기화 (OSC 10/11 set)**: exec 전에 우리 테마의 fg/bg 를 tty 로
/// 한 번 내보낸다. WT 1.22+ 의 conhost 는 TUI 앱의 OSC 10/11 **질의**를 자기가 가로채
/// 자기 색 테이블(기본 ≈검정)로 대신 응답하므로(ms/terminal#17729), Codex 처럼 배경색을
/// 질의해 입력창 배경을 만드는 앱이 검정 기준으로 색을 골라 우리 배경(#1e1e1e)과 겹친다
/// — 실기 스크린샷의 "입력칸 구분 없음"의 실원인. set 은 conhost 가 handled 로 소비해
/// 자기 테이블을 갱신하므로 이후 질의가 올바른 값으로 답한다 (xterm 까지는 오지 않아
/// 프론트 무영향). **값의 정본은 `apps/winmux/src/terminal-view.ts` 의 `TERMINAL_THEME`
/// foreground/background 다 — 테마를 바꾸면 여기도 같이 바꾼다** (갈라지면 재발).
/// `COLORTERM=truecolor` 는 그 짝: Codex 의 색 선택이 truecolor 분기를 타게 한다.
///
/// **3자 동기화 계약**: 같은 값을 쓰는 세 번째 자리가 `sink.rs` 의
/// `COLOR_REPLY_FOREGROUND`/`COLOR_REPLY_BACKGROUND` 다 — OSC 10/11 **질의**에 앱이
/// 직접 답하는 응답기로, 위 "conhost 가 대신 응답한다"는 전제가 실기 probe 에서
/// 뒤집힌(아무도 응답하지 않았다) 뒤 2026-08-11 에 추가됐다. 셋 중 하나를 바꾸면
/// 나머지 둘도 같이 바꾼다.
#[cfg(windows)]
fn bash_argv(history_tab: Option<u64>) -> Vec<String> {
    // 이 파일이 없으면 `winmux` 를 못 찾을 뿐이고, 프로비저닝이 다음 부팅에서 다시
    // 시도한다 — 스폰 핫패스에서 존재를 확인하지 않는다 (wsl.exe 왕복 금지).
    const PATH_PREFIX: &str = "PATH=\"$HOME/.winmux/bin:$PATH\"";
    // TERMINAL_THEME 동기화 계약 (rustdoc 참조): fg #cccccc, bg #1e1e1e.
    const THEME_SYNC: &str = r"printf '\033]10;#cccccc\033\\\033]11;#1e1e1e\033\\'";
    let script = match history_tab {
        Some(tab) => format!(
            "{THEME_SYNC}; mkdir -p \"$HOME/.winmux/history\" \
             && RESUME=\"$HOME/.winmux/resume/tab-{tab}\" && cmd= \
             && if [ -s \"$RESUME\" ]; then IFS= read -r cmd < \"$RESUME\" || true; fi \
             && case \"$cmd\" in 'claude --resume '*) \
             expr \"x$cmd\" : 'xclaude --resume [A-Za-z0-9_-][A-Za-z0-9_-]*$' >/dev/null || cmd= ;; \
             *) cmd= ;; esac \
             && if [ -n \"$cmd\" ]; then \
             printf '%s\\n' \"$cmd\" >> \"$HOME/.winmux/history/tab-{tab}\"; \
             printf '\\033[2m[winmux] resume previous agent: %s\\033[0m\\n' \"$cmd\"; fi \
             && {PATH_PREFIX} COLORTERM=truecolor WINMUX=1 WINMUX_TAB={tab} \
             HISTFILE=\"$HOME/.winmux/history/tab-{tab}\" exec bash -l"
        ),
        // 탭 id 를 모르는 경로(히스토리 미할당)에서는 WINMUX 만 — 없는 id 를 지어내지 않는다.
        None => format!("{THEME_SYNC}; {PATH_PREFIX} COLORTERM=truecolor WINMUX=1 exec bash -l"),
    };
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
    fn history_tab_wraps_bash_with_winmux_env_and_per_tab_histfile() {
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
                "--exec".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                "printf '\\033]10;#cccccc\\033\\\\\\033]11;#1e1e1e\\033\\\\'; \
                 mkdir -p \"$HOME/.winmux/history\" \
                 && RESUME=\"$HOME/.winmux/resume/tab-7\" && cmd= \
                 && if [ -s \"$RESUME\" ]; then IFS= read -r cmd < \"$RESUME\" || true; fi \
                 && case \"$cmd\" in 'claude --resume '*) \
                 expr \"x$cmd\" : 'xclaude --resume [A-Za-z0-9_-][A-Za-z0-9_-]*$' >/dev/null || cmd= ;; \
                 *) cmd= ;; esac \
                 && if [ -n \"$cmd\" ]; then \
                 printf '%s\\n' \"$cmd\" >> \"$HOME/.winmux/history/tab-7\"; \
                 printf '\\033[2m[winmux] resume previous agent: %s\\033[0m\\n' \"$cmd\"; fi \
                 && PATH=\"$HOME/.winmux/bin:$PATH\" COLORTERM=truecolor WINMUX=1 WINMUX_TAB=7 \
                 HISTFILE=\"$HOME/.winmux/history/tab-7\" exec bash -l"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn without_history_tab_the_shell_still_gets_winmux_but_no_tab_id() {
        let spec = spawn_spec(&ShellSpawnReq::default());
        // 탭 id 가 없으면 WINMUX 만 물린 로그인 셸 (셸 기본 history 그대로).
        // PATH 프리펜드는 두 경로에 다 걸린다 — winmux CLI 는 탭 id 와 무관하다.
        // resume 힌트도 없다 — 힌트 파일은 탭 id 로 주소가 정해지므로 id 가 없으면
        // 읽을 파일 자체가 없다 (없는 id 를 지어내지 않는 규율의 연장).
        // 앞부분은 검사하지 않는다 — env WINMUX_DISTRO 가 `-d` 를 덧붙일 수 있다.
        assert!(
            spec.args.ends_with(&[
                "--exec".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                "printf '\\033]10;#cccccc\\033\\\\\\033]11;#1e1e1e\\033\\\\'; \
                 PATH=\"$HOME/.winmux/bin:$PATH\" COLORTERM=truecolor WINMUX=1 exec bash -l"
                    .to_string(),
            ]),
            "{:?}",
            spec.args
        );
        assert!(
            !spec.args.iter().any(|a| a.contains("resume")),
            "{:?}",
            spec.args
        );
    }
}
