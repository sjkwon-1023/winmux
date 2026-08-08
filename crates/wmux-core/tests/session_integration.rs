//! PtySession unix 통합 테스트 — 실제 `sh` 를 PTY 로 띄워 계약을 검증한다.
//! 계약: `docs/plans/spike-plan.md` 4.4장. Windows 에서는 컴파일되지 않는다.
#![cfg(unix)]

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use wmux_core::osc::OscEvent;
use wmux_core::session::{PtySession, SessionManager, SessionOptions, SessionSink, SpawnSpec};

/// 개별 대기 상한 — hang 방지 가드. WSL 부하를 감안해 넉넉히 잡는다.
const TIMEOUT: Duration = Duration::from_secs(10);

/// sink 콜백을 채널로 옮겨 테스트 본문에서 순서대로 검사한다.
#[derive(Debug)]
enum Event {
    Output(Vec<u8>),
    Osc(OscEvent),
    Exit(Option<u32>),
}

struct ChannelSink {
    tx: Sender<Event>,
}

impl SessionSink for ChannelSink {
    fn on_output(&self, bytes: &[u8]) {
        // 테스트가 먼저 끝나 수신자가 drop 된 뒤의 send 실패는 무해하다.
        let _ = self.tx.send(Event::Output(bytes.to_vec()));
    }
    fn on_osc(&self, event: &OscEvent) {
        let _ = self.tx.send(Event::Osc(event.clone()));
    }
    fn on_exit(&self, code: Option<u32>) {
        let _ = self.tx.send(Event::Exit(code));
    }
}

fn sh_spec() -> SpawnSpec {
    SpawnSpec {
        program: "sh".into(),
        args: vec![],
        cwd: None,
        cols: 80,
        rows: 24,
    }
}

fn spawn_sh(opts: SessionOptions) -> (PtySession, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let session = PtySession::spawn(sh_spec(), Box::new(ChannelSink { tx }), opts)
        .expect("failed to spawn sh in pty");
    (session, rx)
}

/// deadline 까지 남은 시간. 이미 지났으면 무엇을 기다리다 초과했는지 알리며 실패.
fn remaining(deadline: Instant, waiting_for: &str) -> Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_else(|| panic!("timed out waiting for {waiting_for}"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// 1) sh spawn → "echo hello" write → on_output 으로 hello 수신.
#[test]
fn spawn_sh_and_receive_echo_output() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    session.write(b"echo hello\n").expect("write echo command");

    let deadline = Instant::now() + TIMEOUT;
    let mut acc: Vec<u8> = Vec::new();
    while !contains(&acc, b"hello") {
        match rx.recv_timeout(remaining(deadline, "echo output")) {
            Ok(Event::Output(bytes)) => acc.extend_from_slice(&bytes),
            Ok(_) => {}
            Err(err) => panic!(
                "no output containing 'hello' ({err}); received {} bytes: {:?}",
                acc.len(),
                String::from_utf8_lossy(&acc)
            ),
        }
    }
    session.kill();
}

// 2) printf 로 OSC 777 방출 → on_osc 이벤트 수신 (BEL 종결·ST 종결 각 1회).
#[test]
fn osc777_events_received_for_bel_and_st() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    // BEL(\007) 종결. sh 의 printf 가 \033/\007 을 실제 제어 바이트로 변환한다.
    session
        .write(b"printf '\\033]777;notify;T1;B1\\007'\n")
        .expect("write printf (BEL)");
    // ST(ESC \) 종결. 작은따옴표 안 `\\` 는 printf 가 `\` 하나로 변환한다.
    session
        .write(b"printf '\\033]777;notify;T2;B2\\033\\\\'\n")
        .expect("write printf (ST)");

    let deadline = Instant::now() + TIMEOUT;
    let mut oscs: Vec<OscEvent> = Vec::new();
    while oscs.len() < 2 {
        match rx.recv_timeout(remaining(deadline, "2 OSC events")) {
            Ok(Event::Osc(event)) => oscs.push(event),
            Ok(_) => {}
            Err(err) => panic!("expected 2 OSC events, got {oscs:?} ({err})"),
        }
    }
    assert_eq!(
        oscs,
        vec![
            OscEvent::Osc777Notify {
                title: "T1".into(),
                body: "B1".into()
            },
            OscEvent::Osc777Notify {
                title: "T2".into(),
                body: "B2".into()
            },
        ]
    );
    session.kill();
}

// 3) 대량 출력 + 무ack → paused 전환, pending 이 high_water 부근에서 멈춤,
//    전량 ack → 재개되어 추가 출력 수신.
#[test]
fn flood_without_ack_pauses_then_full_ack_resumes() {
    const HIGH: usize = 64 * 1024;
    const LOW: usize = 16 * 1024;
    let opts = SessionOptions {
        replay_cap: 64 * 1024,
        high_water: HIGH,
        low_water: LOW,
    };
    let (session, rx) = spawn_sh(opts);
    session.write(b"seq 200000\n").expect("write seq command");

    // ack 없이 paused 전환을 기다린다.
    let deadline = Instant::now() + TIMEOUT;
    while !session.stats().paused {
        let _ = remaining(deadline, "paused transition");
        thread::sleep(Duration::from_millis(10));
    }

    // pending 은 high_water 도달 직후에서 멈춰야 한다 — 마지막 read chunk
    // 하나만큼의 overshoot 만 허용한다.
    let paused_stats = session.stats();
    assert!(
        paused_stats.pending >= HIGH,
        "pending {} below high_water {HIGH} while paused",
        paused_stats.pending
    );
    assert!(
        paused_stats.pending < HIGH + 64 * 1024,
        "pending {} ran away past high_water {HIGH}",
        paused_stats.pending
    );

    // read 자체가 멈췄는지 확인: pause 직전 chunk 의 in-flight 송신을 정리한 뒤
    // 조용한지, pending 이 변하지 않는지 본다.
    thread::sleep(Duration::from_millis(200));
    while rx.try_recv().is_ok() {}
    thread::sleep(Duration::from_millis(300));
    assert!(
        rx.try_recv().is_err(),
        "output kept flowing while session was paused"
    );
    assert_eq!(
        session.stats().pending,
        paused_stats.pending,
        "pending changed while paused without ack"
    );

    // 전량 ack → Resume → 추가 출력이 도착해야 한다.
    session.ack(paused_stats.pending);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match rx.recv_timeout(remaining(deadline, "output after resume")) {
            Ok(Event::Output(_)) => break,
            Ok(_) => {}
            Err(err) => panic!("no output arrived after full ack ({err})"),
        }
    }
    session.kill();
}

// 4) exit → on_exit 호출 (exit code 전달, stats.alive false).
#[test]
fn exit_invokes_on_exit_with_code() {
    let (session, rx) = spawn_sh(SessionOptions::default());
    session.write(b"exit 3\n").expect("write exit command");

    let deadline = Instant::now() + TIMEOUT;
    let code = loop {
        match rx.recv_timeout(remaining(deadline, "exit event")) {
            Ok(Event::Exit(code)) => break code,
            Ok(_) => {}
            Err(err) => panic!("no exit event received ({err})"),
        }
    };
    assert_eq!(code, Some(3));
    // on_exit 직전에 alive 가 내려가므로 이벤트 수신 후에는 false 가 보장된다.
    assert!(
        !session.stats().alive,
        "stats.alive must be false after exit"
    );
}

// SessionManager — id 발급, create/get/remove/stats.
#[test]
fn manager_create_get_remove_stats() {
    let manager = SessionManager::new();
    let (tx, _rx) = mpsc::channel();

    let id1 = manager
        .create(
            sh_spec(),
            Box::new(ChannelSink { tx: tx.clone() }),
            SessionOptions::default(),
        )
        .expect("create session 1");
    let id2 = manager
        .create(
            sh_spec(),
            Box::new(ChannelSink { tx: tx.clone() }),
            SessionOptions::default(),
        )
        .expect("create session 2");
    assert_ne!(id1, id2);
    assert!(manager.get(id1).is_some());
    assert!(manager.get(id2).is_some());

    let stats = manager.stats();
    assert_eq!(stats.len(), 2);
    let ids: Vec<u32> = stats.iter().map(|s| s.id).collect();
    assert!(ids.contains(&id1) && ids.contains(&id2));
    assert!(stats.iter().all(|s| s.alive));

    assert!(manager.remove(id1));
    assert!(manager.get(id1).is_none());
    assert!(!manager.remove(id1), "second remove must be a no-op");
    assert_eq!(manager.stats().len(), 1);
    assert!(manager.remove(id2));
    assert!(manager.stats().is_empty());
}
