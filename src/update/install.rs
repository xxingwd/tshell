//! Replace only at startup, before any PTY or GUI is created. Retain one rollback copy.
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path};

pub fn replace(
    executable: &Path,
    candidate: &Path,
    launch: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let backup = executable.with_extension("previous.exe");
    let temporary = executable.with_extension("next.exe");
    ensure!(!backup.exists() || backup.is_file(), "Invalid backup path");
    if backup.exists() {
        fs::remove_file(&backup).context("Cannot remove previous update backup")?;
    }
    // Copy before moving the working executable: disk-full/access errors leave it intact.
    fs::copy(candidate, &temporary).context("Cannot stage update beside the application")?;
    fs::OpenOptions::new()
        .write(true)
        .open(&temporary)?
        .sync_all()?;
    fs::rename(executable, &backup).context("Cannot move current application to backup")?;
    if let Err(error) = fs::rename(&temporary, executable) {
        fs::rename(&backup, executable)
            .context("Installation failed and backup could not be restored")?;
        return Err(error.into());
    }
    if let Err(error) = launch() {
        fs::remove_file(executable)?;
        fs::rename(&backup, executable)
            .context("New application failed to start and backup could not be restored")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_keeps_backup_and_rolls_back_launch_failure() {
        let directory = tempfile::tempdir().unwrap();
        let exe = directory.path().join("TShell.exe");
        let candidate = directory.path().join("download.exe");
        fs::write(&exe, b"old").unwrap();
        fs::write(&candidate, b"new").unwrap();
        assert!(replace(&exe, &candidate, || anyhow::bail!("launch failed")).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        replace(&exe, &candidate, || Ok(())).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(
            fs::read(exe.with_extension("previous.exe")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn update_missing_download_does_not_remove_working_app() {
        let directory = tempfile::tempdir().unwrap();
        let exe = directory.path().join("TShell.exe");
        fs::write(&exe, b"old").unwrap();
        assert!(replace(&exe, &directory.path().join("missing.exe"), || Ok(())).is_err());
        assert_eq!(fs::read(exe).unwrap(), b"old");
    }

    #[test]
    fn update_running_executable_can_be_replaced() {
        use std::{os::windows::process::CommandExt, process::Command};
        const TEST: &str = "update::install::tests::update_running_executable_can_be_replaced";
        const MODE: &str = "TSHELL_INSTALL_TEST_MODE";
        let current = std::env::current_exe().unwrap();
        if let Ok(mode) = std::env::var(MODE) {
            if mode == "replacement" {
                return;
            }
            let candidate = current.with_file_name("candidate.exe");
            replace(&current, &candidate, || {
                let status = Command::new(&current)
                    .args(["--exact", TEST])
                    .env(MODE, "replacement")
                    .creation_flags(0x08000000)
                    .status()?;
                ensure!(status.success(), "Replacement test process failed");
                Ok(())
            })
            .unwrap();
            assert!(current.with_extension("previous.exe").is_file());
            return;
        }
        // Exercise Windows executable sharing/rename semantics in a disposable process,
        // never the user's application or the original test executable.
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("tshell.exe");
        fs::copy(&current, &executable).unwrap();
        fs::copy(&current, directory.path().join("candidate.exe")).unwrap();
        let status = Command::new(&executable)
            .args(["--exact", TEST])
            .env(MODE, "original")
            .creation_flags(0x08000000)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
