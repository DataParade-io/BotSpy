//! Safe reading of SQLite sources, including live WAL databases.
//!
//! Agents own their databases; BotSpy only ever reads them. A SQLite
//! source in WAL mode — a live app writing while we read — is read
//! through a snapshot copy taken over a read-only connection. SQLite can
//! still create or touch its read-coordination sidecars and acquire locks.
//! Automatic copies live in private temporary directories until their
//! reader closes; explicitly requested snapshots persist at their destination.

use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use std::fmt;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Flags for opening a source database strictly read-only.
const READ_ONLY: OpenFlags = OpenFlags::SQLITE_OPEN_READ_ONLY;

/// Opening or snapshotting a source database failed.
#[derive(Debug)]
pub enum SnapshotError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(err) => write!(f, "sqlite source error: {err}"),
            Self::Io(err) => write!(f, "snapshot io error: {err}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Copy a SQLite database into `snapshot_dir/snapshot.db` through a
/// read-only connection to `source`, using the online backup API.
/// The destination cannot alias the source, be a symlink, or have SQLite
/// sidecars. A failed backup preserves any previous destination; a
/// successful one replaces it atomically with a private file (owner-only
/// permissions on Unix).
pub fn snapshot_sqlite(source: &Path, snapshot_dir: &Path) -> Result<PathBuf, SnapshotError> {
    let source = source.canonicalize().map_err(SnapshotError::Io)?;
    let snapshot_path = snapshot_dir.join("snapshot.db");
    validate_destination(&source, &snapshot_path)?;
    let source_conn =
        Connection::open_with_flags(&source, READ_ONLY).map_err(SnapshotError::Sqlite)?;
    std::fs::create_dir_all(snapshot_dir).map_err(SnapshotError::Io)?;
    // The securely created file stays owned by TempPath until the backup
    // succeeds. Closing it before SQLite opens it also works on Windows.
    let staged = tempfile::NamedTempFile::new_in(snapshot_dir)
        .map_err(SnapshotError::Io)?
        .into_temp_path();
    {
        let mut snapshot_conn = Connection::open(&staged).map_err(SnapshotError::Sqlite)?;
        {
            let backup =
                Backup::new(&source_conn, &mut snapshot_conn).map_err(SnapshotError::Sqlite)?;
            backup
                .run_to_completion(64, Duration::from_millis(5), None)
                .map_err(SnapshotError::Sqlite)?;
        }
        // Persist a self-contained snapshot, not a database whose pages
        // depend on sidecars named after the staging file.
        snapshot_conn
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(SnapshotError::Sqlite)?;
    }
    // Recheck after the backup as well; persist replaces the directory
    // entry rather than opening/truncating a possibly changed destination.
    validate_destination(&source, &snapshot_path)?;
    staged
        .persist(&snapshot_path)
        .map_err(|err| SnapshotError::Io(err.error))?;
    Ok(snapshot_path)
}

fn validate_destination(source: &Path, destination: &Path) -> Result<(), SnapshotError> {
    // Replacing only the main file of a live or uncheckpointed database
    // would let its old WAL or journal be replayed into the new snapshot.
    // Never delete those sidecars: they may contain the only copy of data.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = destination.as_os_str().to_owned();
        sidecar.push(suffix);
        match std::fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(_) => {
                return Err(SnapshotError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "snapshot destination has SQLite sidecars; choose another output directory",
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(SnapshotError::Io(err)),
        }
    }
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || same_file::is_same_file(source, destination).map_err(SnapshotError::Io)?
            {
                return Err(SnapshotError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "snapshot destination must be a regular file distinct from the source",
                )));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(SnapshotError::Io(err)),
    }
    Ok(())
}

/// Owns an automatic snapshot for exactly as long as its reader. Field
/// order matters: SQLite closes before TempDir removes the directory.
/// Abrupt process termination can leave the private directory behind.
#[derive(Debug)]
pub(crate) struct SnapshotConnection {
    connection: Connection,
    _directory: tempfile::TempDir,
}

impl SnapshotConnection {
    pub(crate) fn open(source: &Path) -> Result<Self, SnapshotError> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("botspy-snapshot-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = builder.tempdir().map_err(SnapshotError::Io)?;
        Self::in_directory(source, directory)
    }

    fn in_directory(source: &Path, directory: tempfile::TempDir) -> Result<Self, SnapshotError> {
        let path = snapshot_sqlite(source, directory.path())?;
        let connection =
            Connection::open_with_flags(path, READ_ONLY).map_err(SnapshotError::Sqlite)?;
        Ok(Self {
            connection,
            _directory: directory,
        })
    }
}

impl Deref for SnapshotConnection {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.connection
    }
}

/// Content digest over the given files, in order. Missing files digest
/// distinctly from empty ones. Used to prove a source was not mutated
/// by a read.
pub fn digest_files(paths: &[&Path]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for path in paths {
        hasher.update(path.to_string_lossy().as_bytes());
        match std::fs::read(path) {
            Ok(bytes) => {
                hasher.update([1]);
                hasher.update(&bytes);
            }
            Err(_) => hasher.update([0]),
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Open a snapshot (or any of our own copies) read-only and count the
/// rows of the `events` table the fixture sources use.
pub fn count_events(db: &Path) -> Result<u64, SnapshotError> {
    let conn = Connection::open_with_flags(db, READ_ONLY).map_err(SnapshotError::Sqlite)?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .map_err(SnapshotError::Sqlite)?;
    Ok(count.max(0) as u64)
}

/// Count the rows of a snapshot across the table names the sources use:
/// `events` for the fixture sources, `cursorDiskKV` for Cursor's KV
/// store. The first table that exists wins.
pub fn count_rows(db: &Path) -> Result<u64, SnapshotError> {
    let conn = Connection::open_with_flags(db, READ_ONLY).map_err(SnapshotError::Sqlite)?;
    for table in ["events", "cursorDiskKV"] {
        let exists = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get::<_, i64>(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                err => Err(err),
            })
            .map_err(SnapshotError::Sqlite)?;
        if exists.is_none() {
            continue;
        }
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(SnapshotError::Sqlite)?;
        return Ok(count.max(0) as u64);
    }
    Err(SnapshotError::Sqlite(rusqlite::Error::InvalidQuery))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_at(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE events(id INTEGER); INSERT INTO events VALUES (1);")
            .unwrap();
    }

    #[test]
    fn automatic_copy_lives_until_its_reader_closes() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.db");
        source_at(&source);
        let snapshot = SnapshotConnection::open(&source).unwrap();
        let directory = snapshot._directory.path().to_path_buf();
        let count: i64 = snapshot
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert!(directory.join("snapshot.db").exists());
        drop(snapshot);
        assert!(!directory.exists(), "reader close must remove the copy");
    }

    #[test]
    fn failed_automatic_copy_removes_its_private_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("invalid.db");
        std::fs::write(&source, b"not a SQLite database").unwrap();
        let directory = tempfile::tempdir_in(fixture.path()).unwrap();
        let path = directory.path().to_path_buf();
        assert!(SnapshotConnection::in_directory(&source, directory).is_err());
        assert!(!path.exists(), "failure must remove the incomplete copy");
    }

    #[cfg(unix)]
    #[test]
    fn automatic_copy_is_only_accessible_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.db");
        source_at(&source);
        let snapshot = SnapshotConnection::open(&source).unwrap();
        let directory = snapshot._directory.path();
        let directory_mode = directory.metadata().unwrap().permissions().mode();
        let file_mode = directory
            .join("snapshot.db")
            .metadata()
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(directory_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);
    }
}
