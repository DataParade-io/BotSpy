//! Steps for read-only SQLite/WAL source access
//! (spec 03_importers/wal_snapshot.feature).

use crate::steps::BotSpyWorld;
use botspy::snapshot::digest_files;
use botspy::snapshot_sqlite;
use cucumber::{given, then, when};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static UNIQUE_DIR: AtomicUsize = AtomicUsize::new(0);

fn fresh_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "botspy-wal-{}-{}",
        std::process::id(),
        UNIQUE_DIR.fetch_add(1, Ordering::SeqCst)
    ))
}

#[given(
    regex = r#"a SQLite source in WAL mode at "([^"]+)" with ([0-9]+) rows and a live writer appending rows"#
)]
fn wal_source_with_writer(world: &mut BotSpyWorld, _given_path: String, rows: u64) {
    let base = fresh_dir();
    std::fs::create_dir_all(&base).expect("create fixture dir");
    let db = base.join("store.db");
    let writer = rusqlite::Connection::open(&db).expect("open fixture source db");
    writer
        .pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL mode");
    writer
        .execute_batch("CREATE TABLE events (id INTEGER PRIMARY KEY, note TEXT NOT NULL);")
        .expect("create events table");
    for i in 1..=rows {
        writer
            .execute(
                "INSERT INTO events (id, note) VALUES (?1, ?2)",
                rusqlite::params![i as i64, format!("row {i}")],
            )
            .expect("insert fixture row");
    }
    world.sqlite_db = Some(db);
    world.sqlite_writer = Some(writer);
    world.sqlite_rows_at_snapshot = rows;
}

#[when(regex = r#"I read the source through a snapshot copy"#)]
fn read_through_snapshot(world: &mut BotSpyWorld) {
    let db = world.sqlite_db.clone().expect("no fixture source db");
    let wal = wal_path(&db);
    world.sqlite_digest_before = Some(digest_files(&[&db, &wal]));
    let snapshot = snapshot_sqlite(&db, &fresh_dir()).expect("snapshot the source");
    world.sqlite_digest_after = Some(digest_files(&[&db, &wal]));
    world.snapshot_rows =
        Some(botspy::snapshot::count_events(&snapshot).expect("count snapshot rows"));
}

fn wal_path(db: &std::path::Path) -> PathBuf {
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    PathBuf::from(wal)
}

#[then(regex = r#"the snapshot yields a consistent read of the rows present at snapshot time"#)]
fn snapshot_rows_consistent(world: &mut BotSpyWorld) {
    let rows = world.snapshot_rows.expect("no snapshot was read");
    assert_eq!(
        rows, world.sqlite_rows_at_snapshot,
        "the snapshot does not reflect the rows present at snapshot time"
    );
}

#[then(regex = r#"the source database file and its WAL file were never mutated"#)]
fn source_unmutated(world: &mut BotSpyWorld) {
    let before = world.sqlite_digest_before.as_ref().expect("digest before");
    let after = world.sqlite_digest_after.as_ref().expect("digest after");
    assert_eq!(before, after, "reading the source mutated it");
}

#[then(regex = r#"the live writer kept appending without interference"#)]
fn writer_keeps_appending(world: &mut BotSpyWorld) {
    let writer = world.sqlite_writer.as_ref().expect("no live writer");
    writer
        .execute_batch(
            "INSERT INTO events (id, note) VALUES (101, 'appended during read');
             INSERT INTO events (id, note) VALUES (102, 'appended during read');",
        )
        .expect("the live writer could not append while we read");
    assert_eq!(
        world.snapshot_rows,
        Some(world.sqlite_rows_at_snapshot),
        "the snapshot changed after the writer appended"
    );
}

#[given(expr = "a SQLite source named \"snapshot.db\" containing a synthetic event")]
fn snapshot_named_source(world: &mut BotSpyWorld) {
    let base = fresh_dir();
    std::fs::create_dir_all(&base).expect("create source directory");
    let db = base.join("snapshot.db");
    let conn = rusqlite::Connection::open(&db).expect("create source");
    conn.execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES (1);")
        .expect("create synthetic event");
    drop(conn);
    world.sqlite_digest_before = Some(digest_files(&[&db]));
    world.sqlite_db = Some(db);
}

#[when(expr = "I request a snapshot in the source directory")]
fn snapshot_onto_source(world: &mut BotSpyWorld) {
    let db = world.sqlite_db.as_ref().expect("source");
    world.cli_run = Some(botspy::cli::run_from([
        "botspy",
        "snapshot",
        db.to_str().expect("source path"),
        "--out",
        db.parent().unwrap().to_str().expect("source directory"),
    ]));
    world.sqlite_digest_after = Some(digest_files(&[db]));
}

#[then(expr = "the snapshot request is rejected")]
fn snapshot_rejected(world: &mut BotSpyWorld) {
    assert_ne!(world.cli_run.as_ref().expect("snapshot result").code, 0);
}

#[given(expr = "an invalid SQLite source and an existing snapshot containing a synthetic event")]
fn invalid_source_with_previous_snapshot(world: &mut BotSpyWorld) {
    snapshot_named_source(world);
    let output = world.sqlite_db.take().expect("previous snapshot");
    let source = output.with_file_name("invalid.db");
    std::fs::write(&source, b"not a SQLite database").expect("invalid source");
    world.cli_out_dir = Some(output);
    world.sqlite_db = Some(source);
}

#[when(expr = "I request a snapshot beside the invalid source")]
fn snapshot_invalid_source(world: &mut BotSpyWorld) {
    snapshot_onto_source(world);
    world.sqlite_digest_after = Some(digest_files(&[world
        .cli_out_dir
        .as_ref()
        .expect("previous snapshot")]));
}

#[then(expr = "the previous snapshot is unchanged")]
fn previous_snapshot_unchanged(world: &mut BotSpyWorld) {
    source_unmutated(world);
}

#[given(expr = "a SQLite source and an existing snapshot with live WAL data")]
fn source_with_live_snapshot(world: &mut BotSpyWorld) {
    snapshot_named_source(world);
    let output = world.sqlite_db.take().expect("previous snapshot");
    let writer = rusqlite::Connection::open(&output).expect("open existing snapshot");
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer.execute("INSERT INTO events VALUES (2)", []).unwrap();
    let source = output.with_file_name("source.db");
    let source_conn = rusqlite::Connection::open(&source).unwrap();
    source_conn
        .execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES (7);")
        .unwrap();
    drop(source_conn);
    world.sqlite_digest_before = Some(snapshot_and_sidecars_digest(&output));
    world.sqlite_writer = Some(writer);
    world.cli_out_dir = Some(output);
    world.sqlite_db = Some(source);
}

#[when(expr = "I request a snapshot beside that live destination")]
fn snapshot_live_destination(world: &mut BotSpyWorld) {
    snapshot_onto_source(world);
    world.sqlite_digest_after = Some(snapshot_and_sidecars_digest(
        world.cli_out_dir.as_ref().expect("previous snapshot"),
    ));
}

fn snapshot_and_sidecars_digest(db: &std::path::Path) -> String {
    let wal = wal_path(db);
    let mut shm = db.as_os_str().to_owned();
    shm.push("-shm");
    digest_files(&[db, &wal, &PathBuf::from(shm)])
}

#[when(expr = "I inspect the sources using an isolated temporary directory")]
fn inspect_with_isolated_temp(world: &mut BotSpyWorld) {
    let temp_root = fresh_dir();
    std::fs::create_dir_all(&temp_root).expect("create temporary root");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_botspy"))
        .args(["sources", "--home"])
        .arg(world.cli_home.as_ref().expect("fixture home"))
        .env("TMPDIR", &temp_root)
        .env("TMP", &temp_root)
        .env("TEMP", &temp_root)
        .output()
        .expect("inspect synthetic sources");
    world.cli_run = Some(botspy::cli::RunOutcome {
        code: result.status.code().unwrap_or(1),
        stdout: String::from_utf8(result.stdout).expect("UTF-8 output"),
        stderr: String::from_utf8(result.stderr).expect("UTF-8 errors"),
    });
    world.cli_out_dir = Some(temp_root);
}

#[then(expr = "no automatic snapshot copies remain in the temporary directory")]
fn snapshots_cleaned(world: &mut BotSpyWorld) {
    let temp_root = world.cli_out_dir.as_ref().expect("temporary root");
    assert_eq!(
        std::fs::read_dir(temp_root)
            .expect("read temporary root")
            .count(),
        0,
        "automatic snapshot copies remain after the command exits"
    );
    std::fs::remove_dir(temp_root).expect("remove empty temporary root");
}
