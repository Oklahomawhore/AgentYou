use std::{path::PathBuf, process::Command};

#[test]
fn replay_is_deterministic_and_never_overwrites_database() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("replay.sqlite");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_mind-runtime"))
            .current_dir(&root)
            .args(["replay", "examples/events.jsonl", "--db"])
            .arg(&db)
            .args(["--guards", "examples/task-updates.json"])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows.iter()
            .map(|r| r["action"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["speak", "duplicate", "wait", "respond", "wait", "wait"]
    );
    assert!(rows.iter().all(|r| r["shadow"] == true));
    let before = std::fs::read(&db).unwrap();
    assert!(!run().status.success());
    assert_eq!(before, std::fs::read(&db).unwrap());
    let inspected = Command::new(env!("CARGO_BIN_EXE_mind-runtime"))
        .arg("inspect")
        .arg(&db)
        .output()
        .unwrap();
    assert!(inspected.status.success());
    assert_eq!(
        String::from_utf8(inspected.stdout).unwrap().lines().count(),
        4
    );
    assert_eq!(before, std::fs::read(&db).unwrap());
}
