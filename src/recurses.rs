// ncurses is sort of terrible. Let's make our own thing.
use std::os::unix::io::{RawFd, AsRawFd};
use nix::sys::termios::{Termios, tcgetattr, SetArg, tcsetattr, cfmakeraw, SpecialCharacterIndices};
use nix;

use terminfo::{self, capability};

use std::result;

use std::io::{Stdin, StdinLock, Stdout, StdoutLock};
use std::io::{self, Read, Write};

#[derive(Debug)]
pub enum Error {
    FromNix(nix::Error),
    FromIo(io::Error),
    FromTerminfo(terminfo::Error)
}

type Result<T> = result::Result<T, Error>;

impl From<nix::Error> for Error {
    fn from(orig: nix::Error) -> Error {
        Error::FromNix(orig)
    }
}

impl From<io::Error> for Error {
    fn from(orig: io::Error) -> Error {
        Error::FromIo(orig)
    }
}

impl From<terminfo::Error> for Error {
    fn from(orig: terminfo::Error) -> Error {
        Error::FromTerminfo(orig)
    }
}

pub struct Screen<'a> {
    pub input: StdinLock<'a>,
    pub output: StdoutLock<'a>,
    control_fd: RawFd,
    original_termios: Termios,
    terminfo: terminfo::Database
}

impl<'a> Drop for Screen<'a> {
    fn drop(&mut self) {
        if let Err(_) = tcsetattr(self.control_fd, SetArg::TCSANOW, &self.original_termios) {
            eprintln!("Failed to return to a sane terminal!");
        }

        if let Some(cap) = self.terminfo.get::<capability::ExitCaMode>() {
            if let Err(_) = cap.expand().to(&mut self.output) {
                eprintln!("Failed to reset the screen.")
            }
        } else {
            if let Err(_) = self.clear() {
                eprintln!("Failed to reset the screen.")
            }
        }

        if let Some(cap) = self.terminfo.get::<capability::ExitAttributeMode>() {
            if let Err(_) = cap.expand().to(&mut self.output) {
                eprintln!("Failed to reset formatting.")
            }
        }
    }
}

impl<'a> Screen<'a> {
    pub fn new(stdin: &'a Stdin, stdout: &'a Stdout) -> Result<Screen<'a>> {
        let control_fd = stdin.as_raw_fd();
        let mut ret = Screen {
            input: stdin.lock(),
            output: stdout.lock(),
            control_fd,
            original_termios: tcgetattr(control_fd)?,
            terminfo: terminfo::Database::from_env()?
        };

        let mut termios = ret.original_termios.clone();
        cfmakeraw(&mut termios);
        termios.control_chars[SpecialCharacterIndices::VMIN] = 1;
        termios.control_chars[SpecialCharacterIndices::VTIME] = 25; // TODO: ESCDELAY
        tcsetattr(ret.control_fd, SetArg::TCSANOW, &termios)?;

        // Push all current content off the edge of the screen so that we don't overwrite it.
        if let Some(cap) = ret.terminfo.get::<capability::EnterCaMode>() {
            cap.expand().to(&mut ret.output)?;
        } else {
            write!(ret.output, "\x1B[17S")?;
        }

        Ok(ret)
    }

    pub fn clear(&mut self) -> Result<()> {
        if let Some(cap) = ret.terminfo.get::<capability::ClearScreen>() {
            cap.expand().to(&mut self.output)?;
        } else {
            write!(self.output, "\x1B[2J")?;
        }
        Ok(())
    }
}
