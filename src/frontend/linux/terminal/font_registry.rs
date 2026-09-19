//! Linux bundled font registration.
//!
//! Registers the app-bundled JetBrains Mono with Fontconfig's application font
//! set so the renderer can resolve it without touching the system font
//! directories. The registration is process-scoped and leaves no system files.

use anyhow::{anyhow, Context, Result};
use std::ffi::CString;
use std::path::{Path, PathBuf};

/// Absolute path to the bundled JetBrains Mono font. In a packaged app this
/// should be replaced by the installed resource directory at build time.
pub fn bundled_font_path() -> PathBuf {
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join("assets/fonts/JetBrainsMono-Regular.ttf"))
        })
        .filter(|path| path.is_file())
    {
        return path;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts/JetBrainsMono-Regular.ttf")
}

/// Register the bundled font with the current fontconfig configuration.
pub fn register_bundled_fonts() -> Result<()> {
    let primary = bundled_font_path();
    register_font(&primary)?;
    register_font(&primary.with_file_name("NotoSansSymbols2-Regular.ttf"))
}

fn register_font(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("bundled font missing: {}", path.display()));
    }
    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .context("bundled font path contains NUL")?;
    // SAFETY: fontconfig functions take and return plain C pointers; the path
    // CString outlives the call.
    unsafe {
        let config = fontconfig_sys::FcConfigGetCurrent();
        if config.is_null() {
            return Err(anyhow!("fontconfig current config is null"));
        }
        let added = fontconfig_sys::FcConfigAppFontAddFile(
            config,
            c_path.as_ptr() as *const fontconfig_sys::FcChar8,
        );
        if added == 0 {
            return Err(anyhow!(
                "fontconfig rejected bundled font: {}",
                path.display()
            ));
        }
        let _ = fontconfig_sys::FcConfigBuildFonts(config);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_font_asset_is_present() {
        assert!(bundled_font_path().exists());
        assert!(bundled_font_path()
            .with_file_name("NotoSansSymbols2-Regular.ttf")
            .exists());
    }
}
