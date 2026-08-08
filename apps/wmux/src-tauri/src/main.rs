//! wmux — Tauri v2 부팅부. 상태 배선(setup)과 커맨드 핸들러 등록만 하고,
//! 로직은 wmux-core 와 commands/host/sink/state 모듈에 있다.

// Windows 릴리스 빌드에서 콘솔 창을 띄우지 않는다 (디버그 빌드는 콘솔 유지).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod host;
mod sink;
mod state;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use wmux_core::command::{Command, CommandOutput, Dispatcher, NewTab};
use wmux_core::session::SessionManager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let sessions = Arc::new(SessionManager::new());
            let sinks = Arc::new(state::SinkRegistry::default());
            let tauri_host =
                host::TauriHost::new(handle, Arc::clone(&sessions), Arc::clone(&sinks));
            let dispatcher = Arc::new(Mutex::new(Dispatcher::new(Box::new(tauri_host))));
            app.manage(state::AppState {
                dispatcher: Arc::clone(&dispatcher),
                sessions,
                sinks,
            });

            // 부팅 dogfood — 직접 상태 조작 없이 커맨드 bus 경유로 초기
            // 워크스페이스 + 터미널 탭을 만든다. 프론트 attach 전의 셸 프롬프트
            // 출력이 replay 에 잡히는 것이 attach 프로토콜의 자연 검증이다
            // (계획 3-C). 실패는 가리지 않는다 — 부팅 불능이므로 즉시 패닉
            // (panic 메시지가 stderr 로그를 겸한다).
            let mut d = dispatcher.lock().unwrap();
            let out = d
                .dispatch(Command::CreateWorkspace {
                    name: "main".to_string(),
                    root_path: None,
                    distro: None,
                })
                .unwrap_or_else(|err| panic!("[wmux] boot: CreateWorkspace failed: {err}"));
            let CommandOutput::WorkspaceCreated { pane, .. } = out else {
                panic!("[wmux] boot: unexpected CreateWorkspace output: {out:?}");
            };
            d.dispatch(Command::CreateTab {
                pane,
                tab: NewTab::Terminal { cwd: None },
            })
            .unwrap_or_else(|err| panic!("[wmux] boot: CreateTab failed: {err}"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::dispatch,
            commands::get_state,
            commands::attach_terminal,
            commands::detach_terminal,
            commands::write_stdin,
            commands::send_raw,
            commands::resize,
            commands::ack_output,
            commands::get_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running wmux");
}
