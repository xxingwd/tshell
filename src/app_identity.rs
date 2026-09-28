//! Windows toast identity for an unpackaged, per-user application.
use std::sync::OnceLock;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

pub const APP_ID: &str = "TShell.Desktop";

pub fn register() -> anyhow::Result<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    let directory = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("Windows local application data is unavailable"))?
        .join("TShell")
        .join("branding");
    std::fs::create_dir_all(&directory)?;
    let icon = directory.join("tshell.png");
    let bytes = include_bytes!("../assets/tshell.png");
    if std::fs::read(&icon).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(&icon, bytes)?;
    }
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
    key.set_value("DisplayName", &"TShell")?;
    key.set_value("IconUri", &icon.as_os_str())?;
    key.set_value("IconBackgroundColor", &"0")?;
    let _ = REGISTERED.set(());
    Ok(())
}
