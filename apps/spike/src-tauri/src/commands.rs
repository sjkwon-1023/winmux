//! Tauri 커맨드 — 프론트엔드 ↔ wmux-core 글루 (spike-plan 4.5).
//!
//! 잠금 규율: 레지스트리 뮤텍스 아래에서는 `Arc<PtySession>` 핸들 복사만 한다.
//! 블로킹 가능성이 있는 호출(PTY write·resize·spawn)은 락을 놓은 뒤, 그리고
//! 메인(이벤트 루프) 스레드가 아닌 `spawn_blocking` 스레드에서 수행한다 —
//! paused 상태에서 자식이 stdin을 읽지 않아 write가 블록되면, 메인 스레드가
//! 잡혀 있는 한 `ack_output`도 처리되지 못해 영구 교착이 되기 때문이다.

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, State};
use wmux_core::session::{PtySession, SessionOptions, SpawnSpec};

use crate::sink::ChannelSink;
use crate::state::AppState;

/// spike 기본 버퍼 설정 (spike-plan 4.5): replay 1MB, high water 2MB, low water 512KB.
const REPLAY_CAP: usize = 1024 * 1024;
const HIGH_WATER: usize = 2 * 1024 * 1024;
const LOW_WATER: usize = 512 * 1024;

/// `get_stats` 직렬화형. wmux-core의 `SessionStats`는 serde 의존이 없으므로
/// 글루에서 DTO로 옮겨 내보낸다. `id`는 코어 내부 값 대신 레지스트리 라우팅 id를
/// 강제해 이벤트·커맨드의 id와 항상 일치시킨다.
#[derive(serde::Serialize)]
pub struct SessionStatsDto {
    pub id: u32,
    pub bytes_out: u64,
    pub pending: usize,
    pub paused: bool,
    pub osc_count: u64,
    pub last_osc: Option<String>,
    pub alive: bool,
}

/// 플랫폼별 기본 spawn 명령 (spike-plan 4.5).
/// - Windows: `wsl.exe [-d $WMUX_DISTRO] -- bash -l` (WMUX_DISTRO 미설정이면 기본 배포판).
/// - Unix(개발 실행): `$SHELL -l`, `$SHELL` 없으면 `bash -l`.
fn default_spawn_spec(cols: u16, rows: u16) -> SpawnSpec {
    #[cfg(windows)]
    {
        let mut args: Vec<String> = Vec::new();
        // 빈 문자열은 미설정으로 취급한다.
        if let Ok(distro) = std::env::var("WMUX_DISTRO") {
            if !distro.is_empty() {
                args.push("-d".to_string());
                args.push(distro);
            }
        }
        args.extend(["--", "bash", "-l"].map(String::from));
        SpawnSpec {
            program: "wsl.exe".to_string(),
            args,
            cwd: None,
            cols,
            rows,
        }
    }
    #[cfg(not(windows))]
    {
        let program = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "bash".to_string());
        SpawnSpec {
            program,
            args: vec!["-l".to_string()],
            cwd: None,
            cols,
            rows,
        }
    }
}

/// id로 세션 핸들을 얻는다. 레지스트리 락은 이 함수 안에서만 잡힌다 —
/// 반환된 핸들에 대한 호출은 전부 락 밖에서 이뤄진다.
fn session(state: &AppState, id: u32) -> Result<Arc<PtySession>, String> {
    state
        .registry()
        .get(id)
        .ok_or_else(|| format!("unknown terminal id: {id}"))
}

#[tauri::command]
pub async fn create_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    cols: u16,
    rows: u16,
    on_output: Channel<InvokeResponseBody>,
) -> Result<u32, String> {
    // sink가 osc/exit 이벤트에 id를 실어야 하므로 spawn 전에 id를 발급한다.
    // spawn(프로세스 생성 — 수십 ms 이상)은 blocking 스레드에서 수행한다.
    let id = state.registry().allocate_id();
    let sink = ChannelSink::new(id, on_output, app);
    let opts = SessionOptions {
        replay_cap: REPLAY_CAP,
        high_water: HIGH_WATER,
        low_water: LOW_WATER,
    };
    let spec = default_spawn_spec(cols, rows);
    let session =
        tauri::async_runtime::spawn_blocking(move || PtySession::spawn(spec, Box::new(sink), opts))
            .await
            .map_err(|err| format!("spawn task join failed: {err}"))?
            .map_err(|err| format!("failed to spawn terminal: {err:#}"))?;
    state.registry().insert(id, session);
    Ok(id)
}

#[tauri::command]
pub async fn write_stdin(state: State<'_, AppState>, id: u32, data: String) -> Result<(), String> {
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.write(data.as_bytes()))
        .await
        .map_err(|err| format!("write task join failed (id={id}): {err}"))?
        .map_err(|err| format!("write_stdin failed (id={id}): {err:#}"))
}

#[tauri::command]
pub async fn send_raw(state: State<'_, AppState>, id: u32, bytes: Vec<u8>) -> Result<(), String> {
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.write(&bytes))
        .await
        .map_err(|err| format!("write task join failed (id={id}): {err}"))?
        .map_err(|err| format!("send_raw failed (id={id}): {err:#}"))
}

#[tauri::command]
pub async fn resize(
    state: State<'_, AppState>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    // ConPTY resize는 conhost 채널을 경유하므로 이론상 블록 가능 — write와 같은 경로로 내린다.
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.resize(cols, rows))
        .await
        .map_err(|err| format!("resize task join failed (id={id}): {err}"))?
        .map_err(|err| format!("resize failed (id={id}): {err:#}"))
}

#[tauri::command]
pub fn ack_output(state: State<'_, AppState>, id: u32, n: usize) -> Result<(), String> {
    // ack는 뮤텍스 갱신 + condvar notify뿐이라 블로킹하지 않는다 — sync로 즉시 처리해
    // paused 세션이 최대한 빨리 재개되게 한다.
    let session = session(&state, id)?;
    session.ack(n);
    Ok(())
}

/// replay 스냅샷도 터미널 출력이므로 JSON 배열이 아니라 raw body로 되돌린다 —
/// 프론트엔드 `invoke`는 ArrayBuffer를 받는다.
#[tauri::command]
pub fn replay(state: State<'_, AppState>, id: u32) -> Result<Response, String> {
    let session = session(&state, id)?;
    Ok(Response::new(session.replay()))
}

#[tauri::command]
pub fn close_terminal(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    let session = state
        .registry()
        .remove(id)
        .ok_or_else(|| format!("unknown terminal id: {id}"))?;
    // kill은 레지스트리 락 밖에서 — 자식 kill이 먼저 나가므로, 블록 중이던
    // write가 있어도 자식 종료(EPIPE)로 풀린다. 이후 정리는 코어 책임.
    session.kill();
    Ok(())
}

#[tauri::command]
pub fn get_stats(state: State<'_, AppState>) -> Vec<SessionStatsDto> {
    // 핸들 스냅샷만 락 아래에서 뜨고, 세션별 stats 잠금은 락 밖에서 잡는다.
    let handles = state.registry().handles();
    handles
        .into_iter()
        .map(|(id, session)| {
            let stats = session.stats();
            SessionStatsDto {
                id,
                bytes_out: stats.bytes_out,
                pending: stats.pending,
                paused: stats.paused,
                osc_count: stats.osc_count,
                last_osc: stats.last_osc,
                alive: stats.alive,
            }
        })
        .collect()
}
