//! Finder integration: files opened by double-click / Open With.
//!
//! LaunchServices does not pass such files as command-line arguments (verified with a
//! probe .app: the executable runs with argc=1). It delivers them to the running (or
//! launching) application as the `application:openURLs:` delegate message instead.
//! eframe/winit installs its own `WinitApplicationDelegate` and never implements that
//! message, so the files would be dropped. This module adds the method to winit's
//! delegate class at runtime; the implementation only queues paths and wakes egui,
//! and the viewer drains the queue in its logic pass.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<Mutex<Option<egui::Context>>> = OnceLock::new();

fn queue(paths: impl IntoIterator<Item = PathBuf>) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.extend(paths);
    }
    if let Some(ctx) = CTX.get().and_then(|c| c.lock().ok()).and_then(|c| c.clone()) {
        ctx.request_repaint();
    }
}

/// Paths Finder has asked us to open since the last call; empties the queue.
pub fn take_pending() -> Vec<PathBuf> {
    PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}

/// Arrange for the open-file methods to be in place before Finder can deliver
/// a file, including on a cold launch.
///
/// Finder delivers a file opened at launch before the application finishes
/// launching — earlier than eframe's creation callback runs, and a second
/// winit event loop cannot be created to register the delegate class sooner
/// (winit rejects event-loop recreation). So: observe
/// `NSApplicationWillFinishLaunchingNotification`, which fires before the
/// open is delivered. The observer is the shared NSApplication itself, given
/// a one-off method for the purpose; when it fires, winit's delegate class
/// exists and the open methods are added to it. Call on the main thread,
/// before `run_native`.
pub fn prepare_open_handler() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ONCE: AtomicBool = AtomicBool::new(false);
    if ONCE.swap(true, Ordering::Relaxed) {
        return;
    }
    add_open_methods(); // no-op until the delegate class exists; also covers re-entry
    // SAFETY: all messages go to AppKit/Foundation singletons with the
    // signatures used; the observer (the app) outlives the process.
    unsafe {
        let Some(app_cls) = AnyClass::get(c"NSApplication") else { return };
        let app_ptr = app_cls as *const AnyClass as *mut AnyClass;
        add(app_ptr, sel!(viewrWillFinishLaunching:), will_finish_launching as *const () as usize, c"v@:@");
        let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
        let Some(center_cls) = AnyClass::get(c"NSNotificationCenter") else { return };
        let center: *mut AnyObject = msg_send![center_cls, defaultCenter];
        let Some(string_cls) = AnyClass::get(c"NSString") else { return };
        let name: *mut AnyObject = msg_send![string_cls, stringWithUTF8String: c"NSApplicationWillFinishLaunchingNotification".as_ptr()];
        if app.is_null() || center.is_null() || name.is_null() {
            return;
        }
        let _: () = msg_send![
            &*center,
            addObserver: &*app,
            selector: sel!(viewrWillFinishLaunching:),
            name: &*name,
            object: std::ptr::null::<AnyObject>()
        ];
    }
}

/// `-viewrWillFinishLaunching:` — the delegate class exists by now, so the
/// open methods can finally be added to it.
extern "C-unwind" fn will_finish_launching(_this: &AnyObject, _cmd: Sel, _notif: &AnyObject) {
    add_open_methods();
}

/// Add `application:openURLs:` (and the legacy `application:openFiles:` /
/// `application:openFile:` it supersedes) to winit's application-delegate class.
/// No-op if the class is not registered yet or already handles opens; safe to
/// call again. Must run on the main thread (the creation callback does).
pub fn install_open_handler(ctx: &egui::Context) {
    let _ = CTX.get_or_init(|| Mutex::new(None)).lock().map(|mut c| *c = Some(ctx.clone()));
    add_open_methods();
}

fn add_open_methods() {
    let Some(cls) = AnyClass::get(c"WinitApplicationDelegate") else { return };
    // SAFETY: the class is winit's delegate; the IMPs match the selectors' signatures
    // and only touch the queue above. Adding a method mutates the class, so it needs
    // a mutable pointer; the runtime serializes this on the main thread.
    let cls_ptr = cls as *const AnyClass as *mut AnyClass;
    unsafe {
        add(cls_ptr, sel!(application:openURLs:), open_urls as *const () as usize, c"v@:@@");
        add(cls_ptr, sel!(application:openFiles:), open_files as *const () as usize, c"v@:@@");
        add(cls_ptr, sel!(application:openFile:), open_file as *const () as usize, c"B@:@@");
    }
}

unsafe fn add(cls: *mut AnyClass, sel: Sel, imp: usize, types: &CStr) {
    let Some(cls_ref) = (unsafe { cls.as_ref() }) else { return };
    if cls_ref.instance_method(sel).is_some() {
        return; // already installed (or winit grew its own): leave it alone
    }
    // SAFETY: `imp` is an `extern "C-unwind" fn` with the signature `types` encodes.
    let imp: Imp = unsafe { std::mem::transmute(imp) };
    let _ = unsafe { objc2::ffi::class_addMethod(cls, sel, imp, types.as_ptr()) };
}

/// `-application:openURLs:` — one NSURL per file Finder wants shown.
extern "C-unwind" fn open_urls(_this: &AnyObject, _cmd: Sel, _app: &AnyObject, urls: &AnyObject) {
    // SAFETY: `urls` is the NSArray<NSURL> AppKit passes; we only read from it.
    unsafe {
        let count: usize = msg_send![urls, count];
        let mut paths = Vec::new();
        for i in 0..count {
            let url: *const AnyObject = msg_send![urls, objectAtIndex: i];
            if url.is_null() {
                continue;
            }
            let url = &*url;
            let is_file: bool = msg_send![url, isFileURL];
            if is_file && let Some(p) = ns_path(msg_send![url, path]) {
                paths.push(p);
            }
        }
        queue(paths);
    }
}

/// `-application:openFiles:` — legacy form, an NSArray of NSString paths.
extern "C-unwind" fn open_files(_this: &AnyObject, _cmd: Sel, _app: &AnyObject, files: &AnyObject) {
    // SAFETY: `files` is the NSArray<NSString> AppKit passes; we only read from it.
    unsafe {
        let count: usize = msg_send![files, count];
        let mut paths = Vec::new();
        for i in 0..count {
            let name: *const AnyObject = msg_send![files, objectAtIndex: i];
            if !name.is_null()
                && let Some(p) = ns_path(name)
            {
                paths.push(p);
            }
        }
        queue(paths);
    }
}

/// `-application:openFile:` — legacy single-file form. Returns whether we took it.
extern "C-unwind" fn open_file(_this: &AnyObject, _cmd: Sel, _app: &AnyObject, name: &AnyObject) -> bool {
    // SAFETY: `name` is the NSString AppKit passes; we only read from it.
    let path = unsafe { ns_path(name as *const AnyObject) };
    match path {
        Some(p) => {
            queue([p]);
            true
        }
        None => false,
    }
}

/// An NSString's path as a PathBuf, or None for nil / non-UTF-8 strings.
///
/// SAFETY: `string` must be a valid NSString (or null).
unsafe fn ns_path(string: *const AnyObject) -> Option<PathBuf> {
    if string.is_null() {
        return None;
    }
    // SAFETY: see above; UTF8String is a NUL-terminated C string owned by the NSString.
    unsafe {
        let cstr: *const c_char = msg_send![&*string, UTF8String];
        if cstr.is_null() {
            return None;
        }
        CStr::from_ptr(cstr).to_str().ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_queue_drains_in_order() {
        queue([PathBuf::from("/a/one.jpg"), PathBuf::from("/b/two.NEF")]);
        queue([PathBuf::from("/c/three.png")]);
        assert_eq!(take_pending(), vec![PathBuf::from("/a/one.jpg"), PathBuf::from("/b/two.NEF"), PathBuf::from("/c/three.png")]);
        assert!(take_pending().is_empty());
    }
}
