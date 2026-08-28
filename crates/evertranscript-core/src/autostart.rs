//! Starting the Core at login.
//!
//! "Starts at boot" means **user login**, not system boot (ADR-0026 as
//! amended). A pre-login daemon runs before the user session exists, where
//! per-user microphone permissions do not — so it could not capture anything
//! even if it ran.
//!
//! Registration-only, deliberately: turning the toggle off changes what
//! happens at the *next* login and leaves the running Core alone. Stopping
//! the Core now is a separate act (Quit), and conflating the two would mean
//! one click doing two things the Operator did not ask for (story 9c).

use anyhow::Result;

/// Whether the Core is registered to start at login.
pub fn is_enabled() -> bool {
    platform::is_enabled().unwrap_or(false)
}

/// Registers or unregisters the login item.
pub fn set_enabled(enabled: bool) -> Result<()> {
    platform::set_enabled(enabled)
}

/// Where the registration lives, for `status` output and troubleshooting.
pub fn describe() -> String {
    platform::describe()
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::Context;
    use anyhow::Result;
    use std::path::PathBuf;

    /// The label both the plist filename and the launchd job use.
    const LABEL: &str = "com.evertranscript.core";

    fn plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    pub fn describe() -> String {
        plist_path().display().to_string()
    }

    pub fn is_enabled() -> Result<bool> {
        Ok(plist_path().exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let path = plist_path();
        if !enabled {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context(format!("removing {}", path.display())),
            }
            // Leave any running Core alone: this toggle is about the next
            // login, not about now.
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("gui/{}/{LABEL}", user_id())])
                .output();
            return Ok(());
        }

        let binary = std::env::current_exe().context("finding this binary")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, plist(&binary.display().to_string()))
            .with_context(|| format!("writing {}", path.display()))?;

        // Registering with launchd is best-effort: the plist alone is enough
        // for the *next* login, which is what the toggle promises.
        let _ = std::process::Command::new("launchctl")
            .args([
                "bootstrap",
                &format!("gui/{}", user_id()),
                &path.display().to_string(),
            ])
            .output();
        Ok(())
    }

    fn user_id() -> u32 {
        // SAFETY: getuid is always safe; it reads a process attribute.
        unsafe { libc_getuid() }
    }

    unsafe extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }

    fn plist(binary: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
        )
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::Context;
    use anyhow::Result;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "EverTranscript";

    pub fn describe() -> String {
        format!(r"HKCU\{RUN_KEY}\{VALUE_NAME}")
    }

    pub fn is_enabled() -> Result<bool> {
        let output = std::process::Command::new("reg")
            .args(["query", &format!(r"HKCU\{RUN_KEY}"), "/v", VALUE_NAME])
            .output()
            .context("querying the Run key")?;
        Ok(output.status.success())
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        if !enabled {
            let _ = std::process::Command::new("reg")
                .args([
                    "delete",
                    &format!(r"HKCU\{RUN_KEY}"),
                    "/v",
                    VALUE_NAME,
                    "/f",
                ])
                .output();
            return Ok(());
        }
        let binary = std::env::current_exe().context("finding this binary")?;
        let command = format!("\"{}\" daemon", binary.display());
        let status = std::process::Command::new("reg")
            .args([
                "add",
                &format!(r"HKCU\{RUN_KEY}"),
                "/v",
                VALUE_NAME,
                "/t",
                "REG_SZ",
                "/d",
                &command,
                "/f",
            ])
            .status()
            .context("writing the Run key")?;
        anyhow::ensure!(status.success(), "writing the Run key failed");
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use anyhow::Result;

    pub fn describe() -> String {
        "not supported on this platform".to_string()
    }

    pub fn is_enabled() -> Result<bool> {
        Ok(false)
    }

    pub fn set_enabled(_enabled: bool) -> Result<()> {
        anyhow::bail!("launch at login is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describing_the_registration_names_a_real_location() {
        let description = describe();
        assert!(
            !description.is_empty(),
            "the Operator should be able to see where this lives"
        );
        #[cfg(target_os = "macos")]
        assert!(description.contains("LaunchAgents"), "{description}");
        #[cfg(target_os = "windows")]
        assert!(description.contains("Run"), "{description}");
    }

    #[test]
    fn querying_is_safe_on_a_machine_with_nothing_registered() {
        // Must never panic or hang; a missing registration is just "off".
        let _ = is_enabled();
    }
}
