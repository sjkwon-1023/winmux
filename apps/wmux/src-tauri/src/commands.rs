//! Tauri 커맨드 — 프론트엔드 ↔ wmux-core 글루.
//!
//! 두 갈래로 나뉜다 (10단계 계획 0-3 잠금 배치):
//!
//! - **구조 변이** (`dispatch`, `get_state`): `Mutex<Dispatcher>` 를 잡는다.
//!   dispatch 는 내부 스폰이 블로킹이라 전체를 `spawn_blocking` 에서 돈다.
//! - **핫패스** (`attach_terminal`/`write_stdin`/`send_raw`/`resize`/`ack_output`/
//!   `get_stats`): Dispatcher lock 을 절대 타지 않는다 — `SessionManager`·sink
//!   레지스트리의 짧은 내부 lock 만 스친다. write·resize 는 블로킹 가능성이
//!   있어 `spawn_blocking`, ack 은 뮤텍스 갱신 + condvar notify 뿐이라 sync 즉시
//!   처리한다 (paused 재개 최단 경로 — spike 와 동일 규율).

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, State};
use wmux_core::command::{Command, CommandError, CommandOutput};
use wmux_core::session::{PtySession, SessionId};

use crate::state::{emit_state_changed, AppState};

/// `get_stats` 직렬화형 (spike 이식). 코어 `SessionStats` 는 serde 의존이 없어
/// 글루 DTO 로 내보낸다. `id` 는 레지스트리 발급 `SessionId` — 커맨드·이벤트의
/// id 와 같은 공간이다.
#[derive(serde::Serialize)]
pub struct SessionStatsDto {
    pub id: SessionId,
    pub bytes_out: u64,
    pub pending: usize,
    pub paused: bool,
    pub osc_count: u64,
    pub last_osc: Option<String>,
    pub alive: bool,
}

/// id 로 세션 핸들을 얻는다. 매니저 내부 lock 은 이 조회 안에서만 잡힌다 —
/// 반환된 핸들에 대한 호출은 전부 lock 밖에서 이뤄진다.
fn session(state: &AppState, id: SessionId) -> Result<Arc<PtySession>, String> {
    state
        .sessions
        .get(id)
        .ok_or_else(|| format!("unknown session id: {id}"))
}

/// 구조 변이 단일 진입점 — 커맨드 bus. 성공 시 `state-changed` 로 새 스냅샷을
/// emit 하고 `CommandOutput` 을 돌려준다 (dev 훅·MCP 가 생성 id 를 후속 조작에
/// 쓴다). 실패(`CommandError`)는 상태 불변이 보장되므로 emit 하지 않는다.
#[tauri::command]
pub async fn dispatch(
    app: AppHandle,
    state: State<'_, AppState>,
    cmd: Command,
) -> Result<CommandOutput, CommandError> {
    // 전체를 spawn_blocking 에서: CreateTab 의 셸 스폰(프로세스 생성 — 수십 ms
    // 블로킹)이 Dispatcher lock 아래에서 일어난다 (계획 0-3 — 핫패스와 무간섭
    // 이라 수용). 메인(이벤트 루프) 스레드는 잡지 않는다.
    let dispatcher = Arc::clone(&state.dispatcher);
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = dispatcher.lock().unwrap();
        let out = d.dispatch(cmd)?;
        // emit 은 lock 안에서 — revision 과 상태가 일관된 스냅샷만 나간다.
        emit_state_changed(&app, &d);
        Ok(out)
    })
    .await
    // join 실패 = 위 클로저의 패닉(락 poison 등 프로그램 결함) — 가려서 ok 로
    // 만들지 않고 그대로 크게 터뜨린다.
    .expect("dispatch task panicked")
}

/// 현재 상태 스냅샷 (`{ revision, state }`) — 부팅·재동기화용.
/// async 인 이유: dispatch(spawn_blocking)가 스폰 수십 ms 동안 Dispatcher lock 을
/// 쥘 수 있는데, sync 커맨드로 메인 스레드에서 그 lock 을 기다리면 뒤에 줄 선
/// sync 핫패스(ack_output)까지 지연이 전파된다 (리뷰 finding).
#[tauri::command]
pub async fn get_state(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let dispatcher = Arc::clone(&state.dispatcher);
    tauri::async_runtime::spawn_blocking(move || {
        let d = dispatcher.lock().unwrap();
        // 순수 데이터 직렬화라 실패는 프로그램 결함뿐 — 가리지 않고 패닉.
        serde_json::to_value(d.snapshot()).expect("state snapshot must serialize")
    })
    .await
    .map_err(|err| format!("get_state task join failed: {err}"))
}

/// 터미널 출력 스트림 접속(재접속). raw body `[u64 LE end_offset][replay bytes]`
/// 를 돌려주고, 이후 출력은 `on_output` 채널로 `[u64 LE offset][bytes]` 프레임이
/// 흐른다. Dispatcher lock 불필요 (핫패스).
///
/// **순서 불변식**: 채널을 sink 슬롯에 먼저 장착하고 그 다음 `reattach()` —
/// 순서를 바꾸면 그 사이 출력이 스냅샷에도 채널에도 없는 유실 창이 생긴다
/// (`PtySession::reattach` rustdoc). 겹침은 프론트가 `offset < end_offset` 폐기로
/// dedup 하되 폐기분 포함 전량 ack 한다.
#[tauri::command]
pub fn attach_terminal(
    state: State<'_, AppState>,
    session: SessionId,
    on_output: Channel<InvokeResponseBody>,
) -> Result<Response, String> {
    let sink = state
        .sinks
        .get(session)
        .ok_or_else(|| format!("unknown session id: {session}"))?;
    let handle = self::session(&state, session)?;
    // 1) 채널 먼저 장착 —
    sink.attach(on_output);
    // 2) — 그 다음 reattach (flow 리셋 + 일관 스냅샷).
    let (end_offset, replay) = handle.reattach();
    let mut body = Vec::with_capacity(8 + replay.len());
    body.extend_from_slice(&end_offset.to_le_bytes());
    body.extend_from_slice(&replay);
    Ok(Response::new(body))
}

/// 출력 채널 분리 — 뷰 dispose(탭 전환 등) 시 호출된다. 이후 출력은 Dropped
/// (detach 모드)로 보상 롤백돼 백그라운드 세션이 paused 에 고착되지 않는다
/// (`TerminalSink::detach` rustdoc). 미지 id 는 무해한 no-op (이미 닫힌 세션의
/// 늦은 dispose 가 정상 순서로 도착할 수 있다).
#[tauri::command]
pub fn detach_terminal(state: State<'_, AppState>, session: SessionId) {
    if let Some(sink) = state.sinks.get(session) {
        sink.detach();
    }
}

#[tauri::command]
pub async fn write_stdin(
    state: State<'_, AppState>,
    id: SessionId,
    data: String,
) -> Result<(), String> {
    // paused 상태에서 자식이 stdin 을 읽지 않으면 write 가 블록될 수 있다 —
    // 메인 스레드가 잡히면 ack_output 도 못 돌아 영구 교착이므로 blocking 풀로.
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.write(data.as_bytes()))
        .await
        .map_err(|err| format!("write task join failed (id={id}): {err}"))?
        .map_err(|err| format!("write_stdin failed (id={id}): {err:#}"))
}

#[tauri::command]
pub async fn send_raw(
    state: State<'_, AppState>,
    id: SessionId,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.write(&bytes))
        .await
        .map_err(|err| format!("write task join failed (id={id}): {err}"))?
        .map_err(|err| format!("send_raw failed (id={id}): {err:#}"))
}

#[tauri::command]
pub async fn resize(
    state: State<'_, AppState>,
    id: SessionId,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    // ConPTY resize 는 conhost 채널 경유라 이론상 블록 가능 — write 와 같은 경로.
    let session = session(&state, id)?;
    tauri::async_runtime::spawn_blocking(move || session.resize(cols, rows))
        .await
        .map_err(|err| format!("resize task join failed (id={id}): {err}"))?
        .map_err(|err| format!("resize failed (id={id}): {err:#}"))
}

#[tauri::command]
pub fn ack_output(state: State<'_, AppState>, id: SessionId, n: usize) -> Result<(), String> {
    // ack 는 뮤텍스 갱신 + condvar notify 뿐 — sync 즉시 처리해 paused 세션이
    // 최대한 빨리 재개되게 한다.
    let session = session(&state, id)?;
    session.ack(n);
    Ok(())
}

#[tauri::command]
pub fn get_stats(state: State<'_, AppState>) -> Vec<SessionStatsDto> {
    // 코어 `stats()` 가 레지스트리 lock 을 놓은 채 세션별 stats 를 뜨고 id
    // 오름차순 (id, stats) 쌍을 돌려준다.
    state
        .sessions
        .stats()
        .into_iter()
        .map(|(id, stats)| SessionStatsDto {
            id,
            bytes_out: stats.bytes_out,
            pending: stats.pending,
            paused: stats.paused,
            osc_count: stats.osc_count,
            last_osc: stats.last_osc,
            alive: stats.alive,
        })
        .collect()
}
