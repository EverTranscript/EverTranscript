//! API keys, and the one place they are allowed to be.
//!
//! Story 41: the OS credential store — macOS Keychain, Windows Credential
//! Manager — and nowhere else. Not the database, not the Mirrors, not the
//! logs.
//!
//! M4 is the first milestone that holds a secret at all, which changes the
//! status of a test that has existed since M1:
//! `nothing_key_shaped_reaches_the_record_or_the_logs` has been passing on
//! an empty room. From here it is doing real work.
//!
//! **A key is never returned to a Client once stored.** The API here can
//! say whether one exists, replace it, and delete it — and cannot read it
//! back out to anyone but the code that makes the request. A settings screen
//! able to display a key is a settings screen able to leak one into a
//! screenshot, a screen share, or a support ticket, and every one of those
//! is a meeting.

use anyhow::Result;

/// What the credential store files these under.
const SERVICE: &str = "com.evertranscript.core";

/// Why a credential operation did not work.
///
/// Reported plainly rather than worked around. **There is deliberately no
/// fallback path that writes the key somewhere else** — a product that
/// quietly wrote a key to a config file when the Keychain was locked would
/// be making the one promise story 41 exists to keep, and then breaking it
/// at exactly the moment nobody was watching.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("the credential store could not be reached: {0}")]
    Unavailable(String),
}

/// Where a key is stored, per Backend account.
///
/// Keyed by the provider rather than one key for everything, so switching
/// providers does not require re-entering the previous one — an Operator
/// trying Anthropic for a week should not lose their OpenAI key to it.
fn entry(account: &str) -> Result<keyring::Entry, CredentialError> {
    keyring::Entry::new(SERVICE, account)
        .map_err(|error| CredentialError::Unavailable(error.to_string()))
}

/// Stores a key, replacing any previous one for this account.
pub fn set(account: &str, key: &str) -> Result<(), CredentialError> {
    entry(account)?
        .set_password(key)
        .map_err(|error| CredentialError::Unavailable(error.to_string()))
}

/// Reads a key back — for the code that makes the request, and nothing else.
///
/// Deliberately not exposed over the protocol. See the module note.
pub fn get(account: &str) -> Result<Option<String>, CredentialError> {
    match entry(account)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(CredentialError::Unavailable(error.to_string())),
    }
}

/// Whether a key exists — the question a Client is allowed to ask.
pub fn exists(account: &str) -> bool {
    matches!(get(account), Ok(Some(_)))
}

/// Removes a key. A first-class act, not a side effect of anything else.
///
/// In particular, switching the Knob to Local does **not** call this: an
/// Operator may be switching for one meeting, and deleting their key as a
/// consequence would punish them for choosing privacy.
pub fn delete(account: &str) -> Result<bool, CredentialError> {
    match entry(account)?.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(CredentialError::Unavailable(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique account per test run, so a failed run never leaves a
    /// credential behind that a later one reads as its own.
    fn account() -> String {
        format!("test-{}", uuid::Uuid::now_v7())
    }

    /// The runners have no login keychain, and a test that silently passed
    /// there would be asserting nothing on the platform that matters.
    fn skip_without_a_credential_store() -> bool {
        let probe = account();
        match set(&probe, "probe") {
            Ok(()) => {
                let _ = delete(&probe);
                false
            }
            Err(error) => {
                eprintln!("skipped: no usable credential store here ({error})");
                true
            }
        }
    }

    #[test]
    fn a_key_round_trips_through_the_os_store() {
        if skip_without_a_credential_store() {
            return;
        }
        let account = account();
        set(&account, "sk-secret").expect("stores");
        assert_eq!(get(&account).expect("reads").as_deref(), Some("sk-secret"));
        assert!(exists(&account));
        delete(&account).expect("deletes");
    }

    #[test]
    fn a_missing_key_is_absent_rather_than_an_error() {
        if skip_without_a_credential_store() {
            return;
        }
        // "No key set" is the ordinary state of a fresh install, not a
        // failure — treating it as one would make the Knob's local default
        // look broken on every new machine.
        assert_eq!(get(&account()).expect("reads"), None);
        assert!(!exists(&account()));
    }

    #[test]
    fn replacing_a_key_does_not_leave_the_old_one_behind() {
        if skip_without_a_credential_store() {
            return;
        }
        let account = account();
        set(&account, "sk-old").expect("stores");
        set(&account, "sk-new").expect("replaces");
        assert_eq!(get(&account).expect("reads").as_deref(), Some("sk-new"));
        delete(&account).expect("cleans up");
    }

    #[test]
    fn deleting_a_key_that_is_not_there_is_not_an_error() {
        if skip_without_a_credential_store() {
            return;
        }
        // An Operator clearing a key twice, or clearing one they never set,
        // has done nothing wrong.
        assert!(!delete(&account()).expect("no error"));
    }

    #[test]
    fn keys_are_kept_per_provider() {
        if skip_without_a_credential_store() {
            return;
        }
        // An Operator trying one provider for a week should not lose the
        // other's key to it.
        let (first, second) = (account(), account());
        set(&first, "sk-first").expect("stores");
        set(&second, "sk-second").expect("stores");
        assert_eq!(get(&first).expect("reads").as_deref(), Some("sk-first"));
        assert_eq!(get(&second).expect("reads").as_deref(), Some("sk-second"));
        delete(&first).expect("cleans up");
        delete(&second).expect("cleans up");
    }

    #[test]
    fn deleting_one_providers_key_leaves_the_others_alone() {
        if skip_without_a_credential_store() {
            return;
        }
        let (kept, removed) = (account(), account());
        set(&kept, "sk-kept").expect("stores");
        set(&removed, "sk-removed").expect("stores");
        delete(&removed).expect("deletes");
        assert!(exists(&kept), "the other key survived");
        delete(&kept).expect("cleans up");
    }
}
