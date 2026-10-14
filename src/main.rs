// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

mod errors;
mod log;
mod pairer;
mod private_api;

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType, NSControlSize,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSProgressIndicator, NSProgressIndicatorStyle, NSTextAlignment,
    NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, ns_string};

use crate::pairer::Pairer;

const INSTRUCTIONS: &str = "Press the 🔴 SYNC button on the back of your Wii Remote now.\n\n\
                            (You may need to remove the battery cover to access the button.)\n\n\
                            Do not press any other buttons.";

#[derive(Default)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    pairer: OnceCell<Retained<Pairer>>,
}

define_class!(
    // SAFETY:
    // - NSObject does not have any subclassing requirements.
    // - AppDelegate does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();

            let window = create_window(mtm);
            window.makeKeyAndOrderFront(None);

            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);

            // Creating the pairer creates a CBCentralManager, which prompts for Bluetooth access if needed. Do this
            // once the window is visible so that any alerts have a window to attach to.
            let pairer = Pairer::new(&window, mtm);

            self.ivars().window.set(window).ok();
            self.ivars().pairer.set(pairer).ok();
        }

        #[unsafe(method(applicationSupportsSecureRestorableState:))]
        fn application_supports_secure_restorable_state(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn application_should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        // SAFETY: NSObject's init method has this signature.
        unsafe { msg_send![super(this), init] }
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn create_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    // SAFETY: Releasing when closed is disabled below, as required for windows not owned by a window controller.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, 375.0, 194.0),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: The window is kept alive by AppDelegate.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("WiimotePair"));
    window.setRestorable(false);

    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 375.0, 194.0));
    view.setWantsLayer(true);

    let progress_indicator =
        NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), rect(145.0, 158.0, 16.0, 16.0));
    progress_indicator.setStyle(NSProgressIndicatorStyle::Spinning);
    progress_indicator.setControlSize(NSControlSize::Small);
    progress_indicator.setIndeterminate(true);
    // SAFETY: The sender may be nil.
    unsafe { progress_indicator.startAnimation(None) };
    view.addSubview(&progress_indicator);

    let searching_label = NSTextField::labelWithString(ns_string!("Searching"), mtm);
    searching_label.setFrame(rect(167.0, 158.0, 65.0, 16.0));
    view.addSubview(&searching_label);

    let instructions_label =
        NSTextField::wrappingLabelWithString(&objc2_foundation::NSString::from_str(INSTRUCTIONS), mtm);
    instructions_label.setFrame(rect(20.0, 27.0, 335.0, 112.0));
    instructions_label.setAlignment(NSTextAlignment::Center);
    instructions_label.setSelectable(false);
    view.addSubview(&instructions_label);

    window.setContentView(Some(&view));
    window.center();

    window
}

fn create_main_menu(app: &NSApplication, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("WiimotePair"));

    // SAFETY: All selectors are standard NSApplication actions, sent through the responder chain.
    unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("About WiimotePair"),
            Some(sel!(orderFrontStandardAboutPanel:)),
            ns_string!(""),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        let services_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Services"));
        app_menu
            .addItemWithTitle_action_keyEquivalent(ns_string!("Services"), None, ns_string!(""))
            .setSubmenu(Some(&services_menu));
        app.setServicesMenu(Some(&services_menu));
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Hide WiimotePair"),
            Some(sel!(hide:)),
            ns_string!("h"),
        );
        app_menu
            .addItemWithTitle_action_keyEquivalent(
                ns_string!("Hide Others"),
                Some(sel!(hideOtherApplications:)),
                ns_string!("h"),
            )
            .setKeyEquivalentModifierMask(NSEventModifierFlags::Option | NSEventModifierFlags::Command);
        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Show All"),
            Some(sel!(unhideAllApplications:)),
            ns_string!(""),
        );
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Quit WiimotePair"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        );
    }

    let main_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Main Menu"));
    // SAFETY: The item has no action.
    unsafe { main_menu.addItemWithTitle_action_keyEquivalent(ns_string!("WiimotePair"), None, ns_string!("")) }
        .setSubmenu(Some(&app_menu));

    main_menu
}

/// Shows an alert attached to `window`, calling `callback` once it is dismissed.
pub fn show_alert(window: &NSWindow, title: &str, text: &str, callback: impl Fn() + 'static) {
    let mtm = window.mtm();

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&objc2_foundation::NSString::from_str(title));
    alert.setInformativeText(&objc2_foundation::NSString::from_str(text));
    alert.addButtonWithTitle(ns_string!("OK"));

    let handler = block2::RcBlock::new(move |_response| callback());
    alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
}

fn main() {
    let mtm = MainThreadMarker::new().expect("must be run on the main thread");

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.setMainMenu(Some(&create_main_menu(&app, mtm)));

    // NSApplication only holds a weak reference to its delegate.
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app.run();
}
