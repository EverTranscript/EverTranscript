//! The update feed: Sanctioned Traffic entry one.
//!
//! ADR-0016 chose direct download plus in-app updates precisely so shipping
//! never waits on an app store's opinion of system-audio capture. ADR-0034
//! then made the check one of exactly three things this product may ever say
//! on the wire, **disableable in Settings** — and the guarantee test's final
//! form depends on that switch: "with updates off and models downloaded,
//! literally zero".
//!
//! The check itself is the Client's. electron-updater replaces the whole
//! bundle, Core included, and reads the switch from the Core's settings
//! (`clients/electron/src/main/updates.ts`). The Core keeps only the host, so
//! the trust surface can name it (DECISIONS Q306).

/// Where the update feed lives.
///
/// Named here rather than assembled at the call site so the trust surface
/// can show the exact host an Operator would see in a firewall log.
pub const UPDATE_FEED_HOST: &str = "https://github.com/EverTranscript/EverTranscript";
