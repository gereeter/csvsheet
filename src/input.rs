use std::io::Write;

use const_cstr::ConstCStr;

use curses::{self, Window, Input};

fn write_now(data: &[u8]) -> Result<(), std::io::Error> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(data)?;
    lock.flush()?;
    Ok(())
}

struct BracketedPaste {
    _priv: ()
}

impl Drop for BracketedPaste {
    fn drop(&mut self) {
        let _ = write_now(b"\x1b[?2004l");
    }
}

impl BracketedPaste {
    fn start() -> Option<BracketedPaste> {
        write_now(b"\x1b[?2004h").ok()?;
        Some(BracketedPaste { _priv: () })
    }
}

struct XTermModifyOtherKeys {
    _priv: ()
}

impl Drop for XTermModifyOtherKeys {
    fn drop(&mut self) {
        let _ = write_now(b"\x1b[>4n");
    }
}

impl XTermModifyOtherKeys {
    fn start() -> Option<XTermModifyOtherKeys> {
        write_now(b"\x1b[>4;2m").ok()?;
        Some(XTermModifyOtherKeys { _priv: () })
    }
}

enum XTermModifyKeyState {
    Off,
    ParsingMode(u32),
    ParsingChar(u32, u32)
}

struct KittyFullMode {
    _priv: ()
}

impl Drop for KittyFullMode {
    fn drop(&mut self) {
        let _ = write_now(b"\x1b[?2017l");
    }
}

impl KittyFullMode {
    fn start() -> Option<KittyFullMode> {
        write_now(b"\x1b[?2017h").ok()?;
        Some(KittyFullMode { _priv: () })
    }
}

#[derive(Copy, Clone, Debug)]
enum KeyType {
    Press,
    Release,
    Repeat
}

enum KittyFullModeState {
    Off,
    ParsingType,
    ParsingModifiers(KeyType),
    ParsingKey(KeyType, u32, u32)
}

pub struct InputStream {
    _bracketed_paste: Option<BracketedPaste>,
    _xterm_modify_keys: Option<XTermModifyOtherKeys>,
    _kitty_full_mode: Option<KittyFullMode>,

    extra_bound_keys: Vec<(i32, Input)>,
    
    in_progress_codepoint: u32,
    utf8_bytes_left: usize,
    xterm_modify_key_state: XTermModifyKeyState,
    kitty_full_mode_state: KittyFullModeState

}

const fn make_input(mode: i32, key: i32) -> Input {
    Input::Decomposed(mode & 0b100 != 0, mode & 0b10 != 0, mode & 0b1 != 0, key)
}

unsafe fn define_if_necessary(def: &std::ffi::CStr, code: std::os::raw::c_int) -> Result<(), ()> {
    if curses::key_code_for(def) == Err(curses::KeyError::NotDefined) {
        curses::define_key_code(def, code)
    } else {
        Ok(())
    }
}

const KNOWN_EXTRA_TERM_CAPABILITIES: &'static [(ConstCStr, Input)] = &[
    (const_cstr!("kDC3"), make_input(2, ncurses::KEY_DC)),
    (const_cstr!("kDC4"), make_input(3, ncurses::KEY_DC)),
    (const_cstr!("kDC5"), make_input(4, ncurses::KEY_DC)),
    (const_cstr!("kDC6"), make_input(5, ncurses::KEY_DC)),
    (const_cstr!("kDC7"), make_input(6, ncurses::KEY_DC)),
    (const_cstr!("kDC8"), make_input(7, ncurses::KEY_DC)),

    (const_cstr!("kLFT3"), make_input(2, ncurses::KEY_LEFT)),
    (const_cstr!("kLFT4"), make_input(3, ncurses::KEY_LEFT)),
    (const_cstr!("kLFT5"), make_input(4, ncurses::KEY_LEFT)),
    (const_cstr!("kLFT6"), make_input(5, ncurses::KEY_LEFT)),
    (const_cstr!("kLFT7"), make_input(6, ncurses::KEY_LEFT)),
    (const_cstr!("kLFT8"), make_input(7, ncurses::KEY_LEFT)),

    (const_cstr!("kRIT3"), make_input(2, ncurses::KEY_RIGHT)),
    (const_cstr!("kRIT4"), make_input(3, ncurses::KEY_RIGHT)),
    (const_cstr!("kRIT5"), make_input(4, ncurses::KEY_RIGHT)),
    (const_cstr!("kRIT6"), make_input(5, ncurses::KEY_RIGHT)),
    (const_cstr!("kRIT7"), make_input(6, ncurses::KEY_RIGHT)),
    (const_cstr!("kRIT8"), make_input(7, ncurses::KEY_RIGHT)),

    (const_cstr!("kUP3"), make_input(2, ncurses::KEY_UP)),
    (const_cstr!("kUP4"), make_input(3, ncurses::KEY_UP)),
    (const_cstr!("kUP5"), make_input(4, ncurses::KEY_UP)),
    (const_cstr!("kUP6"), make_input(5, ncurses::KEY_UP)),
    (const_cstr!("kUP7"), make_input(6, ncurses::KEY_UP)),
    (const_cstr!("kUP8"), make_input(7, ncurses::KEY_UP)),

    (const_cstr!("kDN3"), make_input(2, ncurses::KEY_DOWN)),
    (const_cstr!("kDN4"), make_input(3, ncurses::KEY_DOWN)),
    (const_cstr!("kDN5"), make_input(4, ncurses::KEY_DOWN)),
    (const_cstr!("kDN6"), make_input(5, ncurses::KEY_DOWN)),
    (const_cstr!("kDN7"), make_input(6, ncurses::KEY_DOWN)),
    (const_cstr!("kDN8"), make_input(7, ncurses::KEY_DOWN)),

    (const_cstr!("kHOM3"), make_input(2, ncurses::KEY_HOME)),
    (const_cstr!("kHOM4"), make_input(3, ncurses::KEY_HOME)),
    (const_cstr!("kHOM5"), make_input(4, ncurses::KEY_HOME)),
    (const_cstr!("kHOM6"), make_input(5, ncurses::KEY_HOME)),
    (const_cstr!("kHOM7"), make_input(6, ncurses::KEY_HOME)),
    (const_cstr!("kHOM8"), make_input(7, ncurses::KEY_HOME)),

    (const_cstr!("kEND3"), make_input(2, ncurses::KEY_END)),
    (const_cstr!("kEND4"), make_input(3, ncurses::KEY_END)),
    (const_cstr!("kEND5"), make_input(4, ncurses::KEY_END)),
    (const_cstr!("kEND6"), make_input(5, ncurses::KEY_END)),
    (const_cstr!("kEND7"), make_input(6, ncurses::KEY_END)),
    (const_cstr!("kEND8"), make_input(7, ncurses::KEY_END)),

    (const_cstr!("kPRV3"), make_input(2, ncurses::KEY_PPAGE)),
    (const_cstr!("kPRV4"), make_input(3, ncurses::KEY_PPAGE)),
    (const_cstr!("kPRV5"), make_input(4, ncurses::KEY_PPAGE)),
    (const_cstr!("kPRV6"), make_input(5, ncurses::KEY_PPAGE)),
    (const_cstr!("kPRV7"), make_input(6, ncurses::KEY_PPAGE)),
    (const_cstr!("kPRV8"), make_input(7, ncurses::KEY_PPAGE)),

    (const_cstr!("kNXT3"), make_input(2, ncurses::KEY_NPAGE)),
    (const_cstr!("kNXT4"), make_input(3, ncurses::KEY_NPAGE)),
    (const_cstr!("kNXT5"), make_input(4, ncurses::KEY_NPAGE)),
    (const_cstr!("kNXT6"), make_input(5, ncurses::KEY_NPAGE)),
    (const_cstr!("kNXT7"), make_input(6, ncurses::KEY_NPAGE)),
    (const_cstr!("kNXT8"), make_input(7, ncurses::KEY_NPAGE)),
];

impl InputStream {
    pub unsafe fn init(window: &mut Window) -> Self {
        window.set_keypad(true);
        ncurses::raw();
        ncurses::noecho();
        ncurses::mousemask(ncurses::ALL_MOUSE_EVENTS as ncurses::mmask_t, None);
        // TODO: consider behaviour around double, triple clicks
        ncurses::mouseinterval(0); // We care about up/down, not clicks

        // Start bracketed paste mode, but only if we can successfully handle the brackets
        // TODO: Should we query support first?
        let bracketed_paste_guard = if curses::define_key_code(const_cstr!("\x1b[200~").as_cstr(), 2000).is_ok() &&
                                       curses::define_key_code(const_cstr!("\x1b[201~").as_cstr(), 2001).is_ok() {
            BracketedPaste::start()
        } else {
            None
        };

        let xterm_modify_other_keys_guard = if curses::define_key_code(const_cstr!("\x1b[27;").as_cstr(), 2100).is_ok() {
            XTermModifyOtherKeys::start()
        } else {
            None
        };

        // TODO: Should we query support first?
        let kitty_full_mode_guard = if curses::define_key_code(const_cstr!("\x1b_K").as_cstr(), 2200).is_ok() &&
                                       curses::define_key_code(const_cstr!("\x1b\\").as_cstr(), 2201).is_ok() {
            KittyFullMode::start()
        } else {
            None
        };

        // We use Esc heavily and modern computers are quite fast, so unless the user has overridden it directly,
        // set ESCDELAY to a small 25ms. The normal default of 1 second is too high.
        if std::env::var_os("ESCDELAY").is_none() {
            ncurses::set_escdelay(25);
        }

        let mut extra_bound_keys = Vec::new();
        for &(name, inp) in KNOWN_EXTRA_TERM_CAPABILITIES {
            if let Some(description) = curses::get_terminfo_string(name.as_cstr()) {
                if let Ok(code) = curses::key_code_for(description) {
                    extra_bound_keys.push((code, inp));
                }
            }
        }

/*
        // Hackily detect if our terminal is using XTerm-style codes and add the rest if necessary
        if curses::key_code_for(const_cstr!("\x1b[1;2D").as_cstr()) == Ok(ncurses::KEY_SLEFT) &&
           curses::key_code_for(const_cstr!("\x1b[1;2C").as_cstr()) == Ok(ncurses::KEY_SRIGHT) {
            let _ = define_if_necessary(const_cstr!("\x1b[1;5A").as_cstr(), 574); // Ctrl + Up
            let _ = define_if_necessary(const_cstr!("\x1b[1;5B").as_cstr(), 531); // Ctrl + Down
            let _ = define_if_necessary(const_cstr!("\x1b[1;5C").as_cstr(), 568); // Ctrl + Right
            let _ = define_if_necessary(const_cstr!("\x1b[1;5D").as_cstr(), 553); // Ctrl + Left
            let _ = define_if_necessary(const_cstr!("\x1b[1;5H").as_cstr(), 542); // Ctrl + Home
            let _ = define_if_necessary(const_cstr!("\x1b[1;5F").as_cstr(), 536); // Ctrl + End

            let _ = define_if_necessary(const_cstr!("\x1b[1;3A").as_cstr(), 572); // Alt + Up
            let _ = define_if_necessary(const_cstr!("\x1b[1;3B").as_cstr(), 529); // Alt + Down
            let _ = define_if_necessary(const_cstr!("\x1b[1;3C").as_cstr(), 566); // Alt + Right
            let _ = define_if_necessary(const_cstr!("\x1b[1;3D").as_cstr(), 551); // Alt + Left
            let _ = define_if_necessary(const_cstr!("\x1b[1;3H").as_cstr(), 540); // Alt + Home
            let _ = define_if_necessary(const_cstr!("\x1b[1;3F").as_cstr(), 534); // Alt + End

            let _ = define_if_necessary(const_cstr!("\x1b[1;7A").as_cstr(), 576); // Ctrl + Alt + Up
            let _ = define_if_necessary(const_cstr!("\x1b[1;7B").as_cstr(), 533); // Ctrl + Alt + Down
            let _ = define_if_necessary(const_cstr!("\x1b[1;7C").as_cstr(), 570); // Ctrl + Alt + Right
            let _ = define_if_necessary(const_cstr!("\x1b[1;7D").as_cstr(), 555); // Ctrl + Alt + Left
            let _ = define_if_necessary(const_cstr!("\x1b[1;7H").as_cstr(), 544); // Ctrl + Alt + Home
            let _ = define_if_necessary(const_cstr!("\x1b[1;7F").as_cstr(), 538); // Ctrl + Alt + End

            if curses::key_code_for(const_cstr!("\x1b[3~").as_cstr()) == Ok(ncurses::KEY_DC) &&
               curses::key_code_for(const_cstr!("\x1b[5~").as_cstr()) == Ok(ncurses::KEY_PPAGE) &&
               curses::key_code_for(const_cstr!("\x1b[6~").as_cstr()) == Ok(ncurses::KEY_NPAGE) {
                let _ = define_if_necessary(const_cstr!("\x1b[3;5~").as_cstr(), 525); // Ctrl + Delete
                let _ = define_if_necessary(const_cstr!("\x1b[5;5~").as_cstr(), 563); // Ctrl + PageUp
                let _ = define_if_necessary(const_cstr!("\x1b[6;5~").as_cstr(), 558); // Ctrl + PageDown

                let _ = define_if_necessary(const_cstr!("\x1b[3;3~").as_cstr(), 523); // Alt + Delete
                let _ = define_if_necessary(const_cstr!("\x1b[5;3~").as_cstr(), 561); // Alt + PageUp
                let _ = define_if_necessary(const_cstr!("\x1b[6;3~").as_cstr(), 556); // Alt + PageDown

                let _ = define_if_necessary(const_cstr!("\x1b[3;7~").as_cstr(), 527); // Ctrl + Alt + Delete
                let _ = define_if_necessary(const_cstr!("\x1b[5;7~").as_cstr(), 565); // Ctrl + Alt + PageUp
                let _ = define_if_necessary(const_cstr!("\x1b[6;7~").as_cstr(), 560); // Ctrl + Alt + PageDown
            }
        }
*/

        ncurses::ungetch(ncurses::KEY_RESIZE);

        InputStream {
            _bracketed_paste: bracketed_paste_guard,
            _xterm_modify_keys: xterm_modify_other_keys_guard,
            _kitty_full_mode: kitty_full_mode_guard,

            extra_bound_keys: extra_bound_keys,

            in_progress_codepoint: 0,
            utf8_bytes_left: 0,
            xterm_modify_key_state: XTermModifyKeyState::Off,
            kitty_full_mode_state: KittyFullModeState::Off
        }
    }

    pub fn get(&mut self, window: &mut Window) -> Result<Input, ()> {
        loop {
            let mut input = window.get_ch()?;

            // We need to parse utf8.
            if let Input::Byte(byte) = input {
                if self.utf8_bytes_left == 0 {
                    // New character
                    if byte >> 7 == 0b0 {
                        self.utf8_bytes_left = 0;
                        self.in_progress_codepoint = (byte & 0x7f) as u32;
                    } else if byte >> 5 == 0b110 {
                        self.utf8_bytes_left = 1;
                        self.in_progress_codepoint = (byte & 0x1f) as u32;
                    } else if byte >> 4 == 0b1110 {
                        self.utf8_bytes_left = 2;
                        self.in_progress_codepoint = (byte & 0x0f) as u32;
                    } else if byte >> 3 == 0b11110 {
                        self.utf8_bytes_left = 3;
                        self.in_progress_codepoint = (byte & 0x07) as u32;
                    } else {
                        // FIXME: this should not crash
                        panic!("Bad unicode: first byte {:x}", byte);
                    }
                } else {
                    self.utf8_bytes_left -= 1;
                    self.in_progress_codepoint = (self.in_progress_codepoint << 6) | ((byte & 0x3f) as u32);
                }
                if self.utf8_bytes_left == 0 {
                    input = Input::Character(std::char::from_u32(self.in_progress_codepoint).expect("BUG: Bad char cast"));
                } else {
                    continue;
                }
            }

            // Translate keys bound to non-standard terminfo entries
            if let Input::Special(code) = input {
                for &(possible_code, possible_inp) in &self.extra_bound_keys {
                    if possible_code == code {
                        return Ok(possible_inp);
                    }
                }
            }

            // Translate various known special keys to a decomposed form
            match input {
                Input::Special(ncurses::KEY_LEFT)   => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_LEFT)),
                Input::Special(ncurses::KEY_SLEFT)  => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_LEFT)),
                Input::Special(ncurses::KEY_RIGHT)  => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_RIGHT)),
                Input::Special(ncurses::KEY_SRIGHT) => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_RIGHT)),
                Input::Special(ncurses::KEY_UP)     => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_UP)),
                Input::Special(ncurses::KEY_SR)     => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_UP)),
                Input::Special(ncurses::KEY_DOWN)   => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_DOWN)),
                Input::Special(ncurses::KEY_SF)     => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_DOWN)),
                Input::Special(ncurses::KEY_HOME)   => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_HOME)),
                Input::Special(ncurses::KEY_SHOME)  => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_HOME)),
                Input::Special(ncurses::KEY_END)    => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_END)),
                Input::Special(ncurses::KEY_SEND)   => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_END)),
                Input::Special(ncurses::KEY_PPAGE)  => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_PPAGE)),
                Input::Special(ncurses::KEY_NPAGE)  => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_NPAGE)),
                Input::Special(ncurses::KEY_DC)     => return Ok(Input::Decomposed(false, false, false, ncurses::KEY_DC)),
                Input::Special(ncurses::KEY_SDC)    => return Ok(Input::Decomposed(false, false, true, ncurses::KEY_DC)),
                Input::Special(ncurses::KEY_BTAB)   => return Ok(Input::Decomposed(false, false, true, '\t' as i32)),
                Input::Character(chr) if (chr as u32) < 27 && chr != '\t' && chr != '\n' && chr != '\u{8}'
                    => return Ok(Input::Decomposed(true, false, false, chr as i32 + 96)),
                Input::Character(chr) if (chr as u32) > 128 && (chr as u32) < 155 // TODO: Consider whitelist? Cancel is sometimes used for Backspace
                    => return Ok(Input::Decomposed(true, true, false, chr as i32 - 32)),
                _ => { }
            }

            // Handle XTerm's modifyOtherKeys extension, parsing manually
            if let Input::Special(2100) = input {
                self.xterm_modify_key_state = XTermModifyKeyState::ParsingMode(0);
                continue;
            }
            match self.xterm_modify_key_state {
                XTermModifyKeyState::Off => { },
                XTermModifyKeyState::ParsingMode(mode_so_far) => {
                    if let Input::Character(chr) = input {
                        if let Some(digit) = chr.to_digit(10) {
                            self.xterm_modify_key_state = XTermModifyKeyState::ParsingMode(mode_so_far * 10 + digit);
                            continue;
                        } else if chr == ';' {
                            self.xterm_modify_key_state = XTermModifyKeyState::ParsingChar(mode_so_far, 0);
                            continue;
                        }
                    }
                },
                XTermModifyKeyState::ParsingChar(mode, char_so_far) => {
                    if let Input::Character(chr) = input {
                        if let Some(digit) = chr.to_digit(10) {
                            self.xterm_modify_key_state = XTermModifyKeyState::ParsingChar(mode, char_so_far * 10 + digit);
                            continue;
                        } else if chr == '~' {
                            self.xterm_modify_key_state = XTermModifyKeyState::Off;
                            if 1 <= mode {
                                if mode == 2 { // Just shift - XTerm doesn't pass through all shifted characters, though it does some
                                    return Ok(Input::Character(std::char::from_u32(char_so_far).unwrap()));
                                } else {
                                    return Ok(make_input(mode as i32 - 1, char_so_far as i32))
                                };
                            } else {
                                eprintln!("0 mode?");
                                continue;
                            }
                        }
                    }
                }
            }

            // Handle Kitty's full mode extension, parsing manually
            if let Input::Special(2200) = input {
                self.kitty_full_mode_state = KittyFullModeState::ParsingType;
                continue;
            }
            match self.kitty_full_mode_state {
                KittyFullModeState::Off => { },
                KittyFullModeState::ParsingType => match input {
                    Input::Character('p') => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Press);
                        continue;
                    },
                    Input::Character('r') => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Release);
                        continue;
                    },
                    Input::Character('t') => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Repeat);
                        continue;
                    },
                    _ => { }
                },
                KittyFullModeState::ParsingModifiers(key_type) => {
                    if let Input::Character(chr) = input {
                        // Decode base 64
                        let decoded = if 'A' <= chr && chr <= 'Z' {
                            Some(chr as u32 - 'A' as u32)
                        } else if 'a' <= chr && chr <= 'z' {
                            Some(chr as u32 - 'a' as u32 + 26)
                        } else if '0' <= chr && chr <= '9' {
                            Some(chr as u32 - '0' as u32 + 52)
                        } else if chr == '+' {
                            Some(62)
                        } else if chr == '/' {
                            Some(63)
                        } else {
                            None
                        };
                        if let Some(mode) = decoded {
                            self.kitty_full_mode_state = KittyFullModeState::ParsingKey(key_type, mode, 0);
                            continue;
                        }
                    }
                },
                KittyFullModeState::ParsingKey(key_type, mode, key_so_far) => {
                    if let Input::Character(chr) = input {
                        let decoded = if 'A' <= chr && chr <= 'Z' {
                            Some(chr as u32 - 'A' as u32)
                        } else if 'a' <= chr && chr <= 'z' {
                            Some(chr as u32 - 'a' as u32 + 26)
                        } else if '0' <= chr && chr <= '9' {
                            Some(chr as u32 - '0' as u32 + 52)
                        } else {
                            ".-:+=^!/*?&<>()[]{}@%$#".chars().position(|c| c == chr).map(|i| i as u32 + 62)
                        };
                        if let Some(value) = decoded {
                            self.kitty_full_mode_state = KittyFullModeState::ParsingKey(key_type, mode, key_so_far * 85 + value);
                            continue;
                        }
                    } else if let Input::Special(2201) = input {
                        self.kitty_full_mode_state = KittyFullModeState::Off;
                        if let KeyType::Press | KeyType::Repeat = key_type {
                            let mode = mode as i32;
                            // FIXME: more complete translation, unify different input types
                            match key_so_far {
                                18..=43 => return Ok(make_input(mode, 'a' as i32 + key_so_far as i32 - 18)),
                                55 => return Ok(make_input(mode, ncurses::KEY_DC)),
                                56 => return Ok(make_input(mode, ncurses::KEY_RIGHT)),
                                57 => return Ok(make_input(mode, ncurses::KEY_LEFT)),
                                58 => return Ok(make_input(mode, ncurses::KEY_DOWN)),
                                59 => return Ok(make_input(mode, ncurses::KEY_UP)),
                                60 => return Ok(make_input(mode, ncurses::KEY_PPAGE)),
                                61 => return Ok(make_input(mode, ncurses::KEY_NPAGE)),
                                62 => return Ok(make_input(mode, ncurses::KEY_HOME)),
                                63 => return Ok(make_input(mode, ncurses::KEY_END)),
                                50 => return Ok(Input::Character('\u{1b}')), // Escape
                                69 => return Ok(Input::Special(ncurses::KEY_F1)),
                                _ => return Ok(make_input(mode, key_so_far as i32 + 600))
                            }
                        } else {
                            continue;
                        }
                    }
                }
            }

            return Ok(input);
        }
    }
}
