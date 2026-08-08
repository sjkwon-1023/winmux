//! golden fixture round-trip 검증 (10단계 계획 0-7).
//!
//! `fixtures/stage10-*.json` 은 프론트(vitest)와 **공유하는 직렬화 계약**이다 —
//! 같은 파일을 cargo test 는 serde round-trip 으로, vitest 는 TS 타입 파싱으로
//! 소비해 양쪽 표류를 막는다. Rust 단독 round-trip 은 계약 검증으로 무효
//! (fixture 파일이 기준).

use serde::{Deserialize, Serialize};
use wmux_core::command::{Command, CommandError, CommandOutput};
use wmux_core::model::{AppState, PaneId, TabId, WorkspaceId};

fn read_fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {} 읽기 실패: {e}", path.display()))
}

/// `StateSnapshot` 은 borrow 라 Deserialize 가 없다 — 검증용 owned 대응물.
/// 직렬화 형태(camelCase 키)는 동일해야 한다.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnedSnapshot {
    revision: u64,
    state: AppState,
}

#[test]
fn snapshot_fixture_round_trips() {
    let text = read_fixture("stage10-snapshot.json");
    let original: serde_json::Value = serde_json::from_str(&text).unwrap();
    let parsed: OwnedSnapshot = serde_json::from_str(&text).unwrap();

    // fixture → 타입 → JSON 재직렬화가 원본과 일치해야 계약 성립 (키 이름·null
    // 필드·맵 키 문자열화까지 전부 검증된다).
    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(original, reserialized);

    // 스냅샷 revision 은 상태 revision 의 복제다.
    assert_eq!(parsed.revision, parsed.state.revision);

    // 대표 상태의 모델 불변식도 성립해야 한다.
    for ws in &parsed.state.workspaces {
        ws.debug_assert_invariants();
    }
}

#[test]
fn empty_snapshot_fixture_round_trips() {
    // 마지막 워크스페이스가 닫힌 도달 가능 상태 — activeWorkspace null 이
    // TS 타입에서 nullable 로 잡히는지 vitest 쪽에서도 같은 파일로 검증한다.
    let text = read_fixture("stage10-snapshot-empty.json");
    let original: serde_json::Value = serde_json::from_str(&text).unwrap();
    let parsed: OwnedSnapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(original, serde_json::to_value(&parsed).unwrap());
    assert!(parsed.state.workspaces.is_empty());
    assert_eq!(parsed.state.active_workspace, None);
}

#[test]
fn outputs_fixture_matches_serialization() {
    // dispatch 응답 표면(dev 훅·MCP 가 물릴 CommandOutput/CommandError JSON)도
    // fixture 로 잠근다. 두 타입은 Serialize 전용이므로 "타입 → JSON == fixture"
    // 단방향만 검증한다 (round-trip 불가).
    let text = read_fixture("stage10-outputs.json");
    let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();

    // 전 CommandOutput variant 1개씩 (4종) — variant 추가 시 fixture 도 갱신할 것.
    let outputs = vec![
        CommandOutput::WorkspaceCreated {
            workspace: WorkspaceId(1),
            pane: PaneId(2),
        },
        CommandOutput::PaneCreated { pane: PaneId(9) },
        CommandOutput::TabCreated {
            tab: TabId(4),
            session: Some(11),
        },
        CommandOutput::Done,
    ];
    // 전 CommandError variant 1개씩 (3종).
    let errors = vec![
        CommandError::UnknownTarget {
            target: "pane 99".to_string(),
        },
        CommandError::LastPane,
        CommandError::SpawnFailed {
            message: "wsl.exe: program not found".to_string(),
        },
    ];

    let expected = serde_json::json!({ "outputs": outputs, "errors": errors });
    assert_eq!(fixture, expected);
}

#[test]
fn commands_fixture_round_trips() {
    let text = read_fixture("stage10-commands.json");
    let original: serde_json::Value = serde_json::from_str(&text).unwrap();
    let parsed: Vec<Command> = serde_json::from_str(&text).unwrap();

    // 전 Command variant 1개씩 (9종) — variant 추가 시 fixture 도 갱신할 것.
    assert_eq!(parsed.len(), 9);

    let reserialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(original, reserialized);
}
