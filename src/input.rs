use std::io::Write;

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
    
    in_progress_codepoint: u32,
    utf8_bytes_left: usize,
    xterm_modify_key_state: XTermModifyKeyState,
    kitty_full_mode_state: KittyFullModeState

}

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

        // Hackily detect if our terminal is using XTerm-style codes and add the rest if necessary
        if curses::key_code_for(const_cstr!("\x1b[1;2D").as_cstr()) == Ok(ncurses::KEY_SLEFT) &&
           curses::key_code_for(const_cstr!("\x1b[1;2C").as_cstr()) == Ok(ncurses::KEY_SRIGHT) {
            unsafe fn define_if_necessary(def: &std::ffi::CStr, code: std::os::raw::c_int) -> Result<(), ()> {
                if curses::key_code_for(def) == Err(curses::KeyError::NotDefined) {
                    curses::define_key_code(def, code)
                } else {
                    Ok(())
                }
            }

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
            let _ = define_if_necessary(const_cstr!("\x1b[1;7H").as_cstr(), 544); // Ctrl + Alt +  Home
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

        ncurses::ungetch(ncurses::KEY_RESIZE);

        InputStream {
            _bracketed_paste: bracketed_paste_guard,
            _xterm_modify_keys: xterm_modify_other_keys_guard,
            _kitty_full_mode: kitty_full_mode_guard,

            in_progress_codepoint: 0,
            utf8_bytes_left: 0,
            xterm_modify_key_state: XTermModifyKeyState::Off,
            kitty_full_mode_state: KittyFullModeState::Off
        }
    }

    pub fn get(&mut self, window: &mut Window) -> Option<Input> {
        loop {
            let mut input = window.get_ch().ok();

            // We need to parse utf8.
            if let Some(Input::Byte(byte)) = input {
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
                    input = Some(Input::Character(std::char::from_u32(self.in_progress_codepoint).expect("BUG: Bad char cast")));
                } else {
                    continue;
                }
            }

            // Handle XTerm's modifyOtherKeys extension, parsing manually
            if let Some(Input::Special(2100)) = input {
                self.xterm_modify_key_state = XTermModifyKeyState::ParsingMode(0);
                continue;
            }
            match self.xterm_modify_key_state {
                XTermModifyKeyState::Off => { },
                XTermModifyKeyState::ParsingMode(mode_so_far) => {
                    if let Some(Input::Character(chr)) = input {
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
                    if let Some(Input::Character(chr)) = input {
                        if let Some(digit) = chr.to_digit(10) {
                            self.xterm_modify_key_state = XTermModifyKeyState::ParsingChar(mode, char_so_far * 10 + digit);
                            continue;
                        } else if chr == '~' {
                            self.xterm_modify_key_state = XTermModifyKeyState::Off;
                            if 1 <= mode {
                                let ctrl = (mode - 1) & 0b100 != 0;
                                let alt = (mode - 1) & 0b10 != 0;
                                let shift = (mode - 1) & 0b1 != 0;
                                return Some(Input::Decomposed(ctrl, alt, shift, char_so_far));
                            } else {
                                eprintln!("0 mode?");
                                continue;
                            }
                        }
                    }
                }
            }

            // Handle Kitty's full mode extension, parsing manually
            if let Some(Input::Special(2200)) = input {
                self.kitty_full_mode_state = KittyFullModeState::ParsingType;
                continue;
            }
            match self.kitty_full_mode_state {
                KittyFullModeState::Off => { },
                KittyFullModeState::ParsingType => match input {
                    Some(Input::Character('p')) => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Press);
                        continue;
                    },
                    Some(Input::Character('r')) => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Release);
                        continue;
                    },
                    Some(Input::Character('t')) => {
                        self.kitty_full_mode_state = KittyFullModeState::ParsingModifiers(KeyType::Repeat);
                        continue;
                    },
                    _ => { }
                },
                KittyFullModeState::ParsingModifiers(key_type) => {
                    if let Some(Input::Character(chr)) = input {
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
                    if let Some(Input::Character(chr)) = input {
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
                    } else if let Some(Input::Special(2201)) = input {
                        self.kitty_full_mode_state = KittyFullModeState::Off;
                        if let KeyType::Press | KeyType::Repeat = key_type {
                            let ctrl = mode & 0b100 != 0;
                            let alt = mode & 0b10 != 0;
                            let shift = mode & 0b1 != 0;
                            // FIXME: more complete translation, unify different input types
                            if 18 <= key_so_far && key_so_far <= 43 {
                                return Some(Input::Decomposed(ctrl, alt, shift, 'a' as u32 + key_so_far - 18));
                            } else if key_so_far == 56 { // Right
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_RIGHT));
                                } else {
                                    return Some(Input::Special(564 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 57 { // Left
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_LEFT));
                                } else {
                                    return Some(Input::Special(549 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 58 { // Down
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_DOWN));
                                } else {
                                    return Some(Input::Special(527 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 59 { // Up
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_UP));
                                } else {
                                    return Some(Input::Special(570 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 60 { // PageUp
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_PPAGE));
                                } else {
                                    return Some(Input::Special(559 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 61 { // PageDown
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_NPAGE));
                                } else {
                                    return Some(Input::Special(554 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 62 { // Home
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_HOME));
                                } else {
                                    return Some(Input::Special(538 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 63 { // End
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_END));
                                } else {
                                    return Some(Input::Special(532 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 55 { // Delete
                                if !ctrl && !alt {
                                    return Some(Input::Special(ncurses::KEY_DC));
                                } else {
                                    return Some(Input::Special(521 + (mode & 0b111) as i32));
                                }
                            } else if key_so_far == 50 { // Escape
                                return Some(Input::Character('\u{1b}'));
                            } else if key_so_far == 69 { // F1
                                return Some(Input::Special(ncurses::KEY_F1));
                            } else {
                                return Some(Input::Decomposed(ctrl, alt, shift, key_so_far));
                            }
                        } else {
                            continue;
                        }
                    }
                }
            }

            return input;
        }
    }
}
