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

use crate::state::{publish_state, AppState};

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
/// emit + 저장 예약하고(`publish_state`) `CommandOutput` 을 돌려준다 (dev 훅·
/// MCP 가 생성 id 를 후속 조작에 쓴다). 실패(`CommandError`)는 상태 불변이
/// 보장되므로 emit 도 저장도 하지 않는다.
#[tauri::command]
pub async fn dispatch(
    app: AppHandle,
    state: State<'_, AppState>,
    cmd: Command,
) -> Result<CommandOutput, CommandError> {
    // 활동 신호용 판별 — cmd 는 아래 클로저로 move 되므로 먼저 본다.
    let is_workspace_switch = matches!(cmd, Command::SwitchWorkspace { .. });
    // 전체를 spawn_blocking 에서: CreateTab 의 셸 스폰(프로세스 생성 — 수십 ms
    // 블로킹)이 Dispatcher lock 아래에서 일어난다 (계획 0-3 — 핫패스와 무간섭
    // 이라 수용). 메인(이벤트 루프) 스레드는 잡지 않는다.
    let dispatcher = Arc::clone(&state.dispatcher);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut d = dispatcher.lock().unwrap();
        let out = d.dispatch(cmd)?;
        // emit + 저장 예약은 lock 안에서 — revision 과 상태가 일관된 스냅샷만
        // 나가고, 같은 상태가 디스크 저장으로도 예약된다.
        publish_state(&app, &d);
        Ok(out)
    })
    .await
    // join 실패 = 위 클로저의 패닉(락 poison 등 프로그램 결함) — 가려서 ok 로
    // 만들지 않고 그대로 크게 터뜨린다.
    .expect("dispatch task panicked");
    // 성공한 dispatch 는 실제 사용자 활동이다 (UI·dev 훅 발) — 계획 16단계 C-2.
    // SwitchWorkspace 성공은 추가로 pending 워치독의 "안전한 순간" 신호.
    if result.is_ok() {
        state.reset.user_input();
        if is_workspace_switch {
            state.reset.workspace_switch();
        }
    }
    result
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

/// 출력 채널 분리 — 뷰 dispose 시, 그리고 부트 리컨실 스윕(attach 하지 않는 전
/// 터미널 세션 대상 — 프론트 배선은 12단계 청크 C)에서 호출된다. 채널 분리 후 이후
/// 출력은 Dropped(detach 모드)로 보상 롤백된다 (`TerminalSink::detach` rustdoc).
/// 이어서 `reset_flow()` 로 flow 계정까지 리셋한다 (계획 D4 자동 치유) — 이미
/// paused 인 세션은 리더가 read 를 안 해 Dropped 롤백 경로 자체가 실행되지
/// 않으므로, detach 시점에 리셋해야 detach 된 세션이 어떤 경로로든 paused 에
/// 고착되지 않는다. 미지 id 는 무해한 no-op (이미 닫힌 세션의 늦은 dispose 가
/// 정상 순서로 도착할 수 있고, 스윕은 멱등해야 한다).
#[tauri::command]
pub fn detach_terminal(state: State<'_, AppState>, session: SessionId) {
    if let Some(sink) = state.sinks.get(session) {
        sink.detach();
    }
    if let Some(handle) = state.sessions.get(session) {
        handle.reset_flow();
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
    // 주의: stdin 기록은 활동 신호로 치지 **않는다** (16단계 리뷰 finding).
    // xterm 의 onData 는 사용자 타이핑뿐 아니라 단말 질의(DA·DSR·OSC 색상 질의)에
    // 대한 **자동 응답**에도 발화하고, 그 질의는 replay 에 보존돼 리셋 후 재생된다
    // — 여기서 활동으로 집계하면 리셋 → replay → 자동 응답 → idle 재무장의
    // 자기루프가 된다. 실제 타이핑은 프론트 활동 핑(window capture keydown)이
    // 이미 잡으므로 유실도 없다.
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
    // write_stdin 과 동일하게 활동 신호로 치지 않는다 (자동 응답 자기루프 —
    // write_stdin 의 주석 참조).
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

/// 프론트 활동 핑 (계획 16단계 C-2/C-3) — throttled 사용자 입력 신호
/// (wheel/mousedown/keydown, 10초당 1회) + `document.visibilitychange` 보조 신호.
/// `visible` 이 Some 이면 visibility 전이도 함께 반영한다. 순수 열람(스크롤백
/// wheel)도 여기로 잡혀 "활성 사용 중 절대 리셋 금지"가 성립한다 (계획 0장).
#[tauri::command]
pub fn user_activity(state: State<'_, AppState>, visible: Option<bool>) {
    match visible {
        // visibility 전이 보고는 **활동이 아니다** (체크포인트 1 버그 4·5).
        // 활동으로 집계하면: 최소화 보고(visible=false)가 hidden 카운트다운을
        // 스스로 재무장해 hidden 리셋이 영원히 발화하지 못하고, 리로드 직후의
        // visible=true 동기화는 idle 을 재무장해 30초 주기 재발화 루프가 된다.
        // 최소화 클릭 같은 실제 제스처는 그 직전의 mousedown 핑이 이미 잡는다.
        Some(visible) => state.reset.visibility(visible),
        None => state.reset.user_input(),
    }
}

/// 수동 WebView 리셋 — **dev 훅(`window.__wmux.resetUi`)·향후 MCP 전용이며 UI
/// 버튼으로 노출하지 않는다** (계획 v2 12장 원칙). 코어 Command bus 는 구조 변이
/// 전용(ADR-0002)이고 리셋은 상태 무변이·Tauri 의존 동작이라 글루 커맨드로 둔다
/// (계획 0장의 의도적 이탈 — ADR 증류 시 기록).
#[tauri::command]
pub fn reset_ui(state: State<'_, AppState>) {
    state.reset.reset_now();
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
