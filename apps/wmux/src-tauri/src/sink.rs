//! `SessionSink` 구현 — 세션 출력·이벤트를 프론트엔드로 나른다.
//!
//! - 터미널 출력: `tauri::ipc::Channel` 에 `[u64 LE offset][bytes]` 프레임을
//!   `InvokeResponseBody::Raw` 로 전송한다 (JSON 직렬화 금지 — 계획 v2 2·12장).
//!   프론트는 offset 으로 replay 스냅샷과의 겹침을 dedup 한다 (계획 2장).
//! - exit: Dispatcher 에 `SessionExited` 를 반영하고 `publish_state` 로
//!   `state-changed` emit + 저장 예약한다 (dispatch 와 함께 상태 변이의 유일한
//!   두 경로 — 계획 15단계 B-2 저장 훅).
//! - OSC: 10단계에서는 상태 반영 없이 `osc-event` emit 만 남긴다 (18단계 대비,
//!   JSON 이지만 저빈도라 수용).

use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};
use wmux_core::command::SessionEvent;
use wmux_core::osc::OscEvent;
use wmux_core::session::{Delivery, SessionId, SessionSink};

use crate::state::{publish_state, AppState};

/// `osc-event` payload — spike 프론트 계약과 동일 형태:
/// `{ id, kind: "777"|"9"|"7"|"0", title, body }` (비는 칸은 빈 문자열).
#[derive(Clone, serde::Serialize)]
pub struct OscEventPayload {
    pub id: SessionId,
    pub kind: &'static str,
    pub title: String,
    pub body: String,
}

impl OscEventPayload {
    fn from_event(id: SessionId, event: &OscEvent) -> Self {
        match event {
            OscEvent::Osc0Title(title) => Self {
                id,
                kind: "0",
                title: title.clone(),
                body: String::new(),
            },
            OscEvent::Osc7Cwd(uri) => Self {
                id,
                kind: "7",
                title: String::new(),
                body: uri.clone(),
            },
            OscEvent::Osc9Notify(message) => Self {
                id,
                kind: "9",
                title: String::new(),
                body: message.clone(),
            },
            OscEvent::Osc777Notify { title, body } => Self {
                id,
                kind: "777",
                title: title.clone(),
                body: body.clone(),
            },
        }
    }
}

/// 세션 1개 분의 sink. 레지스트리(`SinkRegistry`)와 리더 스레드([`SinkHandle`])가
/// `Arc` 로 공유한다 — attach_terminal 이 실행 중인 세션의 채널 슬롯을 갈아끼울
/// 수 있어야 하기 때문.
pub struct TerminalSink {
    session: SessionId,
    app: AppHandle,
    /// 프론트 출력 채널 슬롯. attach 시 장착되고, send 실패(webview 소멸 등)
    /// 시 해제된다 — 이후 출력은 Dropped(detach 모드)로 흐른다.
    channel: Mutex<Option<Channel<InvokeResponseBody>>>,
    /// 이 세션에 채널이 장착된 적이 있는가 — attach_terminal 이 "최초 attach"
    /// 판정에 쓴다. 최초 attach 의 replay 에 담긴 단말 질의(ConPTY 의 ESC[6n 등)
    /// 는 아직 아무도 응답하지 않은 **라이브 질의**라 xterm 이 응답해야 하고
    /// (미응답 시 conhost 가 CPR 을 기다리며 셸이 멈춘다 — 체크포인트 1 재시작
    /// 빈 화면 버그), 재-attach 의 replay 질의는 이전 프론트가 이미 응답한 낡은
    /// 질의라 응답을 억제해야 한다 (stray `R` 버그).
    attached_once: std::sync::atomic::AtomicBool,
}

impl TerminalSink {
    pub fn new(session: SessionId, app: AppHandle) -> Self {
        Self {
            session,
            app,
            channel: Mutex::new(None),
            attached_once: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// attach 이력을 기록하고 **이전 값**(이미 attach 된 적 있었는지)을 돌려준다.
    pub fn mark_attached(&self) -> bool {
        self.attached_once
            .swap(true, std::sync::atomic::Ordering::Relaxed)
    }

    /// 새 출력 채널을 슬롯에 장착한다 (기존 채널은 대체·폐기).
    ///
    /// **호출 순서 불변식**: `attach_terminal` 은 반드시 이 장착을 먼저 하고
    /// `PtySession::reattach()` 를 나중에 호출해야 한다 — 순서를 바꾸면 그 사이
    /// 출력이 스냅샷에도 채널에도 담기지 않는 유실 창이 생긴다 (reattach rustdoc).
    pub fn attach(&self, channel: Channel<InvokeResponseBody>) {
        *self.channel.lock().unwrap() = Some(channel);
    }

    /// 채널 슬롯을 분리한다 — 뷰 dispose(탭 전환 등) 시 호출. 슬롯이 남아 있으면
    /// 이후 출력이 send 성공(Delivered)인데 프론트는 버리고 ack 하지 않아 pending
    /// 이 high_water 까지 쌓여 백그라운드 세션이 paused 에 고착된다. 분리하면
    /// 출력이 Dropped(detach 모드)로 보상 롤백돼 세션이 자유 진행한다.
    pub fn detach(&self) {
        *self.channel.lock().unwrap() = None;
    }
}

/// `SessionManager::create` 에 넘기는 `Box<dyn SessionSink>` 어댑터 —
/// 레지스트리와 같은 `TerminalSink` 를 공유한다.
pub struct SinkHandle(pub Arc<TerminalSink>);

impl SessionSink for SinkHandle {
    fn on_output(&self, offset: u64, bytes: &[u8]) -> Delivery {
        let mut slot = self.0.channel.lock().unwrap();
        let Some(channel) = slot.as_ref() else {
            // 채널 미장착(attach 전·해제 후) — 리더가 flow 를 보상 롤백하는
            // detach 모드. 출력은 replay buffer 에만 쌓인다.
            return Delivery::Dropped;
        };
        let mut frame = Vec::with_capacity(8 + bytes.len());
        frame.extend_from_slice(&offset.to_le_bytes());
        frame.extend_from_slice(bytes);
        match channel.send(InvokeResponseBody::Raw(frame)) {
            Ok(()) => Delivery::Delivered,
            Err(err) => {
                // send 실패(webview 소멸 등) — 죽은 채널을 슬롯에서 해제해 이후
                // chunk 가 send 시도 없이 Dropped 로 빠지게 한다. 실패 자체는
                // 삼키지 않고 stderr 에 남긴다.
                eprintln!(
                    "[wmux] output channel send failed (session={}): {err}",
                    self.0.session
                );
                *slot = None;
                Delivery::Dropped
            }
        }
    }

    fn on_osc(&self, event: &OscEvent) {
        // 10단계에서는 상태 반영 없음 — emit 만 (18단계 대비).
        let payload = OscEventPayload::from_event(self.0.session, event);
        if let Err(err) = self.0.app.emit("osc-event", payload) {
            eprintln!(
                "[wmux] osc-event emit failed (session={}): {err}",
                self.0.session
            );
        }
    }

    fn on_exit(&self, code: Option<u32>) {
        // 리더 스레드에서 Dispatcher lock 을 잡는다 — 교착 없음: dispatch 가
        // lock 아래에서 부르는 kill 은 join 없는 신호라(코어 rustdoc) 리더의
        // 종료를 기다리지 않는다. 미지 세션(CloseTab 선행)은 코어 apply_event
        // 가 무해한 no-op 으로 보장한다 (계획 0-5).
        let Some(state) = self.0.app.try_state::<AppState>() else {
            // 앱 teardown 중 관리 상태가 이미 내려간 경우뿐 — 기록만 남긴다.
            eprintln!(
                "[wmux] on_exit: managed state unavailable (session={})",
                self.0.session
            );
            return;
        };
        let mut dispatcher = state.dispatcher.lock().unwrap();
        dispatcher.apply_event(SessionEvent::SessionExited {
            session: self.0.session,
            code,
        });
        publish_state(&self.0.app, &dispatcher);
    }
}
