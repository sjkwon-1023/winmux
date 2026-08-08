//! `SessionSink` 구현 — wmux-core 세션의 출력·이벤트를 프론트엔드로 나른다.
//!
//! - 터미널 출력: `tauri::ipc::Channel`에 `InvokeResponseBody::Raw`로 바이너리 그대로
//!   전송한다. JSON 직렬화 금지(계획 v2 2·12장) — 프론트엔드는 ArrayBuffer로 받는다.
//! - OSC·exit: 저빈도이므로 일반 Tauri 이벤트(JSON)로 emit한다.

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use wmux_core::osc::OscEvent;
use wmux_core::session::SessionSink;

/// `osc-event` 이벤트 payload — 프론트엔드 계약과 일치:
/// `{ id, kind: "777"|"9"|"7"|"0", title, body }`.
#[derive(Clone, serde::Serialize)]
pub struct OscEventPayload {
    pub id: u32,
    pub kind: &'static str,
    pub title: String,
    pub body: String,
}

impl OscEventPayload {
    /// wmux-core `OscEvent` → 프론트엔드 계약 형태로 변환.
    /// kind별 배치: 777은 title/body 그대로, 9는 알림 메시지를 body에,
    /// 7은 cwd URI를 body에, 0은 창 제목을 title에 둔다 (비는 칸은 빈 문자열).
    pub fn from_event(id: u32, event: &OscEvent) -> Self {
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

/// `terminal-exit` 이벤트 payload — `{ id, code }` (code는 null 가능).
#[derive(Clone, serde::Serialize)]
pub struct TerminalExitPayload {
    pub id: u32,
    pub code: Option<u32>,
}

/// 세션 1개 분의 sink. 리더 스레드에서 호출되므로 Send + 'static.
pub struct ChannelSink {
    id: u32,
    channel: Channel<InvokeResponseBody>,
    app: AppHandle,
}

impl ChannelSink {
    pub fn new(id: u32, channel: Channel<InvokeResponseBody>, app: AppHandle) -> Self {
        Self { id, channel, app }
    }
}

impl SessionSink for ChannelSink {
    fn on_output(&self, bytes: &[u8]) {
        // sink 시그니처상 에러를 되돌릴 수 없으므로, 실패(webview 종료 등)는
        // 삼키지 않고 stderr에 남긴다.
        if let Err(err) = self.channel.send(InvokeResponseBody::Raw(bytes.to_vec())) {
            eprintln!(
                "[wmux-spike] raw output channel send failed (id={}): {err}",
                self.id
            );
        }
    }

    fn on_osc(&self, event: &OscEvent) {
        let payload = OscEventPayload::from_event(self.id, event);
        if let Err(err) = self.app.emit("osc-event", payload) {
            eprintln!("[wmux-spike] osc-event emit failed (id={}): {err}", self.id);
        }
    }

    fn on_exit(&self, code: Option<u32>) {
        let payload = TerminalExitPayload { id: self.id, code };
        if let Err(err) = self.app.emit("terminal-exit", payload) {
            eprintln!(
                "[wmux-spike] terminal-exit emit failed (id={}): {err}",
                self.id
            );
        }
    }
}
