//! Machine-local settings.
//!
//! Deliberately *not* in the History folder. Settings describe this
//! installation — whether the Operator acknowledged the Briefing here,
//! whether the Core starts at login here — and copying History to a new
//! machine must not carry a consent acknowledgment along with it (ADR-0035).

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Whether the Operator has acknowledged the first-run Briefing on this
    /// machine. Nothing is captured before this is true (ADR-0023): "on by
    /// default" means the Auto-Record toggle ships On, not that recording
    /// precedes consent education.
    pub briefing_acknowledged: bool,
    /// Whether the Core starts at login. Ships on (ADR-0026 as amended).
    pub launch_at_login: bool,
    /// Whether Auto-Record is on. Ships on (ADR-0023). Detection itself
    /// arrives in M2; the setting exists now so the M1 tray and CLI have one
    /// switch to show rather than growing one later.
    pub auto_record: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // The one thing that does *not* default to convenient.
            briefing_acknowledged: false,
            launch_at_login: true,
            auto_record: true,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        crate::paths::app_support_dir().join("settings.json")
    }

    /// Loads settings, falling back to defaults on anything unreadable.
    ///
    /// A corrupt settings file must not stop the Core from running — but it
    /// must not silently grant consent either, and the default for the
    /// acknowledgment is false, so the failure mode is "ask again".
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
                warn!(path = %path.display(), %error, "settings are unreadable; using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes through a temporary file so a crash mid-write cannot leave
    /// settings truncated — which, for the acknowledgment flag, would mean
    /// re-asking rather than wrongly assuming consent.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_has_not_acknowledged_anything() {
        let settings = Settings::default();
        assert!(
            !settings.briefing_acknowledged,
            "consent is never the default"
        );
        // These two do ship on, which is the ratified posture.
        assert!(settings.launch_at_login);
        assert!(settings.auto_record);
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");

        let settings = Settings {
            briefing_acknowledged: true,
            launch_at_login: false,
            ..Default::default()
        };
        settings.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn a_corrupt_file_falls_back_to_defaults_rather_than_to_consent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ this is not json").expect("write");

        let loaded = Settings::load_from(&path);
        assert!(
            !loaded.briefing_acknowledged,
            "an unreadable file must never be read as an acknowledgment"
        );
    }

    #[test]
    fn a_missing_file_is_a_fresh_install() {
        let loaded = Settings::load_from(Path::new("/nonexistent/settings.json"));
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn unknown_fields_do_not_break_an_older_build() {
        // A newer build wrote a setting this one does not know. Refusing to
        // load would strand the Operator on the older version.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"briefingAcknowledged": true, "somethingFromTheFuture": 42}"#,
        )
        .expect("write");

        let loaded = Settings::load_from(&path);
        assert!(loaded.briefing_acknowledged);
    }
}
