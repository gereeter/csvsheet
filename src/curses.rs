use std::os::raw::{c_char, c_int};
use std::ffi::CStr;

pub struct Window {
    inner: ncurses::WINDOW
}

impl Drop for Window {
    fn drop(&mut self) {
        ncurses::endwin();
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Input {
    Byte(u8),
    Character(char),
    Special(i32),
}

// FIXME: error handling
impl Window {
    // Only run once
    pub unsafe fn init_screen() -> Self {
        ncurses::ll::setlocale(ncurses::LC_ALL, b"\0".as_ptr() as *const c_char);
        Window {
            inner: ncurses::initscr()
        }
    }

    pub fn set_keypad(&mut self, keypad: bool) {
        ncurses::keypad(self.inner, keypad);
    }

    pub fn refresh(&mut self) {
        ncurses::wrefresh(self.inner);
    }

    pub fn get_ch(&mut self) -> Result<Input, ()> {
        let code = ncurses::wgetch(self.inner);
        if code == ncurses::ERR {
            Err(())
        } else if code >= 0 && code < 256 {
            Ok(Input::Byte(code as u8))
        } else {
            Ok(Input::Special(code))
        }
    }

    pub fn get_cur_yx(&self) -> (i32, i32) {
        let mut y = 0;
        let mut x = 0;
        ncurses::getyx(self.inner, &mut y, &mut x);
        (y, x)
    }

    pub fn get_max_yx(&self) -> (i32, i32) {
        let mut y = 0;
        let mut x = 0;
        ncurses::getmaxyx(self.inner, &mut y, &mut x);
        (y, x)
    }

    pub fn set_clear_ok(&mut self, clearok: bool) {
        ncurses::clearok(self.inner, clearok);
    }

    pub fn erase(&mut self) {
        ncurses::werase(self.inner);
    }

    pub fn set_attrs(&mut self, attrs: ncurses::attr_t) {
        ncurses::wattr_set(self.inner, attrs, 0);
    }

    pub fn mv(&mut self, y: i32, x: i32) {
        ncurses::wmove(self.inner, y, x);
    }

    pub fn mv_add_str(&mut self, y: i32, x: i32, text: &str) {
        assert!(i32::max_value() as usize >= text.len());
        unsafe {
            ncurses::ll::mvwaddnstr(self.inner, y, x, text.as_bytes().as_ptr() as *const c_char, text.len() as i32);
        }
    }

    pub fn add_str(&mut self, text: &str) {
        assert!(i32::max_value() as usize >= text.len());
        unsafe {
            ncurses::ll::waddnstr(self.inner, text.as_bytes().as_ptr() as *const c_char, text.len() as i32);
        }
    }

    pub fn clear_to_end_of_line(&mut self) {
        ncurses::wclrtoeol(self.inner);
    }
}

pub fn get_mouse() -> Result<ncurses::MEVENT, ()> {
    let mut ret = ncurses::MEVENT {
        id: 0,
        x: 0,
        y: 0,
        z: 0,
        bstate: 0
    };
    if ncurses::getmouse(&mut ret) == ncurses::OK {
        Ok(ret)
    } else {
        Err(())
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum KeyError {
    NotDefined,
    PrefixConflict,
    NotSupported
}

// TODO: runtime detection?
#[cfg(feature = "ncurses-ext")]
extern "C" {
    fn key_defined(definition: *const c_char) -> c_int;
    fn define_key(definition: *const c_char, code: c_int) -> c_int;
}

#[cfg(feature = "ncurses-ext")]
pub fn key_code_for(definition: &CStr) -> Result<c_int, KeyError> {
    let ret = unsafe { key_defined(definition.as_ptr()) };
    if ret == 0 {
        Err(KeyError::NotDefined)
    } else if ret == -1 {
        Err(KeyError::PrefixConflict)
    } else {
        Ok(ret)
    }
}

#[cfg(not(feature = "ncurses-ext"))]
pub fn key_code_for(definition: &CStr) -> Result<c_int, KeyError> { Err(KeyError::NotSupported) }


#[cfg(feature = "ncurses-ext")]
pub unsafe fn define_key_code(definition: &CStr, code: c_int) -> Result<(), ()> {
    let ret = define_key(definition.as_ptr(), code);
    if ret == ncurses::OK {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(not(feature = "ncurses-ext"))]
pub unsafe fn define_key_code(definition: &CStr, code: c_int) -> Result<(), ()> { Err(()) }

