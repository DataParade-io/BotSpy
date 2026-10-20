//! Source-preservation regressions use synthetic databases only.

use botspy::snapshot::snapshot_sqlite;
use std::path::Path;

fn create_source(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES (1);")
        .unwrap();
}

#[test]
fn refuses_to_snapshot_over_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("snapshot.db");
    create_source(&source);
    let before = std::fs::read(&source).unwrap();
    assert!(snapshot_sqlite(&source, dir.path()).is_err());
    assert!(std::fs::read(&source).unwrap() == before, "source changed");
}

#[test]
fn failed_snapshot_keeps_the_previous_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("invalid.db");
    std::fs::write(&source, b"not a database").unwrap();
    let output = dir.path().join("snapshot.db");
    create_source(&output);
    let before = std::fs::read(&output).unwrap();
    assert!(snapshot_sqlite(&source, dir.path()).is_err());
    assert!(std::fs::read(&output).unwrap() == before, "output changed");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn refuses_hard_link_alias_of_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    create_source(&source);
    let output = dir.path().join("snapshot.db");
    std::fs::hard_link(&source, &output).unwrap();
    let before = std::fs::read(&source).unwrap();
    assert!(snapshot_sqlite(&source, dir.path()).is_err());
    assert!(std::fs::read(&source).unwrap() == before, "source changed");
    assert!(same_file::is_same_file(&source, &output).unwrap());
}

#[cfg(unix)]
#[test]
fn refuses_symlink_destination_before_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    create_source(&source);
    let output = dir.path().join("snapshot.db");
    std::os::unix::fs::symlink(&source, &output).unwrap();
    let before = std::fs::read(&source).unwrap();
    assert!(snapshot_sqlite(&source, dir.path()).is_err());
    assert!(std::fs::read(&source).unwrap() == before, "source changed");
    assert!(output.symlink_metadata().unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn refuses_source_alias_through_symlinked_parent() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("snapshot.db");
    create_source(&source);
    let alias = dir.path().join("parent-alias");
    std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
    let before = std::fs::read(&source).unwrap();
    assert!(snapshot_sqlite(&source, &alias).is_err());
    assert!(std::fs::read(&source).unwrap() == before, "source changed");
}

#[test]
fn successful_snapshot_replaces_previous_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    create_source(&source);
    let output = dir.path().join("snapshot.db");
    std::fs::write(&output, b"previous output").unwrap();
    assert_eq!(snapshot_sqlite(&source, dir.path()).unwrap(), output);
    assert_eq!(botspy::snapshot::count_events(&output).unwrap(), 1);
}

#[test]
fn refuses_destination_with_live_wal_and_preserves_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    create_source(&source);
    let output = dir.path().join("snapshot.db");
    let writer = rusqlite::Connection::open(&output).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer
        .execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES (1), (2);")
        .unwrap();
    let wal = dir.path().join("snapshot.db-wal");
    let shm = dir.path().join("snapshot.db-shm");
    let before = botspy::snapshot::digest_files(&[&output, &wal, &shm]);
    assert!(snapshot_sqlite(&source, dir.path()).is_err());
    assert_eq!(
        botspy::snapshot::digest_files(&[&output, &wal, &shm]),
        before
    );
    assert_eq!(botspy::snapshot::count_events(&output).unwrap(), 2);
}

#[test]
fn refuses_any_destination_sqlite_sidecar_without_removing_it() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        create_source(&source);
        let sidecar = dir.path().join(format!("snapshot.db{suffix}"));
        std::fs::write(&sidecar, b"synthetic sidecar").unwrap();
        assert!(snapshot_sqlite(&source, dir.path()).is_err());
        assert!(!dir.path().join("snapshot.db").exists());
        assert!(std::fs::read(&sidecar).unwrap() == b"synthetic sidecar");
    }
}
