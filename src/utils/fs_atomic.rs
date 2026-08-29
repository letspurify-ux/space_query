//! Write a file the user named without destroying what was there.
//!
//! `fs::write` truncates the target before it writes a byte, so a write that
//! fails part way — a full disk, a permission that changed, a network volume
//! that went away — leaves the user's existing file cut short. An export is
//! exactly the case where that matters: the user picks a path in a save panel,
//! often over a file they already have, and the only thing they are told is
//! that the export failed.
//!
//! So the bytes go to a temporary file beside the target and are renamed onto
//! it, which is atomic on every platform this app runs on: the target is either
//! the old file or the whole new one, never half of either.
//!
//! [`crate::utils::config`] and [`crate::utils::logging`] keep their own copies
//! of this dance rather than calling here. They write through a serializer into
//! the open file (not from a finished `String`), and one of them makes the
//! temporary file owner-only before the rename because it holds executed SQL.
//! Folding three different producers into one entry point would mean a writer
//! parameter and a permissions parameter, which is a bigger surface than the
//! one rule they share.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `contents` to `path`, leaving whatever is there untouched if anything
/// goes wrong.
///
/// The temporary file is created in the target's own directory, because a
/// rename is only atomic within one filesystem. It is removed on every failure
/// road, so a failed export leaves no litter beside the file it did not write.
///
/// Two honest costs of replacing rather than truncating, both of them loud
/// rather than silent:
///
/// * it needs to CREATE a file in the target's directory, where a truncating
///   write needed only permission on the file itself. Overwriting a file that
///   sits in a directory the user cannot write to now fails — with the system's
///   own message, and with the old file intact.
/// * a target that is a SYMLINK is replaced by a real file instead of being
///   written through. Following it first would mean resolving the path and then
///   renaming onto the resolved one, which is a different file by the time the
///   rename runs — the check would be worth less than the honesty of not making
///   it.
pub fn write_file_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let tmp_path = temporary_path_for(path);

    let write_result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&tmp_path).map_err(|err| err.to_string())?;
        file.write_all(contents.as_bytes())
            .map_err(|err| err.to_string())?;
        file.flush().map_err(|err| err.to_string())?;
        // The rename below is atomic, but only about which name points at which
        // file: without this the new file's own bytes may still be in flight.
        file.sync_all().map_err(|err| err.to_string())
    })();

    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    // A replacement keeps the permissions of what it replaces. Truncating an
    // existing file never changed its mode, so an export the user had made
    // owner-only would otherwise come back world-readable — a new file's mode
    // is the process umask's to decide, and this file is not new.
    //
    // Best effort: a filesystem that will not carry the mode is not a reason to
    // refuse an export the user asked for.
    if let Ok(existing) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp_path, existing.permissions());
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.to_string());
    }

    Ok(())
}

/// The longest file NAME (not path) a target filesystem is assumed to take.
///
/// 255 bytes is what every filesystem this app runs on allows, and the save
/// panel will not produce a longer one. The temporary name has to fit inside it
/// too: a truncating write only needed the name the user chose, so a temporary
/// name that overflowed would fail an export that used to work.
const MAX_FILE_NAME_BYTES: usize = 255;

/// A name beside `path` that no other write of this app is using.
///
/// The process id and the clock keep two exports of the same file — from two
/// windows, or one started before another finished — off each other's temporary
/// file. A file name the target does not have (`.tmp.<pid>.<nanos>`) keeps it
/// out of the way of anything watching the directory for finished exports.
///
/// The target's own name is only there to make the temporary one recognisable,
/// so it is the part that gives way when the two together would be too long for
/// the filesystem. What must survive whole is the suffix: it is what makes the
/// name unique.
fn temporary_path_for(path: &Path) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let suffix = format!(".tmp.{}.{unique}", std::process::id());
    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".to_string());
    let room = MAX_FILE_NAME_BYTES.saturating_sub(suffix.len());
    let mut stem = stem;
    if stem.len() > room {
        // On a character boundary, so the name stays text.
        let cut = (0..=room)
            .rev()
            .find(|at| stem.is_char_boundary(*at))
            .unwrap_or(0);
        stem.truncate(cut);
    }
    path.with_file_name(format!("{stem}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "space_query_fs_atomic_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_written_file_holds_exactly_what_was_handed_over() {
        let dir = scratch_dir("write");
        let path = dir.join("export.csv");
        write_file_atomically(&path, "A,B\r\n1,2\r\n").expect("writes");
        assert_eq!(
            fs::read_to_string(&path).expect("reads back"),
            "A,B\r\n1,2\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A write that cannot succeed leaves the target exactly as it was and
    /// leaves nothing of its own behind.
    ///
    /// The failure is provoked with a target that is a non-empty directory:
    /// the temporary file is created, and the rename onto it cannot succeed.
    /// What this pins is the CLEANUP — no half-written file under the target's
    /// name, no temporary file left beside it.
    ///
    /// It does not pin the reason this function exists, and cannot: that reason
    /// is a write that fails PART WAY (a full disk, an I/O error), where
    /// `fs::write` has already truncated the user's file and this one has not
    /// touched it. Provoking a partial write needs a filesystem that runs out
    /// mid-write, which a unit test has no portable way to arrange — so the
    /// guarantee lives in the shape of the code (the target is only ever
    /// touched by the rename, after the bytes are all on disk) and this test
    /// guards the road that shape takes on the way to a failure.
    #[test]
    fn a_failed_write_leaves_the_previous_file_alone_and_no_litter_behind() {
        let dir = scratch_dir("failure");
        let target = dir.join("occupied");
        fs::create_dir(&target).expect("a directory where the file should go");
        let marker = target.join("still-here.txt");
        fs::write(&marker, "untouched").expect("marker");

        let error = write_file_atomically(&target, "new contents")
            .expect_err("renaming onto a non-empty directory cannot succeed");
        assert!(!error.is_empty(), "the failure has to say something");

        assert_eq!(
            fs::read_to_string(&marker).expect("the old contents survive"),
            "untouched"
        );
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .expect("scratch dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "a failed write left its temporary file behind: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A name the filesystem accepts stays a name the filesystem accepts.
    ///
    /// The temporary file is the target's name plus a suffix, and a target
    /// already at the length limit would push it past one — failing an export
    /// that a truncating write performed happily.
    #[test]
    fn a_name_at_the_length_limit_still_gets_a_temporary_file() {
        let dir = scratch_dir("long");
        let long_name = format!("{}.csv", "n".repeat(MAX_FILE_NAME_BYTES - 4));
        assert_eq!(long_name.len(), MAX_FILE_NAME_BYTES);
        let path = dir.join(&long_name);

        let temporary = temporary_path_for(&path);
        let temporary_name = temporary
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        assert!(
            temporary_name.len() <= MAX_FILE_NAME_BYTES,
            "the temporary name is {} bytes, past what a filesystem takes",
            temporary_name.len()
        );
        assert!(
            temporary_name.contains(".tmp."),
            "what makes the name unique was truncated away: {temporary_name:?}"
        );
        assert_eq!(
            temporary.parent(),
            path.parent(),
            "same directory, or the rename is not atomic"
        );

        // And the write really runs, under the name the user chose.
        write_file_atomically(&path, "long").expect("writes under a maximal name");
        assert_eq!(fs::read_to_string(&path).expect("reads back"), "long");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Replacing a file keeps the permissions it had.
    ///
    /// `fs::write` truncates in place and never touches the mode; a rename
    /// brings the temporary file's own, which is the umask's answer for a NEW
    /// file. An export the user had made owner-only must not come back
    /// world-readable because this function changed how it writes.
    #[cfg(unix)]
    #[test]
    fn replacing_a_file_keeps_the_mode_it_had() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch_dir("mode");
        let path = dir.join("private.csv");
        fs::write(&path, "old").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("make it private");

        write_file_atomically(&path, "new").expect("writes");

        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the replacement widened the file's permissions to {mode:o}"
        );
        assert_eq!(fs::read_to_string(&path).expect("reads back"), "new");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_file_is_replaced_whole() {
        let dir = scratch_dir("replace");
        let path = dir.join("export.csv");
        fs::write(&path, "old and much longer than the new contents").expect("seed");
        write_file_atomically(&path, "new").expect("writes");
        assert_eq!(fs::read_to_string(&path).expect("reads back"), "new");
        let _ = fs::remove_dir_all(&dir);
    }
}
