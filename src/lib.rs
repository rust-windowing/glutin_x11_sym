#![cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate winit_types;
#[macro_use]
extern crate log;

use parking_lot::Mutex;
use winit_types::error::Error;
use winit_types::platform::{OsError, XError, XNotSupported};
use x11_dl::error::OpenError;
use x11_dl::xlib::{Display as XDisplay, XErrorEvent};

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::os::raw;
use std::ptr;
use std::sync::{Arc, Weak};

lazy_static! {
    pub static ref XEXT: Result<x11_dl::dpms::Xext, OpenError> = x11_dl::dpms::Xext::open();
    pub static ref XSS: Result<x11_dl::xss::Xss, OpenError> = x11_dl::xss::Xss::open();
    pub static ref XFT: Result<x11_dl::xft::Xft, OpenError> = x11_dl::xft::Xft::open();
    pub static ref XT: Result<x11_dl::xt::Xt, OpenError> = x11_dl::xt::Xt::open();
    pub static ref XMU: Result<x11_dl::xmu::Xmu, OpenError> = x11_dl::xmu::Xmu::open();
    pub static ref XRENDER: Result<x11_dl::xrender::Xrender, OpenError> =
        x11_dl::xrender::Xrender::open();
    pub static ref XCURSOR: Result<x11_dl::xcursor::Xcursor, OpenError> =
        x11_dl::xcursor::Xcursor::open();
    pub static ref GLX: Result<x11_dl::glx::Glx, OpenError> = x11_dl::glx::Glx::open();
    pub static ref XINPUT: Result<x11_dl::xinput::XInput, OpenError> =
        x11_dl::xinput::XInput::open();
    pub static ref XINPUT2: Result<x11_dl::xinput2::XInput2, OpenError> =
        x11_dl::xinput2::XInput2::open();
    pub static ref XRANDR_2_2_0: Result<x11_dl::xrandr::Xrandr_2_2_0, OpenError> =
        x11_dl::xrandr::Xrandr_2_2_0::open();
    pub static ref XRANDR: Result<x11_dl::xrandr::Xrandr, OpenError> =
        x11_dl::xrandr::Xrandr::open();
    pub static ref XF86VMODE: Result<x11_dl::xf86vmode::Xf86vmode, OpenError> =
        x11_dl::xf86vmode::Xf86vmode::open();
    pub static ref XTEST_XF86VMODE: Result<x11_dl::xtest::Xf86vmode, OpenError> =
        x11_dl::xtest::Xf86vmode::open();
    pub static ref XRECORD_XF86VMODE: Result<x11_dl::xrecord::Xf86vmode, OpenError> =
        x11_dl::xrecord::Xf86vmode::open();
    pub static ref XINERAMA: Result<x11_dl::xinerama::Xlib, OpenError> =
        x11_dl::xinerama::Xlib::open();
    pub static ref XLIB: Result<x11_dl::xlib::Xlib, OpenError> = x11_dl::xlib::Xlib::open();
    pub static ref XLIB_XCB: Result<x11_dl::xlib_xcb::Xlib_xcb, OpenError> =
        x11_dl::xlib_xcb::Xlib_xcb::open();
}

lazy_static! {
    pub static ref X11_DISPLAY: Mutex<Result<Arc<Display>, Error>> = { Mutex::new(Display::new()) };
    pub static ref DISPLAYS: Mutex<Vec<Weak<Display>>> = Mutex::new(vec![]);
    pub static ref OLD_HANDLERS: Mutex<Vec<unsafe extern "C" fn(_: *mut XDisplay, _: *mut XErrorEvent) -> raw::c_int>> =
        Mutex::new(vec![]);
    pub static ref LATEST_ERROR: Mutex<Option<Error>> = Mutex::new(None);
}

#[macro_export]
macro_rules! syms {
    ($name:ident) => {{ glutin_x11_sym::$name.as_ref().unwrap() }};
    ($($name:ident),+) => {{( $(syms!($name)),+ )}};
}

macro_rules! lsyms {
    ($name:ident) => {{ crate::$name.as_ref().unwrap() }};
    ($($name:ident),+) => {{( $(lsyms!($name)),+ )}};
}

#[derive(Debug)]
pub struct Display {
    display: *mut x11_dl::xlib::Display,
    owned: bool,
}

unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl PartialEq for Display {
    fn eq(&self, o: &Self) -> bool {
        self.display == o.display
    }
}
impl Eq for Display {}

impl Display {
    #[inline]
    fn new() -> Result<Arc<Display>, Error> {
        let xlib = lsyms!(XLIB);
        unsafe { (xlib.XInitThreads)() };
        // FIXME: old handlers...
        let old_handler = unsafe { (xlib.XSetErrorHandler)(Some(x_error_callback)) };

        match old_handler {
            Some(old_handler) if old_handler != x_error_callback => {
                OLD_HANDLERS.lock().push(old_handler);
            }
            _ => (),
        }

        // calling XOpenDisplay
        let display = unsafe {
            let display = (xlib.XOpenDisplay)(ptr::null());
            if display.is_null() {
                return Err(make_oserror!(OsError::XNotSupported(
                    XNotSupported::XOpenDisplayFailed
                )));
            }
            display
        };

        let ret = Arc::new(Display {
            display,
            owned: true,
        });

        DISPLAYS.lock().push(Arc::downgrade(&ret));

        Ok(ret)
    }

    #[inline]
    pub fn raw(&self) -> *mut raw::c_void {
        self.display as *mut _
    }

    #[inline]
    pub fn from_raw(ndisp: *mut raw::c_void) -> Arc<Display> {
        for display in &*DISPLAYS.lock() {
            if let Some(display) = display.upgrade() {
                if display.display == ndisp as *mut _ {
                    return Arc::clone(&display);
                }
            }
        }

        let ret = Arc::new(Display {
            display: ndisp as *mut _,
            owned: false,
        });

        DISPLAYS.lock().push(Arc::downgrade(&ret));

        ret
    }

    /// Checks whether an error has been triggered by the previous function calls.
    #[inline]
    pub fn check_errors(&self) -> Result<(), Error> {
        let error = LATEST_ERROR.lock().take();
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(())
        }
    }

    /// Ignores any previous error.
    #[inline]
    pub fn ignore_error(&self) {
        *LATEST_ERROR.lock() = None;
    }
}

impl Drop for Display {
    #[inline]
    fn drop(&mut self) {
        if self.owned {
            let xlib = lsyms!(XLIB);
            unsafe { (xlib.XCloseDisplay)(self.display) };
        }

        // Do some pruning
        let mut displays = DISPLAYS.lock();

        *displays = displays
            .drain(..)
            .filter(|display| display.upgrade().is_some())
            .collect();
    }
}

unsafe extern "C" fn x_error_callback(
    display_ptr: *mut x11_dl::xlib::Display,
    event: *mut x11_dl::xlib::XErrorEvent,
) -> raw::c_int {
    let xlib = lsyms!(XLIB);
    // `assume_init` is safe here because the array consists of `MaybeUninit` values,
    // which do not require initialization.
    let mut buf: [MaybeUninit<raw::c_char>; 1024] = MaybeUninit::uninit().assume_init();
    (xlib.XGetErrorText)(
        display_ptr,
        (*event).error_code as raw::c_int,
        buf.as_mut_ptr() as *mut raw::c_char,
        buf.len() as raw::c_int,
    );
    let description = CStr::from_ptr(buf.as_ptr() as *const raw::c_char).to_string_lossy();

    let error = make_oserror!(OsError::XError(XError {
        description: description.into_owned(),
        error_code: (*event).error_code,
        request_code: (*event).request_code,
        minor_code: (*event).minor_code,
    }));

    error!("X11 error: {:#?}", error);

    *LATEST_ERROR.lock() = Some(error);

    for old_handler in OLD_HANDLERS.lock().iter().rev() {
        old_handler(display_ptr, event);
    }

    // Fun fact: this return value is completely ignored.
    0
}

impl Deref for Display {
    type Target = *mut x11_dl::xlib::Display;

    fn deref(&self) -> &Self::Target {
        &self.display
    }
}

impl DerefMut for Display {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.display
    }
}
