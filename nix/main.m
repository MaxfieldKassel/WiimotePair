// Copyright 2024 Dolphin Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

// Entry point for builds that cannot compile Main.storyboard (which requires
// Xcode's ibtool), such as the Nix build. This recreates the storyboard's menu
// bar and window in code. Keep it in sync with Base.lproj/Main.storyboard.

#import <Cocoa/Cocoa.h>

#import "AppDelegate.h"
#import "ViewController.h"

@interface ProgrammaticViewController : ViewController

@end

@implementation ProgrammaticViewController

- (void)loadView {
    NSView* view = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, 375, 194)];
    view.wantsLayer = YES;

    NSProgressIndicator* progressIndicator = [[NSProgressIndicator alloc] initWithFrame:NSMakeRect(145, 158, 16, 16)];
    progressIndicator.style = NSProgressIndicatorStyleSpinning;
    progressIndicator.controlSize = NSControlSizeSmall;
    progressIndicator.indeterminate = YES;
    [view addSubview:progressIndicator];
    self.progressIndicator = progressIndicator;

    NSTextField* searchingLabel = [NSTextField labelWithString:@"Searching"];
    searchingLabel.frame = NSMakeRect(167, 158, 65, 16);
    [view addSubview:searchingLabel];

    NSTextField* instructionsLabel = [NSTextField wrappingLabelWithString:@"Press the 🔴 SYNC button on the back of your Wii Remote now.\n\n(You may need to remove the battery cover to access the button.)\n\nDo not press any other buttons."];
    instructionsLabel.frame = NSMakeRect(20, 27, 335, 112);
    instructionsLabel.alignment = NSTextAlignmentCenter;
    instructionsLabel.selectable = NO;
    [view addSubview:instructionsLabel];

    self.view = view;
}

@end

static NSMenu* CreateMainMenu(void) {
    NSMenu* appMenu = [[NSMenu alloc] initWithTitle:@"WiimotePair"];

    [appMenu addItemWithTitle:@"About WiimotePair" action:@selector(orderFrontStandardAboutPanel:) keyEquivalent:@""];
    [appMenu addItem:[NSMenuItem separatorItem]];

    NSMenu* servicesMenu = [[NSMenu alloc] initWithTitle:@"Services"];
    [[appMenu addItemWithTitle:@"Services" action:nil keyEquivalent:@""] setSubmenu:servicesMenu];
    NSApp.servicesMenu = servicesMenu;
    [appMenu addItem:[NSMenuItem separatorItem]];

    [appMenu addItemWithTitle:@"Hide WiimotePair" action:@selector(hide:) keyEquivalent:@"h"];
    NSMenuItem* hideOthersItem = [appMenu addItemWithTitle:@"Hide Others" action:@selector(hideOtherApplications:) keyEquivalent:@"h"];
    hideOthersItem.keyEquivalentModifierMask = NSEventModifierFlagOption | NSEventModifierFlagCommand;
    [appMenu addItemWithTitle:@"Show All" action:@selector(unhideAllApplications:) keyEquivalent:@""];
    [appMenu addItem:[NSMenuItem separatorItem]];

    [appMenu addItemWithTitle:@"Quit WiimotePair" action:@selector(terminate:) keyEquivalent:@"q"];

    NSMenu* mainMenu = [[NSMenu alloc] initWithTitle:@"Main Menu"];
    [[mainMenu addItemWithTitle:@"WiimotePair" action:nil keyEquivalent:@""] setSubmenu:appMenu];

    return mainMenu;
}

int main(int argc, const char * argv[]) {
    @autoreleasepool {
        NSApplication* app = [NSApplication sharedApplication];

        // NSApplication only holds a weak reference to its delegate.
        AppDelegate* delegate = [[AppDelegate alloc] init];
        app.delegate = delegate;
        app.mainMenu = CreateMainMenu();

        NSWindow* window = [NSWindow windowWithContentViewController:[[ProgrammaticViewController alloc] init]];
        window.title = @"WiimotePair";
        window.styleMask = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable;
        window.restorable = NO;
        window.releasedWhenClosed = NO;
        [window center];

        NSWindowController* windowController = [[NSWindowController alloc] initWithWindow:window];
        [windowController showWindow:nil];

        [app activateIgnoringOtherApps:YES];
        [app run];
    }
    return 0;
}
