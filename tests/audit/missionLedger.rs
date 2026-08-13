//! Mission ledger functional gate: state, Receipt, compaction, restart, and exclusive ownership.

use runtrol_ledger::{
    ArtifactEvidence, GateEvidence, Ledger, LedgerSnapshot, MissionId, MissionRecord, MissionState,
    ProviderObservation, Receipt, ReceiptError, ReceiptInput, RunId, TaskId, TaskRecord,
    TransitionApplied,
};
use runtrol_provider::AbsPath;

fn receipt_input() -> ReceiptInput {
    ReceiptInput {
        mission_id: MissionId::now(),
        task_id: TaskId::now(),
        run_id: RunId::now(),
        project_id: "project-fixture".into(),
        instruction_sha256: [1; 32],
        base_commit: "base-fixture".into(),
        finish_tree: "tree-fixture".into(),
        provider_observation: ProviderObservation {
            runtime_id: "opaque-runtime".into(),
            binary_fingerprint: [2; 32],
            model: None,
            native_session_id: "opaque-native-session".into(),
        },
        artifacts: vec![ArtifactEvidence {
            path: "report.md".into(),
            sha256: [3; 32],
            size: 7,
        }],
        gates: vec![GateEvidence {
            id: "fixed-check".into(),
            definition_sha256: [4; 32],
            status: "passed".into(),
        }],
        capability_versions: Vec::new(),
        policy_sha256: [5; 32],
    }
}

#[test]
fn evidence_completeness_refuses_missing_required_classes() {
    assert_eq!(
        Receipt::seal(ReceiptInput {
            project_id: "".into(),
            ..receipt_input()
        }),
        Err(ReceiptError::MissingIdentity)
    );
    assert_eq!(
        Receipt::seal(ReceiptInput {
            artifacts: Vec::new(),
            ..receipt_input()
        }),
        Err(ReceiptError::MissingEvidence)
    );
    assert_eq!(
        Receipt::seal(ReceiptInput {
            gates: Vec::new(),
            ..receipt_input()
        }),
        Err(ReceiptError::MissingEvidence)
    );
}

#[test]
fn canonical_encoding_sorts_repeated_evidence() {
    let base = receipt_input();
    let artifact = ArtifactEvidence {
        path: "a.md".into(),
        sha256: [9; 32],
        size: 1,
    };
    let gate = GateEvidence {
        id: "a-check".into(),
        definition_sha256: [8; 32],
        status: "passed".into(),
    };
    let mut forward = base.clone();
    forward.artifacts.push(artifact.clone());
    forward.gates.push(gate.clone());
    let mut reverse = base;
    reverse.artifacts.insert(0, artifact);
    reverse.gates.insert(0, gate);
    let (forward_id, forward_receipt) = Receipt::seal(forward).expect("forward receipt");
    let (reverse_id, reverse_receipt) = Receipt::seal(reverse).expect("reverse receipt");
    assert_eq!(forward_id, reverse_id);
    assert_eq!(forward_receipt, reverse_receipt);
}

#[test]
fn state_events_are_legal_and_idempotent() {
    let mut mission = MissionRecord::draft([7; 32], "project".into());
    assert_eq!(
        mission.transition(
            "validate-1".into(),
            MissionState::Draft,
            MissionState::Validated
        ),
        Ok(TransitionApplied::Changed)
    );
    assert_eq!(
        mission.transition(
            "validate-1".into(),
            MissionState::Draft,
            MissionState::Validated
        ),
        Ok(TransitionApplied::Duplicate)
    );
    assert!(
        mission
            .transition(
                "skip".into(),
                MissionState::Validated,
                MissionState::Completed
            )
            .is_err()
    );
}

#[test]
fn active_recovery_state_is_never_compacted() {
    let mission = MissionRecord::draft([7; 32], "project".into());
    let mut snapshot = LedgerSnapshot {
        mission,
        tasks: Vec::new(),
        runs: Vec::new(),
        gate_runs: Vec::new(),
        artifacts: Vec::new(),
        receipts: Vec::new(),
        compacted: false,
    };
    snapshot.compact();
    assert!(!snapshot.compacted);
}

#[test]
fn durable_snapshot_survives_reopen() {
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos()
    );
    let root = std::env::temp_dir().join(format!("runtrol-mission-ledger-gate-{suffix}"));
    std::fs::create_dir_all(&root).expect("create scratch");
    let database = AbsPath::canonicalize(root.to_str().expect("UTF-8 scratch"))
        .expect("canonical scratch")
        .join("mission.redb")
        .expect("database path");
    let mission = MissionRecord::draft([7; 32], "project".into());
    let mission_id = mission.id;
    let snapshot = LedgerSnapshot {
        mission,
        tasks: Vec::new(),
        runs: Vec::new(),
        gate_runs: Vec::new(),
        artifacts: Vec::new(),
        receipts: Vec::new(),
        compacted: false,
    };
    {
        let ledger = Ledger::open(&database).expect("open ledger");
        ledger.put(&snapshot).expect("write snapshot");
    }
    let recovered = Ledger::open(&database)
        .expect("reopen ledger")
        .snapshot(mission_id)
        .expect("read snapshot")
        .expect("snapshot exists");
    assert_eq!(recovered, snapshot);
    std::fs::remove_dir_all(root).expect("remove scratch");
}

#[test]
fn one_hundred_mission_snapshot_with_one_thousand_tasks_stays_below_fifty_ms_p95() {
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos()
    );
    let root = std::env::temp_dir().join(format!("runtrol-mission-scale-gate-{suffix}"));
    std::fs::create_dir_all(&root).expect("create scale scratch");
    let database = AbsPath::canonicalize(root.to_str().expect("UTF-8 scratch"))
        .expect("canonical scale scratch")
        .join("mission.redb")
        .expect("scale database path");
    let ledger = Ledger::open(&database).expect("open scale ledger");
    for mission_index in 0_u16..100 {
        let mut mission = MissionRecord::draft([7; 32], format!("project-{mission_index}").into());
        mission.display_name = format!("Mission {mission_index:03}").into();
        let tasks = (0_u16..10)
            .map(|task_index| {
                let mut task = TaskRecord::pending(
                    mission.id,
                    format!("instructions/{task_index}.md").into(),
                    [u8::try_from(task_index).expect("fixture index fits"); 32],
                );
                task.task_key = format!("task-{task_index}").into();
                task
            })
            .collect();
        ledger
            .put(&LedgerSnapshot {
                mission,
                tasks,
                runs: Vec::new(),
                gate_runs: Vec::new(),
                artifacts: Vec::new(),
                receipts: Vec::new(),
                compacted: false,
            })
            .expect("write scale Mission");
    }

    let mut samples = Vec::with_capacity(20);
    for _trial in 0..20 {
        let started = std::time::Instant::now();
        let listed = ledger.list(100).expect("list scale Missions");
        samples.push(started.elapsed());
        assert!(!listed.truncated);
        assert_eq!(listed.missions.len(), 100);
        assert_eq!(
            listed
                .missions
                .iter()
                .map(|snapshot| snapshot.tasks.len())
                .sum::<usize>(),
            1_000
        );
    }
    samples.sort_unstable();
    let p95 = samples[18];
    assert!(
        p95 <= std::time::Duration::from_millis(50),
        "100 Mission and 1,000 Task snapshot p95 was {p95:?}"
    );
    drop(ledger);
    std::fs::remove_dir_all(root).expect("remove scale scratch");
}
