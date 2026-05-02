//! Filesystem layout for a single `blick review` run.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::error::BlickError;

/// Re-point `<runs_root>/latest` at the freshly-completed run. On Unix we
/// use a symlink so other tools can stat-and-follow; elsewhere we fall back
/// to a plain file holding the run id.
pub(super) fn update_latest_pointer(runs_root: &Path, run_id: &str) -> Result<(), BlickError> {
    let latest = runs_root.join("latest");
    match fs::remove_file(&latest) {
        Ok(()) => {}
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::IsADirectory) => {}
        Err(err) => return Err(err.into()),
    }
    match fs::remove_dir_all(&latest) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(run_id, &latest)?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&latest, run_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_targets_the_named_run() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("20990101T000000Z")).unwrap();
        update_latest_pointer(tmp.path(), "20990101T000000Z").unwrap();
        let latest = tmp.path().join("latest");

        #[cfg(unix)]
        {
            let target = std::fs::read_link(&latest).unwrap();
            assert_eq!(target, std::path::PathBuf::from("20990101T000000Z"));
        }
        #[cfg(not(unix))]
        {
            let body = std::fs::read_to_string(&latest).unwrap();
            assert_eq!(body, "20990101T000000Z");
        }
    }

    #[test]
    fn pointer_replaces_an_existing_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        update_latest_pointer(tmp.path(), "first").unwrap();
        update_latest_pointer(tmp.path(), "second").unwrap();
        let latest = tmp.path().join("latest");
        #[cfg(unix)]
        {
            let target = std::fs::read_link(&latest).unwrap();
            assert_eq!(target, std::path::PathBuf::from("second"));
        }
        #[cfg(not(unix))]
        {
            assert_eq!(std::fs::read_to_string(&latest).unwrap(), "second");
        }
    }
}
