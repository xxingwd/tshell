//! Use a pinned modern ConPTY: the inbox Windows host drops OSC colour queries.
use anyhow::{Context, Result};
use libloading::Library;
use std::{fs, path::Path, sync::OnceLock};

pub fn prepare() -> Result<()> {
    static READY: OnceLock<Result<Library, String>> = OnceLock::new();
    READY
        .get_or_init(|| install().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
        .map(|_| ())
}

fn install() -> Result<Library> {
    let directory = dirs::cache_dir()
        .context(crate::t!("win.runtime_directory"))?
        .join("tshell")
        .join("conpty-1.24.260710001-x64");
    fs::create_dir_all(&directory)?;
    install_file(
        &directory,
        "OpenConsole.exe",
        include_bytes!("../vendor/conpty/x64/OpenConsole.exe"),
    )?;
    install_file(
        &directory,
        "conpty.dll",
        include_bytes!("../vendor/conpty/x64/conpty.dll"),
    )?;
    let path = directory.join("conpty.dll");
    // portable-pty loads conpty.dll by name; Windows reuses this module after
    // it has been loaded by absolute path.
    let library = unsafe { Library::new(&path) }.context(crate::t!("win.runtime_load"))?;
    for symbol in [
        b"CreatePseudoConsole".as_slice(),
        b"ResizePseudoConsole",
        b"ClosePseudoConsole",
    ] {
        unsafe { library.get::<unsafe extern "system" fn()>(symbol) }?;
    }
    Ok(library)
}

fn install_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let path = directory.join(name);
    if fs::read(&path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let temporary = directory.join(format!("{name}.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, &path) {
        // Another TShell process may have installed these same pinned bytes.
        let _ = fs::remove_file(&temporary);
        if !fs::read(&path).is_ok_and(|current| current == bytes) {
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_pty_uses_pinned_conpty() -> Result<()> {
        prepare()?;
        let path = dirs::cache_dir()
            .context(crate::t!("win.runtime_directory"))?
            .join("tshell/conpty-1.24.260710001-x64/conpty.dll");
        let pinned = unsafe { Library::new(path) }?;
        let by_name = unsafe { Library::new("conpty.dll") }?;
        for symbol in [
            b"CreatePseudoConsole".as_slice(),
            b"ResizePseudoConsole",
            b"ClosePseudoConsole",
        ] {
            let expected = unsafe { pinned.get::<unsafe extern "system" fn()>(symbol) }?;
            let actual = unsafe { by_name.get::<unsafe extern "system" fn()>(symbol) }?;
            assert_eq!(*actual as usize, *expected as usize);
        }
        Ok(())
    }
}
