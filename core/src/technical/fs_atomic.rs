//! The atomic write of the data store (plan item 2.5).
//!
//! `doc/architecture/data-files.md` ("Every write is atomic: write a temporary
//! file, rename it into place") and `doc/architecture/architecture.md` ("Writes
//! are atomic (write temporary file, rename)") are implemented by [`write_atomic`]:
//! the new text goes to a temporary file in the **same directory** as the target
//! (a `rename` is only atomic inside one directory, so the temporary is never a
//! `/tmp` file), is renamed into place, and the version it replaced is kept as
//! `<path>.bak` (decision 7 — `.gitignore` already lists `*.bak`). A crash
//! therefore never leaves a half-written file: the target holds either the old
//! content or the new one.
//!
//! The three steps are split (`write_temp`, `back_up`, `commit`) so the crash
//! test of the plan can stop between the temporary write and the rename, exactly
//! as a killed process would. Every failure is a typed [`WriteError`] carrying
//! the path, the OS reason kind and the OS message; nothing panics. I/O belongs
//! here on the technical side — the purity test in [`crate::technical`] forbids
//! `std::fs` under `core/src/domain`.

/// Whether [`write_atomic`] kept a previous version as `<path>.bak`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackupOutcome {
    /// The target held content before the write; those exact bytes are now in
    /// `<path>.bak`.
    PreviousKept,
    /// The target did not exist or was empty, so there was nothing to keep: no
    /// `.bak` was written and an existing one was left untouched.
    NothingToKeep,
}

/// Why an atomic write failed: the path that could not be handled, the OS
/// reason kind, and the OS message for the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteError {
    /// The target's directory could not be created or reached.
    DirectoryRejected(std::path::PathBuf, std::io::ErrorKind, String),
    /// The target exists but could not be read in order to back it up — e.g.
    /// the target path is a directory.
    PreviousUnreadable(std::path::PathBuf, std::io::ErrorKind, String),
    /// The temporary sibling could not be written.
    TempWriteFailed(std::path::PathBuf, std::io::ErrorKind, String),
    /// The previous version could not be written to `<path>.bak`.
    BackupWriteFailed(std::path::PathBuf, std::io::ErrorKind, String),
    /// The temporary file could not be renamed into place: the target still
    /// holds its old content and the temporary sibling stays on disk.
    RenameFailed(std::path::PathBuf, std::io::ErrorKind, String),
}

/// The temporary sibling of `path`: `<path>.phiptmp`, in the same directory.
fn temp_path(path: &std::path::Path) -> std::path::PathBuf {
    let name = (path.to_string_lossy() + ".phiptmp").to_string();
    return std::path::PathBuf::from(name);
}

/// The `<path>.bak` sibling of `path` (decision 7).
fn backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let name = (path.to_string_lossy() + ".bak").to_string();
    return std::path::PathBuf::from(name);
}

/// Create the target's directory when it is missing, so a store file may be
/// written before its directory exists.
fn ensure_directory(path: &std::path::Path) -> Result::<(), WriteError> {
    match path.parent() {
        None => { return Ok(()); }
        Some(dir) => {
            if dir.is_empty() { return Ok(()); }
            match std::fs::create_dir_all(dir) {
                Err(reason) => {
                    let kind = reason.kind();
                    let message = format!("{reason}");
                    return Err(WriteError::DirectoryRejected(dir.to_path_buf(), kind, message));
                }
                Ok(_) => { return Ok(()); }
            }
        }
    }
}

/// The bytes the target holds now, empty when it does not exist or is empty.
/// A read failure that is not "not found" (the target is a directory, …) is
/// [`WriteError::PreviousUnreadable`].
fn previous_content(path: &std::path::Path) -> Result::<Vec::<u8>, WriteError> {
    match std::fs::read(path) {
        Err(reason) => {
            let kind = reason.kind();
            if kind == std::io::ErrorKind::NotFound {
                return Ok(Vec::<u8>::new());
            }
            let message = format!("{reason}");
            return Err(WriteError::PreviousUnreadable(path.to_path_buf(), kind, message));
        }
        Ok(bytes) => { return Ok(bytes); }
    }
}

/// Step 1: write `content` to the temporary sibling, returning its path.
pub fn write_temp(path: &std::path::Path, content: &str) -> Result::<std::path::PathBuf, WriteError> {
    let temp = temp_path(path);
    let bytes = content.as_bytes();
    match std::fs::write(&temp, &bytes) {
        Err(reason) => {
            let kind = reason.kind();
            let message = format!("{reason}");
            return Err(WriteError::TempWriteFailed(temp, kind, message));
        }
        Ok(_) => { return Ok(temp); }
    }
}

/// Step 2: keep the previous version as `<path>.bak`. Empty `previous` bytes
/// mean there is nothing to keep, so no `.bak` is written and an existing one
/// is left untouched.
pub fn back_up(path: &std::path::Path, previous: &Vec::<u8>) -> Result::<BackupOutcome, WriteError> {
    if previous.len() == 0 {
        return Ok(BackupOutcome::NothingToKeep);
    }
    let backup = backup_path(path);
    match std::fs::write(&backup, &previous) {
        Err(reason) => {
            let kind = reason.kind();
            let message = format!("{reason}");
            return Err(WriteError::BackupWriteFailed(backup, kind, message));
        }
        Ok(_) => { return Ok(BackupOutcome::PreviousKept); }
    }
}

/// Step 3: rename the temporary file into place — the atomic swap.
pub fn commit(temp: &std::path::PathBuf, path: &std::path::Path) -> Result::<(), WriteError> {
    match std::fs::rename(temp, path) {
        Err(reason) => {
            let kind = reason.kind();
            let message = format!("{reason}");
            return Err(WriteError::RenameFailed(path.to_path_buf(), kind, message));
        }
        Ok(_) => { return Ok(()); }
    }
}

/// Write `content` to `path` atomically: temporary file in the same directory,
/// previous version kept as `<path>.bak`, target directory created when missing.
pub fn write_atomic(path: &std::path::Path, content: &str) -> Result::<BackupOutcome, WriteError> {
    match ensure_directory(path) {
        Err(reason) => { return Err(reason); }
        Ok(_) => { }
    }
    let previous = previous_content(path);
    match previous {
        Err(reason) => { return Err(reason); }
        Ok(previous_bytes) => {
            match write_temp(path, content) {
                Err(reason) => { return Err(reason); }
                Ok(temp) => {
                    match back_up(path, &previous_bytes) {
                        Err(reason) => { return Err(reason); }
                        Ok(outcome) => {
                            match commit(&temp, path) {
                                Err(reason) => { return Err(reason); }
                                Ok(_) => { return Ok(outcome); }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The scratch directory of one test, below the test's working directory; the
/// test removes it with [`remove_scratch`] before it finishes.
#[cfg(test)]
fn make_scratch(name: &str) -> std::path::PathBuf {
    let scratch = std::path::PathBuf::from("test-scratch").join(name);
    match std::fs::create_dir_all(&scratch) {
        Err(reason) => { panic!("cannot create the scratch directory of a fs_atomic test: {reason}"); }
        Ok(_) => { }
    }
    return scratch;
}

/// Remove `dir` and everything below it, ignoring "already gone".
#[cfg(test)]
fn remove_scratch(dir: std::path::PathBuf) {
    let mut pending = Vec::<std::path::PathBuf>::new();
    let mut visited = Vec::<std::path::PathBuf>::new();
    pending.push(dir);
    while !pending.is_empty() {
        let current = pending.pop().unwrap();
        visited.push(current.to_path_buf());
        match std::fs::read_dir(&current) {
            Err(_) => { }
            Ok(entries) => {
                for entry in entries {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        pending.push(path.to_path_buf());
                        continue;
                    }
                    match std::fs::remove_file(&path) {
                        Err(_) => { }
                        Ok(_) => { }
                    }
                }
            }
        }
    }
    // Deepest first: a directory can only be removed once it is empty.
    let mut index = visited.len();
    while index > 0 {
        index = index - 1;
        match std::fs::remove_dir(&visited[index]) {
            Err(_) => { }
            Ok(_) => { }
        }
    }
    // The shared parent of all scratch directories: gone once the last test
    // finished, still in use while others run, so a failure is ignored.
    match std::fs::remove_dir(std::path::PathBuf::from("test-scratch")) {
        Err(_) => { }
        Ok(_) => { }
    }
}

/// The bytes of `path`, or an empty `Vec` when it does not exist.
#[cfg(test)]
fn scratch_bytes(path: &std::path::PathBuf) -> Vec::<u8> {
    match std::fs::read(&path) {
        Err(_) => { return Vec::<u8>::new(); }
        Ok(bytes) => { return bytes; }
    }
}

/// Whether the directory of `path` exists.
#[cfg(test)]
fn scratch_is_dir(path: &std::path::PathBuf) -> bool {
    match path.parent() {
        None => { return false; }
        Some(dir) => { return dir.is_dir(); }
    }
}

/// Whether `path` exists as a readable file.
#[cfg(test)]
fn scratch_exists(path: &std::path::PathBuf) -> bool {
    match std::fs::read(&path) {
        Err(_) => { return false; }
        Ok(_) => { return true; }
    }
}

/// Write `content` to `path`, ignoring failures (test setup only).
#[cfg(test)]
fn scratch_write(path: &std::path::PathBuf, content: &str) {
    let bytes = content.as_bytes();
    match std::fs::write(&path, &bytes) {
        Err(_) => { }
        Ok(_) => { }
    }
}

#[test]
fn crash_after_the_temp_write_but_before_the_rename_leaves_the_original_intact() {
    let scratch = make_scratch("crash-before-rename");
    let target = scratch.join("transactions.csv");
    let original = "39945b2fed;2024-01-03;-3999;EUR;REWE MARKT 12;REWE MARKET;bank_a.pdf\n";
    scratch_write(&target, original);

    // The process is killed here: step 1 ran, `commit` never did.
    let temp = write_temp(&target, "half a new file").unwrap();
    assert!(scratch_exists(&temp));

    // The original file is still there, byte for byte, and unmodified.
    assert!(scratch_bytes(&target) == original.as_bytes());
    // No backup was taken and no temporary content leaked into the target.
    assert!(!scratch_exists(&backup_path(&target)));
    remove_scratch(std::path::PathBuf::from("test-scratch").join("crash-before-rename"));
}

#[test]
fn the_bak_holds_the_exact_previous_content_after_a_successful_write() {
    let scratch = make_scratch("bak-content");
    let target = scratch.join("annotations.csv");
    // Content that a byte copy must not touch: `;`, quotes, a non-ASCII
    // character, and no trailing line ending.
    let original = "a1;\"quoted; note\"\nZürich;2024-01-03\nno trailing newline";
    scratch_write(&target, original);

    let outcome = write_atomic(&target, "new text\n").unwrap();
    assert!(outcome == BackupOutcome::PreviousKept);
    assert!(scratch_bytes(&target) == "new text\n".as_bytes());
    assert!(scratch_bytes(&backup_path(&target)) == original.as_bytes());

    // A second write rotates: `.bak` now holds what the previous write put in
    // place, and the version before that is gone (only one generation kept).
    let second = write_atomic(&target, "newer text\n").unwrap();
    assert!(second == BackupOutcome::PreviousKept);
    assert!(scratch_bytes(&target) == "newer text\n".as_bytes());
    assert!(scratch_bytes(&backup_path(&target)) == "new text\n".as_bytes());
    remove_scratch(std::path::PathBuf::from("test-scratch").join("bak-content"));
}

#[test]
fn write_creates_a_missing_target_directory_and_keeps_nothing_to_back_up() {
    let scratch = make_scratch("missing-dir");
    let target = scratch.join("state").join("deep").join("markers.csv");
    // The directory does not exist yet; the write must create it.
    assert!(!scratch_is_dir(&target));
    let outcome = write_atomic(&target, "bank_a.pdf;0\n").unwrap();
    assert!(outcome == BackupOutcome::NothingToKeep);
    assert!(scratch_is_dir(&target));
    assert!(scratch_bytes(&target) == "bank_a.pdf;0\n".as_bytes());
    // A target that never existed has nothing to keep: no `.bak` is created.
    assert!(!scratch_exists(&backup_path(&target)));
    remove_scratch(std::path::PathBuf::from("test-scratch").join("missing-dir"));
}

#[test]
fn an_empty_target_loses_nothing_and_never_touches_an_existing_bak() {
    let scratch = make_scratch("empty-target");
    let target = scratch.join("categories.csv");
    let backup = backup_path(&target);

    // An empty file: the new content lands in full, no empty `.bak` is written.
    scratch_write(&target, "");
    let outcome = write_atomic(&target, "finance;Finances\n").unwrap();
    assert!(outcome == BackupOutcome::NothingToKeep);
    assert!(scratch_bytes(&target) == "finance;Finances\n".as_bytes());
    assert!(!scratch_exists(&backup));

    // Now the target holds content, so a write keeps it in `.bak`…
    let rotated = write_atomic(&target, "finance;Finances;renamed\n").unwrap();
    assert!(rotated == BackupOutcome::PreviousKept);
    assert!(scratch_bytes(&backup) == "finance;Finances\n".as_bytes());

    // …and truncating the target back to empty must NOT overwrite that `.bak`
    // with nothing: the previous version survives, no data is lost.
    scratch_write(&target, "");
    let emptied = write_atomic(&target, "finance;Finances;renamed;again\n").unwrap();
    assert!(emptied == BackupOutcome::NothingToKeep);
    assert!(scratch_bytes(&backup) == "finance;Finances\n".as_bytes());
    assert!(scratch_bytes(&target) == "finance;Finances;renamed;again\n".as_bytes());
    remove_scratch(std::path::PathBuf::from("test-scratch").join("empty-target"));
}

#[test]
fn a_failing_step_is_a_typed_error_and_the_old_file_survives() {
    let scratch = make_scratch("typed-errors");

    // The target path is a directory: reading it to back it up fails, and the
    // failure is typed, not a panic.
    let directory = scratch.join("rules");
    match std::fs::create_dir_all(&directory) {
        Err(_) => { panic!("cannot create the scratch directory rules: cannot proceed"); }
        Ok(_) => { }
    }
    match write_atomic(&directory, "90-learned\n") {
        Err(reason) => {
            match reason {
                WriteError::PreviousUnreadable(path, _, _) => { assert!(path == directory); }
                _ => { assert!(false); }
            }
        }
        Ok(_) => { assert!(false); }
    }
    // The directory and its content are untouched.
    assert!(directory.is_dir());

    // A parent that is a plain file: the directory cannot be created.
    let blocked = scratch.join("blocked-by-file").join("inner").join("rules.txt");
    scratch_write(&scratch.join("blocked-by-file"), "not a directory\n");
    match write_atomic(&blocked, "text\n") {
        Err(reason) => {
            match reason {
                WriteError::DirectoryRejected(path, _, _) => { assert!(path == scratch.join("blocked-by-file").join("inner")); }
                _ => { assert!(false); }
            }
        }
        Ok(_) => { assert!(false); }
    }
    assert!(scratch_bytes(&scratch.join("blocked-by-file")) == "not a directory\n".as_bytes());
    assert!(!scratch_exists(&blocked));
    remove_scratch(std::path::PathBuf::from("test-scratch").join("typed-errors"));
}

#[test]
fn a_rename_that_cannot_happen_keeps_the_old_content_and_reports_it() {
    let scratch = make_scratch("rename-fails");
    let target = scratch.join("overrides.csv");
    scratch_write(&target, "old content\n");
    // Steps 1 and 2 run, step 3 is pointed at a directory that cannot be
    // replaced by a rename — the old content is still on the target.
    let temp = write_temp(&target, "new content\n").unwrap();
    let previous = scratch_bytes(&target);
    assert!(back_up(&target, &previous).unwrap() == BackupOutcome::PreviousKept);
    let directory = scratch.join("cannot-be-replaced");
    match std::fs::create_dir_all(&directory) {
        Err(_) => { panic!("cannot create the scratch directory: cannot proceed"); }
        Ok(_) => { }
    }
    match commit(&temp, &directory) {
        Err(reason) => {
            match reason {
                WriteError::RenameFailed(path, _, _) => { assert!(path == directory); }
                _ => { assert!(false); }
            }
        }
        Ok(_) => { assert!(false); }
    }
    assert!(scratch_bytes(&target) == "old content\n".as_bytes());
    assert!(scratch_bytes(&backup_path(&target)) == "old content\n".as_bytes());
    remove_scratch(std::path::PathBuf::from("test-scratch").join("rename-fails"));
}
