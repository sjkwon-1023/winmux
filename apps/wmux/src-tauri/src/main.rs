//! wmux — Tauri v2 부팅부. 상태 배선(setup)과 커맨드 핸들러 등록만 하고,
//! 로직은 wmux-core 와 commands/host/sink/state 모듈에 있다.
//!
//! # 부팅 순서 (계획 15단계 B-2 · 0장 manage-first)
//!
//! load(state.json) → Restored 면 `Dispatcher::adopt`(스폰 없음) / Fresh 면
//! dogfood → **manage** → 탭별 respawn 루프(회당 lock·publish) → 초기
//! `saver.schedule`. manage 를 재스폰보다 먼저 해야 재스폰된 세션의 on_exit 이
//! 관리 상태를 못 찾아 소실되는 창이 없다. respawn 전 스냅샷의 pty_session
//! null 인 Running 탭은 무해하다 — view-reconcile 은 세션 없는 탭을 attach 하지
//! 않고, publish 도착마다 점진 attach 된다 (계획 0장).

// Windows 릴리스 빌드에서 콘솔 창을 띄우지 않는다 (디버그 빌드는 콘솔 유지).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod host;
mod sink;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;
use wmux_core::command::{Command, Dispatcher, NewTab};
use wmux_core::persist::{self, FreshReason, LoadOutcome, Saver};
use wmux_core::session::SessionManager;

/// Saver debounce 창 — 연속 변이를 1회 기록으로 합친다. 크래시 시 마지막 기록
/// 이후 ≤500ms 의 변이 유실은 MVP 수용 (계획 B-1).
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// corrupt 백업 결과를 로그용 문자열로 — rename 실패도 가리지 않고 원인 그대로.
fn backup_label(backup: &Result<PathBuf, String>) -> String {
    match backup {
        Ok(path) => path.display().to_string(),
        Err(err) => format!("(backup failed: {err})"),
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let sessions = Arc::new(SessionManager::new());
            let sinks = Arc::new(state::SinkRegistry::default());
            let tauri_host =
                host::TauriHost::new(handle.clone(), Arc::clone(&sessions), Arc::clone(&sinks));

            // 상태 파일은 앱 데이터 디렉터리의 state.json. 경로 해석 실패는 부팅
            // 불능이므로 setup 에러로 그대로 올린다 (가짜 진행 금지).
            let state_path = app.path().app_data_dir()?.join("state.json");

            let dispatcher = match persist::load(&state_path) {
                LoadOutcome::Restored { state, repairs } => {
                    for repair in &repairs {
                        eprintln!("[wmux] boot: state repaired: {repair}");
                    }
                    // 스폰 없이 채택만 — 재스폰은 manage 후 아래 루프에서 (모듈
                    // doc 의 manage-first 근거 참조).
                    Dispatcher::adopt(state, Box::new(tauri_host))
                }
                LoadOutcome::Fresh(reason) => {
                    // Fresh 사유를 부팅 결정 로그로 남긴다 — 손상·버전 강등의
                    // 세부는 persist::load 가 이미 stderr 에 남겼다.
                    match &reason {
                        FreshReason::NoFile => eprintln!(
                            "[wmux] boot: no saved state at {} — starting fresh",
                            state_path.display()
                        ),
                        FreshReason::Corrupt { backup, error } => eprintln!(
                            "[wmux] boot: saved state corrupt: {error}; original kept at {} — starting fresh",
                            backup_label(backup)
                        ),
                        FreshReason::UnsupportedVersion { found, backup } => eprintln!(
                            "[wmux] boot: saved state version {found} unsupported; original kept at {} — starting fresh",
                            backup_label(backup)
                        ),
                    }
                    // 부팅 dogfood — 직접 상태 조작 없이 커맨드 bus 경유로 초기
                    // 워크스페이스 + 터미널 탭을 단일 dispatch 로 원자 생성한다
                    // (계획 13-D1). 프론트 attach 전의 셸 프롬프트 출력이 replay
                    // 에 잡히는 것이 attach 프로토콜의 자연 검증이다 (계획 3-C).
                    // 실패는 가리지 않는다 — 부팅 불능이므로 즉시 패닉 (panic
                    // 메시지가 stderr 로그를 겸한다).
                    let mut dispatcher = Dispatcher::new(Box::new(tauri_host));
                    dispatcher
                        .dispatch(Command::CreateWorkspace {
                            name: "main".to_string(),
                            root_path: None,
                            distro: None,
                            tab: Some(NewTab::Terminal { cwd: None }),
                        })
                        .unwrap_or_else(|err| {
                            panic!("[wmux] boot: CreateWorkspace failed: {err}")
                        });
                    dispatcher
                }
            };
            let dispatcher = Arc::new(Mutex::new(dispatcher));
            let saver = Arc::new(Saver::spawn(state_path, SAVE_DEBOUNCE));

            // manage 를 재스폰보다 먼저 (계획 0장) — 재스폰된 세션의 on_exit 은
            // try_state 로 관리 상태를 찾으므로, 스폰이 먼저면 그 사이 exit
            // 이벤트가 소실되는 창이 생긴다.
            app.manage(state::AppState {
                dispatcher: Arc::clone(&dispatcher),
                sessions,
                sinks,
                saver: Arc::clone(&saver),
            });

            // 탭별 재스폰 — 회당 lock 취득·해제 + publish (계획 0장: lock 사이에
            // 도착하는 on_exit/dispatch 가 끼어들 수 있어 이벤트 소실 창이 없다).
            let respawn_targets = dispatcher.lock().unwrap().running_terminal_tabs();
            for tab in respawn_targets {
                let d_guard = &mut *dispatcher.lock().unwrap();
                if let Err(err) = d_guard.respawn_tab(tab) {
                    // 실패는 respawn_tab 이 이미 그 탭을 Exited{None} 으로 강등해
                    // 상태에 반영했다 — 여기서는 loud 기록만 남긴다.
                    eprintln!("[wmux] boot: respawn failed (tab={}): {err}", tab.0);
                }
                state::publish_state(&handle, d_guard);
            }

            // sanitize·수리·강등 결과를 즉시 디스크에 반영한다 (계획 0장 초기
            // 저장) — 이 시점 상태가 다음 크래시 복원의 기준선이 된다. Fresh
            // 부팅의 초기 워크스페이스도 이 한 번으로 저장된다.
            saver.schedule(dispatcher.lock().unwrap().state().clone());
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
        .build(tauri::generate_context!())
        .expect("error while building wmux")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 종료 직전 대기분 flush — debounce 창(≤500ms) 안의 마지막 변이가
                // 정상 종료에서 유실되지 않게 한다 (크래시 유실은 계획상 수용).
                match app.try_state::<state::AppState>() {
                    Some(managed) => managed.saver.flush(),
                    // setup 실패로 manage 전에 종료되는 경로뿐 — flush 할 상태
                    // 자체가 없다.
                    None => eprintln!("[wmux] exit: managed state unavailable; nothing to flush"),
                }
            }
        });
}
