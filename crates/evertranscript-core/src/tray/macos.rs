//! The menu bar item on macOS.
//!
//! Deliberately thin. Everything that decides *what* to show lives in the
//! parent module, where it can be tested; this renders a [`TrayView`],
//! forwards two clicks, and gets out of the way.
//!
//! Two things worth knowing before reading:
//!
//! - **No app bundle is involved.** A status item normally implies a
//!   packaged `.app`, but an accessory activation policy gets a plain binary
//!   into the menu bar without one — which matters because the Core is
//!   installed as a LaunchAgent running the binary directly (ADR-0026).
//! - **The GUI session is checked first.** Touching `NSApplication` where
//!   there is no window server is not something to find out about in
//!   production, and the Core has to keep serving on machines that have no
//!   screen at all. `CGSessionCopyCurrentDictionary` returning nothing is
//!   the signal, and it is checked before AppKit is touched.

use std::cell::RefCell;
use std::sync::Arc;

use objc2::AllocAnyThread;
use objc2::DefinedClass;
use objc2::define_class;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSApplication;
use objc2_app_kit::NSApplicationActivationPolicy;
use objc2_app_kit::NSEvent;
use objc2_app_kit::NSEventModifierFlags;
use objc2_app_kit::NSEventType;
use objc2_app_kit::NSImage;
use objc2_app_kit::NSMenu;
use objc2_app_kit::NSMenuItem;
use objc2_app_kit::NSStatusBar;
use objc2_app_kit::NSStatusItem;
use objc2_app_kit::NSVariableStatusItemLength;
use objc2_foundation::MainThreadMarker;
use objc2_foundation::NSData;
use objc2_foundation::NSObject;
use objc2_foundation::NSPoint;
use objc2_foundation::NSSize;
use objc2_foundation::NSString;
use objc2_foundation::NSTimer;
use objc2_foundation::ns_string;
use tracing::info;

use super::TrayController;
use super::TrayIndicator;
use super::TrayView;
use super::Unavailable;

/// How often the menu bar redraws itself from the published view.
///
/// The view is produced on the runtime; this only copies it into AppKit, so
/// it is cheap and can be brisk enough to feel immediate.
const REDRAW_SECONDS: f64 = 0.4;

/// The point size of the menu bar glyph: the bar is 22 pt, the glyph 18.
const GLYPH_POINTS: f64 = 18.0;

/// The menu bar glyphs, generated from the mark by `brand/render.mjs`.
///
/// Embedded in the binary because the Core has no bundle to load resources
/// from. Each is a multi-representation TIFF — the drawing at 18 px and at
/// 36 px — from which `NSImage` picks the one for the screen's scale.
const READY: &[u8] = include_bytes!("glyphs/ready.tiff");
const RECORDING: &[u8] = include_bytes!("glyphs/recording.tiff");
const BUSY: &[u8] = include_bytes!("glyphs/busy.tiff");
const ATTENTION: &[u8] = include_bytes!("glyphs/attention.tiff");

/// One decoded image per [`TrayIndicator`], made once and kept.
struct Glyphs {
    ready: Retained<NSImage>,
    recording: Retained<NSImage>,
    busy: Retained<NSImage>,
    attention: Retained<NSImage>,
}

impl Glyphs {
    fn decode() -> Result<Self, Unavailable> {
        Ok(Self {
            ready: template_image(READY, "ready")?,
            recording: template_image(RECORDING, "recording")?,
            busy: template_image(BUSY, "busy")?,
            attention: template_image(ATTENTION, "attention")?,
        })
    }

    fn image(&self, indicator: TrayIndicator) -> &NSImage {
        match indicator {
            TrayIndicator::Ready => &self.ready,
            TrayIndicator::Recording => &self.recording,
            TrayIndicator::Busy => &self.busy,
            TrayIndicator::Attention => &self.attention,
        }
    }
}

/// Decodes an embedded glyph as a template image.
///
/// A template is black with alpha and nothing else: the menu bar draws it
/// black on a light bar, white on a dark one, and dimmed when the item is
/// inactive — the one way a status item looks native in every appearance.
/// The bytes are compiled in, so a failure here is a build defect, not a
/// condition to recover from; it is reported rather than unwrapped so the
/// Core still serves headless.
fn template_image(bytes: &[u8], name: &str) -> Result<Retained<NSImage>, Unavailable> {
    let data = NSData::with_bytes(bytes);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return Err(Unavailable::Failed(format!(
            "the {name} menu bar glyph did not decode"
        )));
    };
    image.setTemplate(true);
    image.setSize(NSSize::new(GLYPH_POINTS, GLYPH_POINTS));
    Ok(image)
}

/// What the menu-bar objects and the controller need to reach each other.
struct Ivars {
    controller: Arc<TrayController>,
    status_item: Retained<NSStatusItem>,
    action_item: Retained<NSMenuItem>,
    status_line: Retained<NSMenuItem>,
    glyphs: Glyphs,
    /// The last view rendered, so an unchanged menu is left untouched.
    rendered: RefCell<Option<TrayView>>,
}

define_class!(
    // A plain NSObject: it is the menu items' target and the timer's
    // receiver, and owns nothing AppKit subclasses.
    #[unsafe(super(NSObject))]
    #[name = "EverTranscriptTray"]
    #[ivars = Ivars]
    struct Tray;

    impl Tray {
        /// The record/stop item.
        #[unsafe(method(toggleRecording:))]
        fn toggle_recording(&self, _sender: Option<&AnyObject>) {
            let view = self.ivars().controller.activate();
            // Render the click immediately rather than waiting for the next
            // redraw: a menu bar that lags a click feels broken.
            self.render(&view);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            self.ivars().controller.quit();
            if let Some(mtm) = MainThreadMarker::new() {
                stop_run_loop(mtm);
            }
        }

        /// Timer tick: copy the published view into the menu bar.
        #[unsafe(method(redraw:))]
        fn redraw(&self, _timer: Option<&AnyObject>) {
            // Quit from anywhere — a signal, the CLI — must close the menu
            // bar too, or the icon outlives the Core it represents.
            if self.ivars().controller.shutdown_token().is_cancelled() {
                if let Some(mtm) = MainThreadMarker::new() {
                    stop_run_loop(mtm);
                }
                return;
            }
            let view = self.ivars().controller.view();
            self.render(&view);
        }
    }
);

impl Tray {
    /// Writes a view into the menu bar, skipping work when nothing moved.
    fn render(&self, view: &TrayView) {
        let ivars = self.ivars();
        if ivars.rendered.borrow().as_ref() == Some(view) {
            return;
        }
        unsafe {
            if let Some(button) = ivars.status_item.button(MainThreadMarker::new_unchecked()) {
                button.setImage(Some(ivars.glyphs.image(view.indicator)));
                // The image is the whole indicator; a title beside it would
                // widen the item for nothing.
                button.setTitle(ns_string!(""));
            }
            ivars
                .action_item
                .setTitle(&NSString::from_str(&view.action));
            ivars.action_item.setEnabled(view.action_enabled);
            ivars
                .status_line
                .setTitle(&NSString::from_str(&view.status));
        }
        *ivars.rendered.borrow_mut() = Some(view.clone());
    }
}

/// Leaves `NSApplication::run`, so the daemon can finish shutting down.
///
/// `stop:` on its own is not enough, and the way it fails is quiet: it sets
/// a flag that `run` only checks after it finishes handling an event, and a
/// menu bar item nobody is clicking generates none. A Core told to quit by
/// SIGTERM would sit in the run loop indefinitely with its socket already
/// closed. Posting an event gives the loop the one thing it needs to notice.
fn stop_run_loop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let wake =
        NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
    if let Some(wake) = wake {
        app.postEvent_atStart(&wake, true);
    }
}

/// True when this process has a window server to draw in.
///
/// Checked before AppKit is touched: a Core on a headless machine must serve
/// normally, not discover the problem inside `NSApplication`.
fn has_gui_session() -> bool {
    objc2_core_graphics::CGSessionCopyCurrentDictionary().is_some()
}

pub fn run(controller: Arc<TrayController>) -> Result<(), Unavailable> {
    if !has_gui_session() {
        return Err(Unavailable::NoGuiSession);
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(Unavailable::Failed(
            "the tray must run on the main thread".to_string(),
        ));
    };

    let app = NSApplication::sharedApplication(mtm);
    // Accessory, not Regular: a menu bar item with no Dock icon and no
    // application menu. Without this the Core would appear in the Dock and
    // in the app switcher, which is not what a background recorder is.
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
        return Err(Unavailable::Failed(
            "macOS refused an accessory activation policy".to_string(),
        ));
    }

    let glyphs = Glyphs::decode()?;
    let bar = NSStatusBar::systemStatusBar();
    let status_item = bar.statusItemWithLength(NSVariableStatusItemLength);
    let menu = NSMenu::new(mtm);
    // Not clickable: this line is the explanation, not an action.
    let status_line = NSMenuItem::new(mtm);
    status_line.setEnabled(false);
    let action_item = NSMenuItem::new(mtm);
    action_item.setKeyEquivalent(ns_string!("r"));

    let tray = {
        let this = Tray::alloc().set_ivars(Ivars {
            controller: Arc::clone(&controller),
            status_item: status_item.clone(),
            action_item: action_item.clone(),
            status_line: status_line.clone(),
            glyphs,
            rendered: RefCell::new(None),
        });
        let this: Retained<Tray> = unsafe { msg_send![super(this), init] };
        this
    };

    unsafe {
        action_item.setTarget(Some(&tray));
        action_item.setAction(Some(objc2::sel!(toggleRecording:)));

        let quit = NSMenuItem::new(mtm);
        quit.setTitle(ns_string!("Quit EverTranscript"));
        quit.setKeyEquivalent(ns_string!("q"));
        quit.setTarget(Some(&tray));
        quit.setAction(Some(objc2::sel!(quit:)));

        menu.addItem(&status_line);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&action_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        // Explicit Quit, as ADR-0023 requires: closing a window must not be
        // the thing that stops a recorder, and neither should guessing.
        menu.addItem(&quit);
        status_item.setMenu(Some(&menu));
    }

    // Paint once before the first tick, so the menu bar is never briefly
    // blank or stale.
    tray.render(&controller.view());

    let _timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            REDRAW_SECONDS,
            &tray,
            objc2::sel!(redraw:),
            None,
            true,
        )
    };

    info!("the menu bar item is up");
    // Blocks until `stop:` — Quit, or the Core shutting down for any other
    // reason.
    app.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::GLYPH_POINTS;
    use super::Glyphs;

    #[test]
    fn the_gui_session_check_answers_without_touching_appkit() {
        // The daemon's headless path depends on this being callable safely
        // on any machine, including one with no window server.
        let _ = super::has_gui_session();
    }

    #[test]
    fn every_menu_bar_glyph_decodes_as_an_18_point_template() {
        // Decoding needs ImageIO, not a window server, so this runs on the
        // CI runner too. A glyph that failed here would take the tray down
        // on every Mac.
        let glyphs = Glyphs::decode().expect("the embedded glyphs decode");
        for (name, image) in [
            ("ready", &glyphs.ready),
            ("recording", &glyphs.recording),
            ("busy", &glyphs.busy),
            ("attention", &glyphs.attention),
        ] {
            assert!(
                image.isTemplate(),
                "{name} must be a template, or the dark menu bar draws it black"
            );
            let size = image.size();
            assert_eq!(
                (size.width, size.height),
                (GLYPH_POINTS, GLYPH_POINTS),
                "{name}"
            );
            assert!(
                image.representations().count() >= 2,
                "{name} should carry a 1x and a 2x drawing, got {}",
                image.representations().count()
            );
        }
    }
}
