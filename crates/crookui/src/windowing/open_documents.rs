//! macOS's request to open files, handed to the application as paths.
//!
//! What Finder sends an application Launch Services names for a file — a
//! script double-clicked once that application is the terminal scripts run
//! in, Open With, a file dropped on its Dock icon, `open -a` — is an Apple
//! Event AppKit answers by telling the application's delegate
//! `application:openURLs:`. winit's delegate has no such method, and AppKit's
//! answer for an application whose delegate cannot open anything is that it
//! cannot open files of that kind: the file was handed to Crook and went
//! nowhere.
//!
//! So the method is added to winit's delegate class here, at run time, the
//! way [`quit`](super::quit) adds `applicationShouldTerminate:`. What it is
//! handed goes to the event loop as paths, and from there to
//! [`WindowDelegate::open_files`](super::WindowDelegate::open_files). Only
//! files: a URL of any other kind — `open -a Crook https://…` sends one — is
//! said in the log and left alone, since nothing in a terminal opens it.
//!
//! # A request that starts Crook
//!
//! A double-click while Crook is not running launches it with the request in
//! hand, and AppKit tells the delegate during its launch, before
//! `applicationDidFinishLaunching:` — before winit has run a turn of its
//! loop, opened the window or passed a single event on. So the method does
//! nothing but post the paths on the event loop's proxy. winit empties that
//! queue only once it has said `resumed`, which is after the window opened,
//! and the application — built before the loop ran — has had its workspace
//! from the start. The request waits in the queue; nothing is dropped, and
//! nothing waits on a clock.
//!
//! # Not the command line
//!
//! AppKit has a second source of the same request: unless it is told
//! otherwise, it reads a Cocoa application's own command line as it launches
//! and asks the delegate to open every word that is neither an option nor an
//! option's value. Crook reads its command line itself, and in `crook -e vim
//! deploy.sh` AppKit takes `-e vim` for an option and its value and
//! `deploy.sh` for a file — which, once the delegate answers, would be a
//! second tab in front of the editor, running the script it was opened to
//! edit. So [`install`] tells AppKit the command line is not its to read. What
//! Finder, Open With and `open -a` send comes as an Apple Event, never on the
//! command line, and is not touched.

use std::cell::OnceCell;
use std::path::PathBuf;

use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2_foundation::{NSArray, NSURL};

thread_local! {
    /// Where the delegate's method hands the paths it was asked to open.
    ///
    /// Per thread, as [`quit`](super::quit)'s guard is and for its reasons:
    /// AppKit calls the method on the main thread, and the closure holds the
    /// event loop's proxy. Set once, since a process builds one event loop.
    static DELIVER: OnceCell<Box<dyn Fn(Vec<PathBuf>)>> = const { OnceCell::new() };
}

/// The method added to winit's delegate class, as AppKit calls it: the
/// delegate, the selector, the application, and the URLs to open.
///
/// Both objects as bare pointers: nothing here reads the application, and a
/// reference to a nil array — which only a test sends — would be undefined
/// behaviour.
type OpenUrls = extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);

/// Teaches winit's application delegate `application:openURLs:`, and hands
/// every file it is asked to open to `deliver`.
///
/// After the event loop is built, which is when winit makes its delegate, and
/// before it runs, which is when the first request can come. Where the
/// delegate cannot be taught — off the main thread, with no delegate, or a
/// winit that has learned the method itself — files are refused as they were
/// before, and the log says so.
pub(super) fn install(deliver: impl Fn(Vec<PathBuf>) + 'static) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    // First, and whether or not the delegate can be taught: a word of the
    // command line is never a file to open, method or no method.
    leave_the_command_line_alone();

    let Some(main_thread) = MainThreadMarker::new() else {
        log::warn!("opening files was set up off the main thread; Crook will open none");
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);
    let Some(delegate) = application.delegate() else {
        log::warn!("the application has no delegate to open files with; Crook will open none");
        return;
    };

    // Set before the method exists, so that no call can find it missing.
    if DELIVER.with(|cell| cell.set(Box::new(deliver))).is_err() {
        log::warn!("opening files was set up twice; the first one stands");
        return;
    }

    let class = AsRef::<AnyObject>::as_ref(&*delegate).class();
    if !teach(class) {
        log::warn!(
            "{} already answers application:openURLs:; Crook will not hear what it is asked to open",
            class.name().to_string_lossy()
        );
        return;
    }

    // Handed over again, for the reason `quit::install` hands it over: the
    // application may have written down which optional messages its delegate
    // answers when it was handed it, and this one came after.
    application.setDelegate(None);
    application.setDelegate(Some(&*delegate));
}

/// The default that says whether AppKit opens the words of an application's
/// command line, as it launches, as files.
const TREAT_UNKNOWN_ARGUMENTS_AS_OPEN: &str = "NSTreatUnknownArgumentsAsOpen";

/// Tells AppKit not to open the words of Crook's command line as files: see
/// [the module](self#not-the-command-line).
///
/// Before the event loop runs, since AppKit reads the command line as the
/// application finishes launching. Registered rather than stored, as Java's
/// AppKit glue does it: said again at every launch and never written into
/// the person's preferences, so a value they set there themselves still
/// stands. As the string `NO`, the form AppKit reads it in — Firefox's glue
/// says so of the same setting.
fn leave_the_command_line_alone() {
    use objc2_foundation::{NSDictionary, NSString, NSUserDefaults, ns_string};

    let key = NSString::from_str(TREAT_UNKNOWN_ARGUMENTS_AS_OPEN);
    let no: &AnyObject = ns_string!("NO");
    let defaults = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[no]);
    // SAFETY: the dictionary's keys are strings and its value one too, which
    // is what the registration domain holds and what AppKit reads this one as.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

/// Adds `application:openURLs:` to `class`, answered by [`open_urls`], and
/// says whether it did. Never over a method the class has of its own.
fn teach(class: &AnyClass) -> bool {
    use std::ffi::CString;
    use std::ptr;

    use objc2::encode::EncodeReturn;
    use objc2::runtime::Imp;
    use objc2::{Encode, ffi, sel};

    // Spelled out of the types rather than written as "v@:@@", as
    // `quit::teach` spells its own.
    let types = CString::new(format!(
        "{}{}{}{}{}",
        <()>::ENCODING_RETURN,
        <&AnyObject>::ENCODING,
        Sel::ENCODING,
        <*mut AnyObject>::ENCODING,
        <*mut NSArray<NSURL>>::ENCODING,
    ))
    .expect("a type encoding has no NUL in it");

    let method: OpenUrls = open_urls;
    // SAFETY: an `Imp` is any method's function pointer, called by the
    // runtime with the arguments the encoding names, which are the ones
    // `open_urls` takes.
    let imp = unsafe { std::mem::transmute::<OpenUrls, Imp>(method) };
    // SAFETY: `class` is a registered class, and `types` is the encoding of
    // `imp`'s return and arguments, self and the selector included.
    unsafe {
        ffi::class_addMethod(
            ptr::from_ref(class).cast_mut(),
            sel!(application:openURLs:),
            imp,
            types.as_ptr(),
        )
    }
    .as_bool()
}

/// The paths of the file URLs among `urls`, in their order.
///
/// Asked first whether each is a file's, since Foundation reads a path out
/// of a URL of any kind, and the path of a web page's address is no file.
/// The rest are named in the log by their scheme alone, which is enough to
/// tell where one came from and leaves out whatever a link carries after it.
fn files_in(urls: &NSArray<NSURL>) -> Vec<PathBuf> {
    urls.to_vec()
        .into_iter()
        .filter_map(|url| {
            let path = url.isFileURL().then(|| url.to_file_path()).flatten();
            if path.is_none() {
                let scheme = url.scheme().map(|scheme| scheme.to_string());
                log::info!(
                    "asked to open a URL that is not a file's ({}:); Crook opens only files",
                    scheme.as_deref().unwrap_or("no scheme")
                );
            }
            path
        })
        .collect()
}

/// `application:openURLs:`, as winit's delegate now answers it.
///
/// Nothing in it can panic, since an unwind would cross AppKit's frames. A
/// request that comes with nobody to hand it to is dropped, as AppKit
/// dropped every one before there was a method.
extern "C-unwind" fn open_urls(
    _: &AnyObject,
    _: Sel,
    _: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    // SAFETY: AppKit hands an array of URLs that lives for the length of the
    // call, or nil.
    let Some(urls) = (unsafe { urls.as_ref() }) else {
        return;
    };
    let files = files_in(urls);
    if files.is_empty() {
        return;
    }
    // `try_with` because a request can come as the process ends, when a
    // thread's locals may already be gone.
    let _ = DELIVER.try_with(|deliver| {
        if let Some(deliver) = deliver.get() {
            deliver(files);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;
    use std::ptr;
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::runtime::{ClassBuilder, NSObject};
    use objc2::{ClassType, msg_send};
    use objc2_foundation::NSString;

    use super::*;

    #[test]
    fn a_delegate_taught_to_open_files_hands_their_paths_on_and_nothing_else() {
        // A class of the test's own in place of winit's delegate, which
        // exists only once an event loop does, and only one per process.
        let class = ClassBuilder::new(c"CrookOpenDocumentsTestDelegate", NSObject::class())
            .expect("no other class has the name")
            .register();
        assert!(teach(class), "the class was not taught to open files");
        assert!(
            !teach(class),
            "a class that answers already was taught over"
        );

        let delivered = Rc::new(RefCell::new(Vec::new()));
        let into = Rc::clone(&delivered);
        assert!(
            DELIVER
                .with(|cell| cell.set(Box::new(move |files| into.borrow_mut().push(files))))
                .is_ok()
        );
        // SAFETY: `new` is NSObject's, and the class adds no state to it.
        let delegate: Retained<AnyObject> = unsafe { msg_send![class, new] };
        let open = |urls: Option<&NSArray<NSURL>>| {
            let urls = urls.map_or(ptr::null_mut(), |urls| ptr::from_ref(urls).cast_mut());
            // SAFETY: the method was added above with this signature, and
            // never reads the application it is handed.
            let () = unsafe {
                msg_send![&*delegate, application: ptr::null_mut::<AnyObject>(), openURLs: urls]
            };
        };

        let script = NSURL::from_file_path("/Users/someone/My Scripts/deploy").expect("a file URL");
        let page = NSURL::URLWithString(&NSString::from_str("https://crook.id/")).expect("a URL");
        let folder = NSURL::from_file_path("/Users/someone/work").expect("a file URL");
        open(Some(&*NSArray::from_retained_slice(&[
            script, page, folder,
        ])));
        assert_eq!(
            *delivered.borrow(),
            [vec![
                Path::new("/Users/someone/My Scripts/deploy").to_path_buf(),
                Path::new("/Users/someone/work").to_path_buf(),
            ]],
            "the files were not handed on as their paths, in order, without the page"
        );

        // A request with no file in it, and none at all, hand nothing on.
        let page = NSURL::URLWithString(&NSString::from_str("https://crook.id/")).expect("a URL");
        open(Some(&*NSArray::from_retained_slice(&[page])));
        open(None);
        assert_eq!(
            delivered.borrow().len(),
            1,
            "a request with no file was handed on"
        );
    }

    #[test]
    fn the_words_of_the_command_line_are_not_files_to_open() {
        use objc2_foundation::NSUserDefaults;

        // What AppKit asks, as it launches, before it opens a loose word of
        // `crook -e vim deploy.sh` as a file.
        let asked = || {
            NSUserDefaults::standardUserDefaults()
                .stringForKey(&NSString::from_str(TREAT_UNKNOWN_ARGUMENTS_AS_OPEN))
                .map(|value| value.to_string())
        };
        assert_ne!(
            asked().as_deref(),
            Some("NO"),
            "AppKit was told already, so this test cannot tell whether Crook tells it"
        );
        // Through `install`, the call Crook makes, so that the test fails if
        // `install` stops registering the default or registers it only after
        // one of its early returns. libtest runs each test on a thread of its
        // own, where `install` returns at the main-thread check; on the main
        // thread a test process has no delegate. Either way it never reaches
        // `DELIVER` or teaches a class.
        install(|_| {});
        assert_eq!(
            asked().as_deref(),
            Some("NO"),
            "install returned without telling AppKit the command line is not its to read"
        );
    }
}
