//! Tauri 커맨드 — 프론트엔드 ↔ winmux-core 글루.
//!
//! 세 갈래로 나뉜다 (10단계 계획 0-3 잠금 배치 + 21단계 뷰어):
//!
//! - **구조 변이** (`dispatch`, `get_state`): `Mutex<Dispatcher>` 를 잡는다.
//!   dispatch 는 내부 스폰이 블로킹이라 전체를 `spawn_blocking` 에서 돈다.
//! - **핫패스** (`attach_terminal`/`write_stdin`/`send_raw`/`resize`/`ack_output`/
//!   `get_stats`): Dispatcher lock 을 절대 타지 않는다 — `SessionManager`·sink
//!   레지스트리의 짧은 내부 lock 만 스친다. write·resize 는 블로킹 가능성이
//!   있어 `spawn_blocking`, ack 은 뮤텍스 갱신 + condvar notify 뿐이라 sync 즉시
//!   처리한다 (paused 재개 최단 경로 — spike 와 동일 규율).
//! - **뷰어 파일 접근** (`fs_list_dir`/`fs_stat`/`fs_read_chunk` — 21단계): 상태를
//!   건드리지 않는 읽기 전용 콘텐츠 플레인이라 Dispatcher lock 도 관리 상태도
//!   타지 않는다. 9P(`\\wsl.localhost`) I/O 와 distro 질의(프로세스 스폰)가 전부
//!   블로킹이라 **경로 해석까지 통째로** `spawn_blocking` 안에서 돈다.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, State};
use winmux_core::command::{Command, CommandError, CommandOutput};
use winmux_core::session::{PtySession, SessionId};
use winmux_core::wslpath;

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
    // 새 워크스페이스의 distro 도 먼저 복사해 둔다 (성공 후 프로비저닝 대상).
    // 부팅 때 없던 distro 가 이 경로로만 들어오므로 여기가 두 번째 호출 지점이다.
    let created_distro = match &cmd {
        Command::CreateWorkspace { distro, .. } => Some(distro.clone()),
        _ => None,
    };
    let provision_app = app.clone();
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
        state.reset.user_input("dispatch");
        if is_workspace_switch {
            state.reset.workspace_switch();
        }
        // 워크스페이스가 실제로 생겼을 때만 — 이미 프로비저닝한 distro(부팅 때
        // 건 것 포함)는 `ensure_provisioned` 의 프로세스 수명 캐시가 걸러 낸다.
        if let Some(distro) = created_distro {
            crate::provision::ensure_provisioned(&provision_app, distro.as_deref());
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

/// 터미널 출력 스트림 접속(재접속). raw body
/// `[u64 LE end_offset][u8 first_attach][replay bytes]` 를 돌려주고, 이후 출력은
/// `on_output` 채널로 `[u64 LE offset][bytes]` 프레임이 흐른다. Dispatcher lock
/// 불필요 (핫패스).
///
/// `first_attach` (1 = 이 세션의 최초 attach): 프론트가 replay 속 단말 질의에
/// 대한 xterm 자동 응답을 허용할지 판정한다 — 최초 attach 의 질의는 아직 응답이
/// 안 간 라이브 질의(억제 시 conhost 가 CPR 대기로 셸이 멈춤), 재-attach 의
/// 질의는 이미 응답된 낡은 질의(재응답 시 stray `R`)다. `TerminalSink::mark_attached`
/// rustdoc 참조.
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
    let first_attach = !sink.mark_attached();
    // 1) 채널 먼저 장착 —
    sink.attach(on_output);
    // 2) — 그 다음 reattach (flow 리셋 + 일관 스냅샷).
    let (end_offset, replay) = handle.reattach();
    let mut body = Vec::with_capacity(9 + replay.len());
    body.extend_from_slice(&end_offset.to_le_bytes());
    body.push(u8::from(first_attach));
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
        None => state.reset.user_input("ping"),
    }
}

/// 수동 WebView 리셋 — **dev 훅(`window.__winmux.resetUi`)·향후 MCP 전용이며 UI
/// 버튼으로 노출하지 않는다** (계획 v2 12장 원칙). 코어 Command bus 는 구조 변이
/// 전용(ADR-0002)이고 리셋은 상태 무변이·Tauri 의존 동작이라 글루 커맨드로 둔다
/// (계획 0장의 의도적 이탈 — ADR 증류 시 기록).
#[tauri::command]
pub fn reset_ui(state: State<'_, AppState>) {
    state.reset.reset_now();
}

/// `pick_workspace_folder` 응답 — 고른 폴더를 워크스페이스 생성 인자로 편 형태.
/// 필드명은 글루 DTO 관례(serde 기본 snake_case, `DirEntryDto` 전례) 그대로다.
#[derive(serde::Serialize)]
pub struct PickedFolder {
    /// 워크스페이스 `rootPath` 로 그대로 쓰는 리눅스 절대 경로.
    pub linux_path: String,
    /// `\\wsl.localhost\<distro>\...` 를 골랐을 때의 배포판 이름. 드라이브 경로
    /// (`C:\...` → `/mnt/c/...`)는 배포판을 알 수 없어 None 이고, 그때는 기존
    /// 기본값 해석(워크스페이스 distro 미지정 → WINMUX_DISTRO → wsl 기본)을 탄다.
    pub distro: Option<String>,
    /// 워크스페이스 이름 기본값 — 고른 폴더의 마지막 세그먼트.
    pub name: String,
}

/// 워크스페이스 폴더 선택 (Windows 네이티브 대화상자). 취소는 `Ok(None)` —
/// 에러가 아니다. 선택된 Windows 경로는 `wslpath::from_windows_path` 로 리눅스
/// 경로 + 배포판으로 되돌리고, 되돌릴 수 없는 경로(네트워크 UNC 등)는 그 사유를
/// 그대로 Err 로 올린다 (프론트가 상태 라인에 표시).
///
/// 대화상자는 블로킹 모달이라 통째로 `spawn_blocking` 에서 돈다 — 메인(이벤트
/// 루프) 스레드를 잡으면 대화상자가 떠 있는 동안 앱 전체가 멈춘다. Dispatcher
/// lock 은 타지 않는다 (선택 결과로 CreateWorkspace 를 보내는 것은 프론트 몫).
#[cfg(windows)]
#[tauri::command]
pub async fn pick_workspace_folder() -> Result<Option<PickedFolder>, String> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Select a workspace folder")
            .pick_folder()
    })
    .await
    .map_err(|err| format!("pick_workspace_folder task join failed: {err}"))?;
    let Some(picked) = picked else {
        return Ok(None); // 사용자 취소 — 조용한 no-op
    };
    // 비 UTF-8 Windows 경로는 코어 경로 계약(String)에 실을 수 없다 — 조용히
    // lossy 변환해 다른 폴더를 가리키게 두지 않고 거부한다.
    let path = picked
        .to_str()
        .ok_or_else(|| format!("selected path is not valid UTF-8: {}", picked.display()))?;
    let (distro, linux_path) = wslpath::from_windows_path(path)?;
    Ok(Some(PickedFolder {
        name: folder_name(&linux_path, distro.as_deref()),
        linux_path,
        distro,
    }))
}

/// unix(개발 실행)에는 띄울 네이티브 대화상자가 없다 — 조용한 no-op 이나 가짜
/// 경로로 가리지 않고 명시적으로 실패한다 (`host.rs`·`host_path` 의 cfg 분기와
/// 같은 규율: Windows 전용 기능은 dev 경로에서 loud 하게 없음을 알린다).
#[cfg(not(windows))]
#[tauri::command]
pub async fn pick_workspace_folder() -> Result<Option<PickedFolder>, String> {
    Err("folder picker is Windows-only".to_owned())
}

/// 리눅스 경로의 마지막 세그먼트 (빈 세그먼트는 건너뛴다) — 코어의 탭 제목
/// 규칙(`command.rs::path_title`)과 같은 계산이되, 루트 픽의 퇴화만 보정한다
/// (리뷰 finding): distro 루트("/")는 `"/"` 대신 distro 이름이, 드라이브 루트
/// (`/mnt/c`)는 `"c"` 대신 `"C:"` 가 워크스페이스 이름으로 자연스럽다.
#[cfg(windows)]
fn folder_name(linux_path: &str, distro: Option<&str>) -> String {
    if linux_path == "/" {
        if let Some(d) = distro {
            return d.to_owned();
        }
    }
    if let Some(drive) = linux_path
        .strip_prefix("/mnt/")
        .filter(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return format!("{}:", drive.to_ascii_uppercase());
    }
    linux_path
        .rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or("/")
        .to_owned()
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

// ---------------------------------------------------------------------------
// 뷰어 파일 접근 (21단계 계획 glue 계약)
//
// folderBrowser·textViewer 가 쓰는 읽기 전용 커맨드 3종. Windows 에서는
// `\\wsl.localhost\<distro>\...` UNC 로 접근한다 — Windows→WSL 방향이라 interop 을
// 잠근 배포판에서도 동작한다 (계획 v2 5장). 경로 형태 검증·UNC 조립은 순수 함수
// (`winmux_core::wslpath`)에 있고 테스트도 거기 있다 — 게이트가 `-p winmux-core` 만
// 돌기 때문이다.
//
// **이 3종의 실동작 검증은 UNC·9P 가 필요해 Linux 게이트로는 불가능하다 —
// 체크포인트 2 사용자 체크리스트로 이월하는 것이 계획 명문이다** (21단계 계획
// "완료 기준" 3·6·9·12번 항목).
// ---------------------------------------------------------------------------

/// `fs_list_dir` 한 번이 돌려주는 최대 항목 수. 9P 는 대형 디렉터리에서 느리고
/// 프론트도 이 이상을 한 번에 그리지 않는다 — 넘치면 잘라내고 `truncated` 로
/// 알린다 (계획 리스크 [med] 완화).
const MAX_DIR_ENTRIES: usize = 5_000;

/// `fs_read_chunk` 한 번의 최대 길이 (4 MiB). textViewer 의 윈도우는 512KiB 라
/// 통상 한참 아래고, 이 상한은 프론트 결함이 IPC 로 거대 버퍼를 요구하는 것을
/// 막는다.
const MAX_READ_LEN: u32 = 4 * 1024 * 1024;

/// `fs_list_dir` 응답. **정렬하지 않는다** — dirs-first·name asc 정렬은 프론트
/// 순수 함수(vitest 대상)의 몫이다 (계획 프론트 계약).
#[derive(serde::Serialize)]
pub struct DirListing {
    pub entries: Vec<DirEntryDto>,
    /// 상한(`MAX_DIR_ENTRIES`) 초과로 목록을 잘랐다 — 프론트가 배너로 알린다.
    pub truncated: bool,
}

/// 디렉터리 항목 하나. 필드명은 serde 기본(snake_case) 그대로 나간다 — 글루 DTO
/// 는 `SessionStatsDto` 전례를 따르고 계획의 glue 계약도 이 이름으로 적혀 있다
/// (코어 모델의 camelCase 는 코어 타입 쪽 rename 계약이라 별개다). 타입 이름만
/// `std::fs::DirEntry` 와 겹치지 않게 `Dto` 접미사를 붙였다.
#[derive(serde::Serialize)]
pub struct DirEntryDto {
    pub name: String,
    pub is_dir: bool,
    /// 디렉터리이거나 항목 metadata 조회가 실패하면 None.
    pub size: Option<u64>,
}

/// `fs_stat` 응답 — 링크는 따라간 뒤의 **최종 대상** 기준이다.
#[derive(serde::Serialize)]
pub struct FileStat {
    pub size: u64,
    pub mtime_ms: u64,
    pub is_dir: bool,
}

/// 뷰어 디렉터리 목록 (folderBrowser).
#[tauri::command]
pub async fn fs_list_dir(distro: Option<String>, path: String) -> Result<DirListing, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 경로 해석도 블로킹이다 — Windows 경로는 distro 질의(wsl.exe 스폰)를
        // 유발할 수 있어 해석까지 이 안에서 한다.
        let root = host_path(distro, &path)?;
        list_dir(&root)
    })
    .await
    .map_err(|err| format!("fs_list_dir task join failed: {err}"))?
}

/// 파일 크기·수정시각 조회 (textViewer 윈도우 계산, 청크 D 의 mtime 폴링).
#[tauri::command]
pub async fn fs_stat(distro: Option<String>, path: String) -> Result<FileStat, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let target = host_path(distro, &path)?;
        // `metadata` 는 링크를 따라간다 — 뷰어가 알고 싶은 것은 최종 대상이다.
        let meta = std::fs::metadata(&target)
            .map_err(|err| format!("cannot stat {}: {err}", target.display()))?;
        let modified = meta
            .modified()
            .map_err(|err| format!("cannot read mtime of {}: {err}", target.display()))?;
        // 라이브 리로드의 판정 기준값이라 조회 실패를 조용한 0 으로 대체하지
        // 않는다 (epoch 이전 시각도 이 용도에선 의미가 없어 그대로 에러).
        let since_epoch = modified.duration_since(UNIX_EPOCH).map_err(|err| {
            format!(
                "mtime of {} is before the unix epoch: {err}",
                target.display()
            )
        })?;
        Ok(FileStat {
            size: meta.len(),
            // u128 → u64 는 포화시킨다 (실재하지 않는 범위지만 조용한 절단 금지).
            mtime_ms: u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX),
            is_dir: meta.is_dir(),
        })
    })
    .await
    .map_err(|err| format!("fs_stat task join failed: {err}"))?
}

/// 파일의 바이트 윈도우 읽기 (textViewer). 응답은 **raw 바이트** —
/// `attach_terminal` 과 같은 `tauri::ipc::Response` 경로라 base64 왕복이 없다.
/// UTF-8 파단·부분행 절삭은 프론트 몫이다 (계획 프론트 계약).
///
/// `len` 상한 초과는 조용히 줄이지 않고 **거부**한다 — 요청한 크기와 다른 윈도우가
/// 돌아가면 프론트의 오프셋 계산이 어긋난다. EOF 를 넘는 `offset` 은 빈 응답이며
/// 에러가 아니다 (파일이 그새 줄어든 경우 — 프론트가 윈도우를 되감는다).
#[tauri::command]
pub async fn fs_read_chunk(
    distro: Option<String>,
    path: String,
    offset: u64,
    len: u32,
) -> Result<Response, String> {
    if len > MAX_READ_LEN {
        return Err(format!(
            "fs_read_chunk len {len} exceeds the {MAX_READ_LEN} byte limit"
        ));
    }
    let bytes =
        tauri::async_runtime::spawn_blocking(move || read_chunk(distro, &path, offset, len))
            .await
            .map_err(|err| format!("fs_read_chunk task join failed: {err}"))??;
    Ok(Response::new(bytes))
}

/// 블로킹 디렉터리 열거 — fs 순서 그대로, 상한 초과분은 잘라낸다.
fn list_dir(root: &Path) -> Result<DirListing, String> {
    let iter =
        std::fs::read_dir(root).map_err(|err| format!("cannot list {}: {err}", root.display()))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in iter {
        // 상한 도달 후 **다음 항목이 실재할 때만** truncated 다.
        if entries.len() >= MAX_DIR_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry.map_err(|err| format!("cannot list {}: {err}", root.display()))?;
        entries.push(dir_entry(&entry));
    }
    Ok(DirListing { entries, truncated })
}

/// 항목 하나의 표시 정보. 개별 metadata 조회 실패는 그 항목만 "파일·크기 미상"으로
/// 낮춰 담는다 — 9P 에서 항목 하나의 권한·경합 실패가 목록 전체를 죽이지 않게.
fn dir_entry(entry: &std::fs::DirEntry) -> DirEntryDto {
    let name = entry.file_name().to_string_lossy().into_owned();
    // `file_type`·`DirEntry::metadata` 는 링크를 따라가지 않는다. 클릭 동작이
    // 종류로 갈리므로(디렉터리 탐색 vs 파일 열기) 링크일 때만 대상 metadata 를
    // 한 번 더 조회한다 — 링크가 아닌 항목에는 추가 I/O 가 없다.
    let meta = match entry.file_type() {
        Ok(file_type) if file_type.is_symlink() => std::fs::metadata(entry.path()).ok(),
        _ => entry.metadata().ok(),
    };
    match meta {
        Some(meta) if meta.is_dir() => DirEntryDto {
            name,
            is_dir: true,
            size: None,
        },
        Some(meta) => DirEntryDto {
            name,
            is_dir: false,
            size: Some(meta.len()),
        },
        None => DirEntryDto {
            name,
            is_dir: false,
            size: None,
        },
    }
}

/// 블로킹 윈도우 읽기 — `offset` 에서 최대 `len` 바이트.
fn read_chunk(
    distro: Option<String>,
    path: &str,
    offset: u64,
    len: u32,
) -> Result<Vec<u8>, String> {
    let target = host_path(distro, path)?;
    let mut file = std::fs::File::open(&target)
        .map_err(|err| format!("cannot open {}: {err}", target.display()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|err| format!("cannot seek {} to {offset}: {err}", target.display()))?;
    let mut buf = Vec::new();
    file.take(u64::from(len))
        .read_to_end(&mut buf)
        .map_err(|err| format!("cannot read {}: {err}", target.display()))?;
    Ok(buf)
}

/// 리눅스 경로 → 이 프로세스가 실제로 열 수 있는 호스트 경로.
///
/// Windows 는 `\\wsl.localhost\<distro>\...` UNC 로 조립하고, unix(개발 실행)는
/// 형태 검증만 한 뒤 리눅스 경로를 직접 쓴다 (distro 는 의미가 없다). 스폰 쪽
/// `host.rs::spawn_spec` 의 cfg 분기와 같은 대칭이다.
#[cfg(windows)]
fn host_path(distro: Option<String>, path: &str) -> Result<PathBuf, String> {
    let distro = resolve_distro(distro)?;
    Ok(PathBuf::from(wslpath::to_unc(&distro, path)?))
}

#[cfg(not(windows))]
fn host_path(_distro: Option<String>, path: &str) -> Result<PathBuf, String> {
    wslpath::validate_linux_path(path)?;
    Ok(PathBuf::from(path))
}

/// distro 해석 (계획 21단계 핵심 결정): 인자(workspace.distro) → env `WINMUX_DISTRO`
/// → `wsl.exe -l -q` 기본 배포판 lazy 질의. **셋 다 실패해야** 에러다 — 터미널
/// 스폰(`host.rs`: distro 없으면 wsl.exe 기본값)과 정합을 맞춘 것으로, 둘 다
/// 미설정인 가장 흔한 구성에서 뷰어만 죽는 비대칭을 만들지 않는다. 빈 문자열은
/// 미설정 취급 (`host.rs::spawn_spec` 과 동일).
#[cfg(windows)]
fn resolve_distro(distro: Option<String>) -> Result<String, String> {
    if let Some(distro) = distro.filter(|d| !d.is_empty()) {
        return Ok(distro);
    }
    if let Some(distro) = std::env::var("WINMUX_DISTRO").ok().filter(|d| !d.is_empty()) {
        return Ok(distro);
    }
    default_distro()
}

/// 기본 배포판 이름을 프로세스 수명 동안 캐시한다 — 파일 접근마다 wsl.exe 를
/// 띄우지 않기 위한 캐시라 **성공만** 담는다. 실패까지 캐시하면 앱을 켠 뒤
/// 배포판을 설치·복구한 사용자가 재시작 전까지 영구히 막힌다. (초기 경합으로
/// 질의가 두 번 나갈 수 있으나 결과는 하나로 수렴한다.)
#[cfg(windows)]
pub(crate) fn default_distro() -> Result<String, String> {
    static DEFAULT_DISTRO: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(cached) = DEFAULT_DISTRO.get() {
        return Ok(cached.clone());
    }
    let distro = query_default_distro()?;
    Ok(DEFAULT_DISTRO.get_or_init(|| distro).clone())
}

/// `wsl.exe -l -q` 질의. 출력은 **UTF-16LE** 이고(파이프로 리다이렉트해도 그렇다 —
/// 실검증은 체크포인트 2 항목 12), `-l` 은 기본 배포판을 맨 앞에 내므로 디코드 후
/// 첫 비어있지 않은 줄이 답이다. 실패 메시지에는 사용자가 취할 조치를 함께 적는다.
#[cfg(windows)]
fn query_default_distro() -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let output = std::process::Command::new("wsl.exe")
        .args(["-l", "-q"])
        // 릴리스 빌드는 windows_subsystem="windows" 라 콘솔이 없다 — 이 플래그가
        // 없으면 질의마다 콘솔 창이 깜빡인다.
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| {
            format!(
                "cannot run 'wsl.exe -l -q' to find the default distro: {err}; \
                 set workspace distro or WINMUX_DISTRO"
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "'wsl.exe -l -q' failed ({}): {}; set workspace distro or WINMUX_DISTRO",
            output.status,
            decode_utf16le(&output.stderr).trim()
        ));
    }
    let listing = decode_utf16le(&output.stdout);
    // BOM(U+FEFF)은 공백류가 아니라 trim 으로 떨어지지 않는다 — 명시적으로 벗긴다.
    listing
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            "'wsl.exe -l -q' listed no distro; set workspace distro or WINMUX_DISTRO".to_owned()
        })
}

/// UTF-16LE 바이트열 → String. 짝이 안 맞는 마지막 바이트는 버리고, 부적합
/// 서로게이트는 U+FFFD 로 둔다 (진단 문자열 용도라 lossy 로 충분하다).
#[cfg(windows)]
fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}
