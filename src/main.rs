#![feature(nll)] // TODO: consider stabilization?
// nll is jusr convenience

extern crate pancurses;
extern crate csv;
extern crate unicode_segmentation;
extern crate unicode_width;
extern crate terminfo;
extern crate clap;
extern crate tempfile;

#[cfg(unix)]
extern crate nix;

mod indexed_vec;
mod stack;
//mod recurses;

use indexed_vec::{Idx, IndexVec};
use stack::RefillingStack;

use std::cmp;
use std::iter;
use std::path::Path;
use std::ptr;

use csv::ReaderBuilder;
use pancurses::{initscr, cbreak, noecho, endwin, Window};
use pancurses::{Attribute, Attributes, A_NORMAL};
use pancurses::{Input, getmouse, mousemask, mouseinterval, ALL_MOUSE_EVENTS};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT, Ordering};
#[cfg(unix)]
use nix::libc::c_int;
#[cfg(unix)]
use nix::sys::signal::{Signal, SigAction, SigHandler, SaFlags, SigSet};
#[cfg(unix)]
use nix::sys::termios::{tcgetattr, tcsetattr, SetArg, IXON, cfmakeraw};

#[derive(Clone)]
struct ShapedString {
    text: String,
    // Byte index, column start
    grapheme_info: Vec<(usize, usize)>,
    total_width: usize
}

impl ShapedString {
    fn new() -> Self {
        ShapedString {
            text: String::new(),
            grapheme_info: Vec::new(),
            total_width: 0
        }
    }

    fn from_string(text: String) -> Self {
        let mut this = ShapedString {
            text: text,
            grapheme_info: Vec::new(),
            total_width: 0
        };
        this.reshape();
        this
    }

    fn display_column(&self, grapheme_index: usize) -> usize {
        self.grapheme_info.get(grapheme_index)
                          .map(|&(_, col)| col)
                          .unwrap_or(self.total_width)
    }

    fn index_of_display_column(&self, display_column: usize) -> usize {
        if self.total_width <= display_column {
            self.grapheme_info.len()
        } else {
            self.grapheme_info.iter()
                              .rposition(|&(_, col)| col <= display_column)
                              .unwrap_or(0)
        }
    }

    fn at_beginning(&self, position: &TextPosition) -> bool {
        position.grapheme_index == 0
    }

    fn at_end(&self, position: &TextPosition) -> bool {
        position.grapheme_index >= self.grapheme_info.len()
    }

    fn move_left(&self, position: &mut TextPosition) {
        position.grapheme_index -= 1;
        position.display_column = self.grapheme_info[position.grapheme_index].1;
    }

    fn move_right(&self, position: &mut TextPosition) {
        position.grapheme_index += 1;
        position.display_column = self.display_column(position.grapheme_index);
    }

    fn move_vert(&self, position: &mut TextPosition) {
        position.grapheme_index = self.index_of_display_column(position.display_column);
    }

    fn delete_left(&mut self, position: &mut TextPosition) {
        // TODO: RTL text
        let removed_grapheme_start = self.grapheme_info[position.grapheme_index - 1].0;
        let removed_grapheme_end = self.grapheme_info.get(position.grapheme_index).map(|&(idx, _)| idx).unwrap_or(self.text.len());
        self.text.replace_range(removed_grapheme_start..removed_grapheme_end, "");
        // TODO: reuse information?
        self.reshape();
        position.grapheme_index -= 1;
        position.display_column = self.display_column(position.grapheme_index);
    }

    fn delete_right(&mut self, position: &mut TextPosition) {
        // TODO: RTL text
        let removed_grapheme_start = self.grapheme_info[position.grapheme_index].0;
        let removed_grapheme_end = self.grapheme_info.get(position.grapheme_index + 1).map(|&(idx, _)| idx).unwrap_or(self.text.len());
        self.text.replace_range(removed_grapheme_start..removed_grapheme_end, "");
        // TODO: reuse information?
        self.reshape();
        position.display_column = self.display_column(position.grapheme_index);
    }

    fn insert(&mut self, position: &mut TextPosition, chr: char) {
        let insertion_point = self.grapheme_info.get(position.grapheme_index).map(|&(idx, _)| idx).unwrap_or(self.text.len());
        let tail_bytes = self.text.len() - insertion_point;
        self.text.insert(insertion_point, chr);
        self.reshape();
        // TODO: RTL text
        position.grapheme_index = if tail_bytes == 0 {
            self.grapheme_info.len()
        } else {
            self.grapheme_info
                .iter()
                .rposition(|&(idx, _)| idx + tail_bytes <= self.text.len())
                .unwrap_or(0)
        };
        position.display_column = self.display_column(position.grapheme_index);
    }

    fn reshape(&mut self) {
        self.grapheme_info.clear();
        let mut column = 0;
        for (index, grapheme) in self.text.grapheme_indices(true) {
            self.grapheme_info.push((index, column));
            // TODO: Tabs?
            column += UnicodeWidthStr::width(grapheme);
        }
        self.total_width = column;
    }
}

#[derive(Copy, Clone)]
struct TextPosition {
    // TODO: Use a GraphemeCursor to avoid precomputation?
    grapheme_index: usize,
    display_column: usize
}

impl TextPosition {
    fn beginning() -> Self {
        TextPosition {
            grapheme_index: 0,
            display_column: 0
        }
    }

    fn end(str: &ShapedString) -> Self {
        TextPosition {
            grapheme_index: str.grapheme_info.len(),
            display_column: str.total_width
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct RowId(usize);

impl Idx for RowId {
    fn new(idx: usize) -> Self {
        RowId(idx)
    }

    fn index(self) -> usize {
        self.0
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct ColId(usize);

impl Idx for ColId {
    fn new(idx: usize) -> Self {
        ColId(idx)
    }

    fn index(self) -> usize {
        self.0
    }
}


struct Document {
    modified: bool,
    delimiter: u8,
    // TODO: more editable data structure?
    data: IndexVec<RowId, IndexVec<ColId, ShapedString>>,
    views: RefillingStack<View>,
    row_numbers: IndexVec<RowId, usize>,
    col_numbers: IndexVec<ColId, usize>,
    column_widths: IndexVec<ColId, usize>,
}

impl Document {
    fn new(mut data: IndexVec<RowId, IndexVec<ColId, ShapedString>>, delimiter: u8) -> Self {
        // All documents must have at least on cell
        if data.is_empty() {
            data.push(IndexVec::new());
        }
        if data[RowId(0)].is_empty() {
            data[RowId(0)].push(ShapedString::new());
        }

        // Fix raggedness
        let width = data.iter().map(|row| row.len()).max().unwrap_or(0);
        let height = data.len();
        for row in &mut data {
            let padding = width - row.len();
            row.extend(iter::repeat(ShapedString::new()).take(padding));
        }

        Document {
            modified: false, // TODO: consider marking as true for raggedness?
            delimiter: delimiter,
            data: data,
            views: RefillingStack::new(View {
                headers: 1, // TODO: provide a way to customize this?
                rows: (0..height).map(RowId).collect(),
                cols: (0..width).map(ColId).collect(),
                ty: ViewType::Base
            }),
            row_numbers: (0..height).collect(),
            col_numbers: (0..width).collect(),
            column_widths: IndexVec::from_vec(vec![0; width])
        }
    }

    fn width(&self) -> usize {
        self.col_numbers.len()
    }

    fn height(&self) -> usize {
        self.row_numbers.len()
    }

    fn insert_col(&mut self, col_num: usize) -> ColId {
        self.modified = true;
        for row in &mut self.data {
            row.push(ShapedString::new());
        }
        for num in &mut self.col_numbers {
            if *num >= col_num {
                *num += 1;
            }
        }
        self.col_numbers.push(col_num)
    }

    fn insert_row(&mut self, row_num: usize) -> RowId {
        self.modified = true;
        self.data.push(IndexVec::from_vec(vec![ShapedString::new(); self.width()]));
        for num in &mut self.row_numbers {
            if *num >= row_num {
                *num += 1;
            }
        }
        self.row_numbers.push(row_num)
    }

    fn save_to(&mut self, path: &Path) -> Result<(), std::io::Error> {
        let mut temp_file = tempfile::NamedTempFile::new_in(path.parent().ok_or(std::io::ErrorKind::Other)?)?;
        {
            let mut writer = csv::WriterBuilder::new().delimiter(self.delimiter)
                                                      .from_writer(&mut temp_file);
            for &row_id in &self.views.base().rows {
                writer.write_record(self.views.base().cols.iter().map(|&col_id| self.data[row_id][col_id].text.as_bytes()))?;
            }
        }
        temp_file.as_file().sync_data()?;
        // Close the file
        let temp_path = temp_file.into_temp_path();

        temp_path.persist(path)?;
        self.modified = false;
        Ok(())
    }
}

#[derive(Clone)]
struct View {
    headers: usize,
    rows: Vec<RowId>,
    cols: Vec<ColId>,
    ty: ViewType
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ViewType {
    Filter,
    Hide,
    Base
}

struct Cursor {
    row_index: usize,
    col_index: usize,
    cell_display_column: usize,
    in_cell_pos: TextPosition
}

enum UndoOp {
    Edit {
        row_id: RowId,
        col_id: ColId,
        before_in_cell_pos: TextPosition,
        after_in_cell_pos: TextPosition,
        before_text: ShapedString
    }
}

impl UndoOp {
    fn apply_to(self, document: &mut Document, cursor: &mut Cursor) -> UndoOp {
        document.modified = true;
        match self {
            UndoOp::Edit { row_id, col_id, before_in_cell_pos, after_in_cell_pos, before_text } => {
                let after_text = std::mem::replace(&mut document.data[row_id][col_id], before_text);

                // TODO: Is popping views until we find the cell the correct behavior here?
                let (row_index, col_index) = loop {
                    let maybe_row_idx = document.views.top().rows.iter().position(|&cand_row_id| cand_row_id == row_id);
                    let maybe_col_idx = document.views.top().cols.iter().position(|&cand_col_id| cand_col_id == col_id);
                    if let (Some(row_idx), Some(col_idx)) = (maybe_row_idx, maybe_col_idx) {
                        break (row_idx, col_idx);
                    } else {
                        assert!(!document.views.is_at_base());
                        document.views.pop();
                    }
                };
                *cursor = Cursor {
                    row_index: row_index,
                    col_index: col_index,
                    cell_display_column: document.views.top().cols[..col_index].iter().map(|&pre_col_id| document.column_widths[pre_col_id] + 3).sum(),
                    in_cell_pos: before_in_cell_pos
                };

                UndoOp::Edit {
                    row_id: row_id,
                    col_id: col_id,
                    before_in_cell_pos: after_in_cell_pos,
                    after_in_cell_pos: before_in_cell_pos,
                    before_text: after_text
                }
            },
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum EditType {
    Insert,
    Delete
}

struct UndoState {
    undo_stack: Vec<UndoOp>,
    redo_stack: Vec<UndoOp>,
    current_edit_type: Option<EditType>,
    pristine_state: Option<usize>
}

impl UndoState {
    fn new() -> UndoState {
        UndoState {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_edit_type: None,
            pristine_state: Some(0)
        }
    }

    fn is_pristine(&self) -> bool {
        self.pristine_state == Some(self.undo_stack.len())
    }

    fn prepare_edit(&mut self, edit_type: Option<EditType>, document: &Document, cursor: &Cursor) {
        if edit_type.is_some() {
            self.redo_stack.clear();
            if let Some(height) = self.pristine_state {
                if height > self.undo_stack.len() {
                    self.pristine_state = None;
                }
            }
        }

        if edit_type != self.current_edit_type {
            self.current_edit_type = edit_type;
            if let Some(&mut UndoOp::Edit { ref mut after_in_cell_pos, .. }) = self.undo_stack.last_mut() {
                *after_in_cell_pos = cursor.in_cell_pos;
            }
            if edit_type.is_some() {
                let row_id = document.views.top().rows[cursor.row_index];
                let col_id = document.views.top().cols[cursor.col_index];
                self.undo_stack.push(UndoOp::Edit {
                    row_id: row_id,
                    col_id: col_id,
                    before_in_cell_pos: cursor.in_cell_pos,
                    after_in_cell_pos: cursor.in_cell_pos,
                    before_text: document.data[row_id][col_id].clone()
                });
            }
        }
    }
}

/*
fn draw_clipped_string_ascii(window: &Window, x: usize, y: usize, left: usize, right: usize, value: &str) {
    // Fast path early out
    if x >= right || x + value.len() <= left {
        return;
    }

    // TODO: Consider binary search
    let start_byte = left.saturating_sub(x);
    let start_col = cmp::max(x, left);
    let end_byte = cmp::min(value.len(), right - x);
    window.mvaddstr(y as i32, (start_col - left) as i32, &value[start_byte..end_byte]);
}
*/

// TODO: right-to-left text?
fn draw_clipped_string(window: &Window, x: usize, y: usize, left: usize, right: usize, value: &ShapedString) {
    // Fast path early out
    if x >= right || x + value.total_width <= left {
        return;
    }

    // TODO: Consider binary search
    let (start_byte, start_col) = if left > x {
        match value.grapheme_info.iter().find(|&&(_, col)| x + col >= left) {
            Some(&(byte, col)) => (byte, x + col),
            None => return // This can happen, e.g. if the last grapheme is wide and partially clips over the left boundary
        }
    } else {
        (0, x)
    };
    let end_byte = if x + value.total_width > right {
        match value.grapheme_info.iter().rfind(|&&(_, col)| x + col <= right) {
            Some(&(byte, _)) => byte,
            None => return // This can't actually happen because it is covered by the early-out case
        }
    } else {
        value.text.len()
    };
    window.mvaddstr(y as i32, (start_col - left) as i32, &value.text[start_byte..end_byte]);
}

fn display_row(document: &Document, row: RowId, window: &Window, y: usize, left: usize, right: usize, attributes: Attributes) {
    let single_sep = ShapedString::from_string(" │ ".to_owned());
    let double_sep = ShapedString::from_string(" ║ ".to_owned());
    let mut x = 0usize;
    let mut prev_col_num = None;
    for &col in &document.views.top().cols {
        if let Some(num) = prev_col_num {
            window.attrset(A_NORMAL);
            let sep = if num + 1 == document.col_numbers[col] {
                &single_sep
            } else {
                &double_sep
            };
            draw_clipped_string(window, x, y, left, right, sep);
            x += 3;
        }
        window.attrset(attributes);
        draw_clipped_string(window, x, y, left, right, &document.data[row][col]);
        x += document.column_widths[col];
        prev_col_num = Some(document.col_numbers[col]);
    }
}

fn get_cell<'a>(document: &'a Document, cursor: &Cursor) -> &'a ShapedString {
    &document.data[document.views.top().rows[cursor.row_index]][document.views.top().cols[cursor.col_index]]
}

struct WindowEnder;

impl Drop for WindowEnder {
    fn drop(&mut self) {
        endwin();
    }
}

fn debug_print(window: &Window, message: &str) {
    let (height, width) = window.get_max_yx();
    let (y, x) = window.get_cur_yx();
    window.mvaddstr(height - 1, width.saturating_sub(message.len() as i32), message);
    window.mv(y, x);
}

enum Mode {
    Normal,
    Filter {
        query: ShapedString,
        query_pos: TextPosition,
    },
    Quitting
}

fn handle_editing(input: Option<Input>, text: &mut ShapedString, position: &mut TextPosition) -> bool {
    match input {
        Some(Input::KeyBackspace) => if !text.at_beginning(position) {
            text.delete_left(position);
        },
        Some(Input::KeyDC) if !text.at_end(position) => {
            text.delete_right(position);
        },
        Some(Input::Character(chr)) if !chr.is_control() => {
            text.insert(position, chr);
        },
        _ => {
            return false;
        }
    }
    true
}

#[derive(Debug)]
enum Direction {
    Left,
    Right,
    Up,
    Down
}

// FIXME: read terminfo instead of using hardcoded values
fn handle_navigation<'a, F: FnOnce(Direction, bool) -> Option<&'a ShapedString>>(input: Option<Input>, text: &'a ShapedString, position: &mut TextPosition, navigate: F) -> bool {
    match input {
        Some(Input::KeyLeft) => {
            if !text.at_beginning(position) {
                text.move_left(position);
            } else if let Some(new_text) = navigate(Direction::Left, false) {
                *position = TextPosition::end(new_text);
            }
        },
        Some(Input::Unknown(249)) => { // Ctrl + Left
            if let Some(new_text) = navigate(Direction::Left, false) {
                *position = TextPosition::end(new_text);
            } else {
                *position = TextPosition::beginning();
            }
        },
        Some(Input::KeyHome) => {
            if !text.at_beginning(position) {
                *position = TextPosition::beginning();
            } else if let Some(_new_text) = navigate(Direction::Left, true) {
                *position = TextPosition::beginning();
            }
        },
        Some(Input::KeyRight) => {
            if !text.at_end(position) {
                text.move_right(position);
            } else if let Some(_new_text) = navigate(Direction::Right, false) {
                *position = TextPosition::beginning();
            } else {
                // TODO: what if the last cell doesn't fit on the screen?
                return true;
            }
        },
        Some(Input::Unknown(264)) => { // Ctrl + Right
            if let Some(_new_text) = navigate(Direction::Right, false) {
                *position = TextPosition::beginning();
            } else if !text.at_end(position) {
                *position = TextPosition::end(text);
            } else {
                return true;
            }
        },
        Some(Input::KeyEnd) => {
            if !text.at_end(position) {
                *position = TextPosition::end(text);
            } else if let Some(new_text) = navigate(Direction::Right, true) {
                *position = TextPosition::end(new_text);
            } else {
                return true;
            }
        },
        Some(Input::KeyUp) | Some(Input::Unknown(270)) => { // [Ctrl +] Up
            if let Some(new_text) = navigate(Direction::Up, false) {
                new_text.move_vert(position);
            }
        },
        Some(Input::KeyPPage) => { // PageUp
            if let Some(new_text) = navigate(Direction::Up, true) {
                new_text.move_vert(position);
            }
        },
        Some(Input::KeyDown) | Some(Input::Unknown(227)) => { // [Ctrl +] Down
            if let Some(new_text) = navigate(Direction::Down, false) {
                new_text.move_vert(position);
            }
        },
        Some(Input::KeyNPage) => { // PageDown
            if let Some(new_text) = navigate(Direction::Down, true) {
                new_text.move_vert(position);
            }
        },
        _ => { }
    }
    false
}

fn main() {
    let arg_matches = clap::App::new("CSVSheet")
                                    .version("0.1")
                                    .author("Jonathan S <gereeter+code@gmail.com>")
                                    .about("View and edit CSV/DSV/TSV files")
                                    .arg(clap::Arg::with_name("delimiter")
                                        .short("d")
                                        .long("delimiter")
                                        .help("Sets the delimiter to split a line into records")
                                        .takes_value(true))
                                    .arg(clap::Arg::with_name("FILE")
                                        .help("Sets the file to view/edit")
                                        .required(true)
                                        .index(1))
                                    .get_matches();

    let file_name_arg = arg_matches.value_of_os("FILE").unwrap();
    let file_name = std::fs::canonicalize(&file_name_arg).expect("Unable to reach file");

    let delimiter = arg_matches.value_of_os("delimiter").and_then(|delim_os| delim_os.to_str()).and_then(|delim_str| {
        if delim_str.len() == 1 {
            Some(delim_str.as_bytes()[0])
        } else {
            // FIXME: Maybe just error instead?
            eprintln!("WARNING: non-byte delimiter provided, falling back to file extension detection");
            None
        }
    }).or_else(|| file_name.extension().and_then(|ext| {
        if ext == "dsv" {
            Some(b'|')
        } else if ext == "tsv" {
            Some(b'\t')
        } else {
            None
        }
    })).unwrap_or(b',');

    let reader = ReaderBuilder::new().delimiter(delimiter)
                                     .has_headers(false) // we handle this ourselves
                                     .flexible(true) // We'll fix up the file
                                     .from_path(&file_name)
                                     .expect("Unable to read file");
    let mut document = Document::new(
        reader.into_records()
              .map(|record| {
                  record.expect("Problem reading record")
                        .iter()
                        .map(|s| ShapedString::from_string(s.to_owned()))
                        .collect()
              })
             .collect(),
        delimiter
    );
    for row in &document.data {
        for (cell, col_width) in row.iter().zip(document.column_widths.iter_mut()) {
            *col_width = cmp::max(*col_width, cell.total_width);
        }
    }

    static CTRL_Z_COUNT: AtomicUsize = ATOMIC_USIZE_INIT;
    static CTRL_C_COUNT: AtomicUsize = ATOMIC_USIZE_INIT;

    
    // TODO: What do we do on other platforms?
    #[cfg(unix)] {
        unsafe {
            // Ctrl + Z
            extern "C" fn handle_ctrl_z(_: c_int) { CTRL_Z_COUNT.fetch_add(1, Ordering::Relaxed); }
            if let Err(err) = nix::sys::signal::sigaction(Signal::SIGTSTP,
                                                          &SigAction::new(SigHandler::Handler(handle_ctrl_z),
                                                                          SaFlags::empty(),
                                                                          SigSet::empty())) {
                eprintln!("WARNING: failed to disable suspension (Ctrl+Z): {:?}", err);
            }
            // Ctrl + C
            extern "C" fn handle_ctrl_c(_: c_int) { CTRL_C_COUNT.fetch_add(1, Ordering::Relaxed); }
            if let Err(err) = nix::sys::signal::sigaction(Signal::SIGINT,
                                                          &SigAction::new(SigHandler::Handler(handle_ctrl_c),
                                                                          SaFlags::empty(),
                                                                          SigSet::empty())) {
                eprintln!("WARNING: failed to disable interruption (Ctrl+C): {:?}", err);
            }
        }
        // Ctrl + S, Ctrl + Q
        let stdin_fd = std::io::stdin().as_raw_fd();
        match tcgetattr(stdin_fd) {
            Ok(mut attr) => {
                /*
                */
//                cfmakeraw(&mut attr);
                attr.input_flags &= !IXON;
                if let Err(err) = tcsetattr(stdin_fd, SetArg::TCSANOW, &attr) {
                    eprintln!("WARNING: failed to disable (set) flow control (Ctrl+S,Ctrl+Q): {:?}", err);
                }
            },
            Err(err) => {
                eprintln!("WARNING: failed to disable (get) flow control (Ctrl+S,Ctrl+Q): {:?}", err);
            }
        }
    }
    

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    //let mut screen = recurses::Screen::new(&stdin, &stdout);


    // TODO: check for errors!
    let window = initscr();
    let _defer_endwin = WindowEnder;
    window.keypad(true);
    cbreak();
    noecho();
    mousemask(ALL_MOUSE_EVENTS, ptr::null_mut());
    // TODO: consider behaviour around double, triple clicks
    mouseinterval(0); // We care about up/down, not clicks

    let mut width = 0;
    let mut height = 1;
    let mut offset_x = 0;
    let mut offset_y = 0;
    let mut cursor = Cursor {
        row_index: 0,
        col_index: 0,
        cell_display_column: 0,
        in_cell_pos: TextPosition::beginning()
    };
    let mut data_entry_start_index = 0;
    let mut data_entry_start_display_column = 0;
    let mut screen_x = 0;
    let mut screen_y = 0;
    let mut in_progress_codepoint = 0;
    let mut utf8_bytes_left = 0;
    let mut mode = Mode::Normal;
    let mut undo_state = UndoState::new();
    window.ungetch(&Input::KeyResize);
    loop {
        let mut redraw = false;
        let mut new_mode = Mode::Normal;

        // FIXME: This triggers on Ctrl + Z /and/ Ctrl + Shift + Z, but we'd like the latter to be redo. For now we settle for Ctrl + Alt + Z,
        // but it would be much much better to detect the shift key.
        let undo_count = CTRL_Z_COUNT.swap(0, Ordering::Relaxed);
        if undo_count > 0 {
            undo_state.prepare_edit(None, &document, &cursor);
            for _ in 0..undo_count {
                if let Some(op) = undo_state.undo_stack.pop() {
                    let inverse_op = op.apply_to(&mut document, &mut cursor);
                    undo_state.redo_stack.push(inverse_op);
                    redraw = true;
                }
            }
            if undo_state.is_pristine() {
                document.modified = false;
            }
        }
        let copy_count = CTRL_C_COUNT.swap(0, Ordering::Relaxed);
        if copy_count > 0 {
            debug_print(&window, &format!("> UNIMPLEMENTED: Copied {} times!", copy_count));
            window.refresh();
        }

        let mut input = window.getch();

        // Pancurses gets this very wrong, unfortunately. What pancurses calls a Character
        // is actually just a byte, incorrectly cast to a character.
        // We need to parse utf8.
        if let Some(Input::Character(byte_chr)) = input {
            let byte = byte_chr as u32;
            if byte >= 256 {
                panic!("BUG: non-byte Character received!");
            }
            if utf8_bytes_left == 0 {
                // New character
                if byte >> 7 == 0b0 {
                    utf8_bytes_left = 0;
                    in_progress_codepoint = byte & 0x7f;
                } else if byte >> 5 == 0b110 {
                    utf8_bytes_left = 1;
                    in_progress_codepoint = byte & 0x1f;
                } else if byte >> 4 == 0b1110 {
                    utf8_bytes_left = 2;
                    in_progress_codepoint = byte & 0x0f;
                } else if byte >> 3 == 0b11110 {
                    utf8_bytes_left = 3;
                    in_progress_codepoint = byte & 0x07;
                } else {
                    // FIXME: this should not crash
                    panic!("Bad unicode: first byte {:x}", byte);
                }
            } else {
                utf8_bytes_left -= 1;
                in_progress_codepoint = (in_progress_codepoint << 6) | (byte & 0x3f);
            }
            debug_print(&window, &format!("> Got byte {:x}, in progress codepoint = {:x}", byte, in_progress_codepoint));
            if utf8_bytes_left == 0 {
                input = Some(Input::Character(std::char::from_u32(in_progress_codepoint).expect("BUG: Bad char cast")));
            } else {
                input = None;
            }
        }

        if let Some(Input::KeyResize) = input {
            let (new_height, new_width) = window.get_max_yx();
            height = new_height as usize;
            width = new_width as usize;
            screen_x = cmp::min(screen_x, width);
            screen_y = cmp::min(screen_y, height);
            window.clearok(true);
            redraw = true;
        }

        if let Some(Input::Character('\u{11}')) = input { // Ctrl + Q
            if document.modified {
                new_mode = Mode::Quitting;
                redraw = true;
            } else {
                break;
            }
        }

        // Keys with modifiers (at least on xterm, utf8 mode)
        //        Left     Up    Right     Down     Delete  Backspace
        // None   KeyLeft  KeyUp KeyRight  KeyDown  KeyDC   KeyBackspace
        // Shift  KeySLeft KeySR KeySRight KeySF    KeySDC  KeyBackspace
        // Ctrl   249      270   264       227      221     \u{7f}
        // Both   250      271   265       228      222     \u{7f}
        // Alt    247      268   262       225      219     \u{88}
        let mut try_fit_x = false; // Signal that we should continue scrolling to show the whole cell without actually moving the cursor
        match mode {
            Mode::Filter { mut query, mut query_pos } => {
                // Editing
                let refilter = handle_editing(input, &mut query, &mut query_pos);
                handle_navigation(input, &query, &mut query_pos, |_, _| None);
                if refilter {
                    document.views.pop();
                    document.views.duplicate_top();
                    document.views.top_mut().ty = ViewType::Filter;

                    let mut index = 0;
                    let mut good_count = 0;
                    let mut new_cursor_index = 0;
                    let headers = document.views.top().headers;
                    // TODO: better closuree captures should make this unnecessary
                    let document_views = &mut document.views;
                    let document_data = &mut document.data;
                    document_views.top_mut().rows.retain(|&row| {
                        let mut good = false;
                        if index < headers {
                            good = true;
                        }

                        if document_data[row].iter().any(|str| str.text.contains(&query.text)) {
                            good = true;
                        }

                        if good {
                            if index <= cursor.row_index {
                                new_cursor_index = good_count;
                            }
                            good_count += 1;
                        }
                        index += 1;
                        return good;
                    });
                    cursor.row_index = new_cursor_index;
                    redraw = true;
                    new_mode = Mode::Filter { query, query_pos };
                } else if let Some(Input::Character('\u{1b}')) = input { // Escape
                    let cursor_row = document.views.top().rows[cursor.row_index];
                    document.views.pop();
                    cursor.row_index = document.views.top().rows.iter().position(|&row| row == cursor_row).expect("BUG: unfiltered view does not contain cursor!");
                    new_mode = Mode::Normal;
                    redraw = true;
                } else if let Some(Input::Character('\n')) = input {
                    new_mode = Mode::Normal;
                    redraw = true;
                } else {
                    new_mode = Mode::Filter { query, query_pos };
                }
            },
        Mode::Normal => {
            // Undo management
            // TODO: don't duplicate the knowledge of what keys do what
            match input {
                Some(Input::KeyDC) | Some(Input::KeyBackspace) => {
                    undo_state.prepare_edit(Some(EditType::Delete), &document, &cursor);
                },
                Some(Input::Character(c)) if !c.is_control() => {
                    undo_state.prepare_edit(Some(EditType::Insert), &document, &cursor);
                },
                Some(Input::KeyLeft) | Some(Input::Unknown(249)) | Some(Input::KeyHome) |
                Some(Input::KeyRight) | Some(Input::Unknown(264)) | Some(Input::KeyEnd) |
                Some(Input::KeyUp) | Some(Input::KeyPPage) |
                Some(Input::KeyDown) | Some(Input::KeyNPage) => {
                    undo_state.prepare_edit(None, &document, &cursor);
                },
                _ => { }
            }

            // Editing
            {
                let cell = &mut document.data[document.views.top().rows[cursor.row_index]][document.views.top().cols[cursor.col_index]];
                let reevaluate_column_width = cell.total_width == document.column_widths[document.views.top().cols[cursor.col_index]];
                let changed = handle_editing(input, cell, &mut cursor.in_cell_pos);
                if changed {
                    document.modified = true;
                    redraw = true;
                    if reevaluate_column_width || cursor.in_cell_pos.display_column > cell.total_width {
                        document.column_widths[document.views.top().cols[cursor.col_index]] = document.data.iter().map(|row| row[document.views.top().cols[cursor.col_index]].total_width).max().unwrap_or(0);
                    }
                }
            }
            // Navigation
            let mut new_pos = cursor.in_cell_pos;
            try_fit_x = handle_navigation(input, get_cell(&document, &cursor), &mut new_pos, |dir, skip| {
                match dir {
                    Direction::Left if cursor.col_index > 0 => if skip {
                        cursor.col_index = 0;
                        cursor.cell_display_column = 0;
                    } else {
                        cursor.col_index -= 1;
                        cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                    },
                    Direction::Right if cursor.col_index + 1 < document.views.top().cols.len() => if skip {
                        cursor.col_index = document.views.top().cols.len() - 1;
                        cursor.cell_display_column = document.views.top().cols[..cursor.col_index].iter().map(|&col_id| document.column_widths[col_id]).sum::<usize>() + 3 * cursor.col_index;
                    } else {
                        cursor.col_index += 1;
                        cursor.cell_display_column += document.column_widths[document.views.top().cols[cursor.col_index - 1]] + 3;
                    },
                    Direction::Up if cursor.row_index > 0 => if skip { // TODO: consider jumping farther/over empty spots for Ctrl + Up?
                        let page_size = height - document.views.top().headers;
                        cursor.row_index = cursor.row_index.saturating_sub(page_size);
                    } else {
                        cursor.row_index -= 1;
                    },
                    Direction::Down if cursor.row_index + 1 < document.views.top().rows.len() => if skip {
                        let page_size = height - document.views.top().headers;
                        cursor.row_index = cmp::min(cursor.row_index + page_size, document.views.top().rows.len() - 1);
                    } else {
                        cursor.row_index += 1;
                    },
                    _ => {
                        return None;
                    }
                }
                data_entry_start_index = cursor.col_index;
                data_entry_start_display_column = cursor.cell_display_column;
                Some(&document.data[document.views.top().rows[cursor.row_index]][document.views.top().cols[cursor.col_index]])
            });
            cursor.in_cell_pos = new_pos;
        match input {
            Some(Input::KeyResize) => {
                let (new_height, new_width) = window.get_max_yx();
                height = new_height as usize;
                width = new_width as usize;
                screen_x = cmp::min(screen_x, width);
                screen_y = cmp::min(screen_y, height);
                window.clearok(true);
                redraw = true;
            },
            Some(Input::Character('\u{9a}')) => { // Ctrl + Alt + Z
                undo_state.prepare_edit(None, &document, &cursor);
                if let Some(op) = undo_state.redo_stack.pop() {
                    let inverse_op = op.apply_to(&mut document, &mut cursor);
                    undo_state.undo_stack.push(inverse_op);
                    redraw = true;
                }
                if undo_state.is_pristine() {
                    document.modified = false;
                }
            },
            Some(Input::Character('\u{6}')) => { // Ctrl + F
                undo_state.prepare_edit(None, &document, &cursor);
                document.views.duplicate_top();
                document.views.top_mut().ty = ViewType::Filter;
                new_mode = Mode::Filter {
                    query: ShapedString::new(),
                    query_pos: TextPosition::beginning()
                };
                redraw = true;
            },
            // TODO: better shortcut? Actually delete the line and have a way to paste it?
            // It seems mostly undefined in "standard" desktop programs (only create hyperlink, but eh, no one knows or cares about that).
            // This sort of matches the "Kill" up to end of line behavior or nano or emacs or unixy things. More like nano than emacs.
            Some(Input::Character('\u{b}')) => { // Ctrl + K
                undo_state.prepare_edit(None, &document, &cursor);
                if document.views.top().rows.len() > 1 {
                    if document.views.top().ty != ViewType::Hide {
                        document.views.duplicate_top();
                        document.views.top_mut().ty = ViewType::Hide;
                    }
                    document.views.top_mut().rows.remove(cursor.row_index);
                    if cursor.row_index >= document.views.top().rows.len() {
                        cursor.row_index -= 1;
                    }
                    get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                    redraw = true;
                }
            },
            Some(Input::Character('\u{11}')) => { // Ctrl + Q
                undo_state.prepare_edit(None, &document, &cursor);
                if document.modified {
                    new_mode = Mode::Quitting;
                    redraw = true;
                } else {
                    break;
                }
            },
            Some(Input::Character('\u{14}')) => { // Ctrl + T
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                document.column_widths.push(0);
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index + 1, new_col_id);
                }
                cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                cursor.col_index += 1;
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(Input::Character('\u{17}')) => { // Ctrl + W
                if document.views.top().cols.len() > 1 {
                    undo_state.prepare_edit(None, &document, &cursor);
                    if document.views.top().ty != ViewType::Hide {
                        document.views.duplicate_top();
                        document.views.top_mut().ty = ViewType::Hide;
                    }
                    document.views.top_mut().cols.remove(cursor.col_index);
                    if cursor.col_index < document.views.top().cols.len() {
                        cursor.in_cell_pos = TextPosition::beginning();
                    } else {
                        cursor.col_index -= 1;
                        cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                        cursor.in_cell_pos = TextPosition::end(get_cell(&document, &cursor));
                    }
                    redraw = true;
                }
            },
            Some(Input::Character('\u{1b}')) => { // Escape
                undo_state.prepare_edit(None, &document, &cursor);
                let cursor_row = document.views.top().rows[cursor.row_index];
                let cursor_col = document.views.top().cols[cursor.col_index];
                document.views.pop();
                cursor.row_index = document.views.top().rows.iter().position(|&row| row == cursor_row).expect("BUG: old view does not contain cursor!");
                cursor.col_index = document.views.top().cols.iter().position(|&col| col == cursor_col).expect("BUG: old view does not contain cursor!");
                cursor.cell_display_column = document.views.top().cols.iter().take(cursor.col_index).map(|&col| document.column_widths[col]).sum::<usize>() + 3 * cursor.col_index;
                redraw = true;
            },
            Some(Input::Character('\u{13}')) => { // Ctrl + S
                undo_state.prepare_edit(None, &document, &cursor);
                // TODO: track file moves and follow the file
                // TODO: display error to the user
                if let Ok(_) = document.save_to(&file_name) {
                    undo_state.pristine_state = Some(undo_state.undo_stack.len());
                }
            },
            // ------------------------------------------ Navigation ----------------------------------------------
            Some(Input::Unknown(247)) | Some(Input::Unknown(251)) => { // [Ctrl +] Alt + Left
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id]);
                document.column_widths.push(0);
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index, new_col_id);
                }
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(Input::Unknown(262)) | Some(Input::Unknown(266)) => { // [Ctrl +] Alt + Right
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                document.column_widths.push(0);
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index + 1, new_col_id);
                }
                cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                cursor.col_index += 1;
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(Input::Character('\t')) => {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                if cursor.col_index + 1 == document.views.top().cols.len() {
                    // TODO: is creating a new column really the right behaviour?
                    let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                    document.column_widths.push(0);
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                        upd_view.cols.insert(index + 1, new_col_id);
                    }
                    redraw = true;
                }
                cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                cursor.col_index += 1;
                // TODO: or jump to the beginning?
                cursor.in_cell_pos = TextPosition::end(get_cell(&document, &cursor));
            },
            Some(Input::Character('\n')) => {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                if cursor.row_index + 1 == document.views.top().rows.len() {
                    let new_row_id = document.insert_row(document.row_numbers[current_row_id] + 1);
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                        upd_view.rows.insert(index + 1, new_row_id);
                    }
                    redraw = true;
                }
                cursor.row_index += 1;
                cursor.col_index = data_entry_start_index;
                cursor.cell_display_column = data_entry_start_display_column;
                // TODO: or jump to the beginning
                cursor.in_cell_pos = TextPosition::end(get_cell(&document, &cursor));
            },
            Some(Input::Unknown(268)) | Some(Input::Unknown(272)) => { // [Ctrl +] Alt + Up
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let new_row_id = document.insert_row(document.row_numbers[current_row_id]);
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                    upd_view.rows.insert(index, new_row_id);
                }
                get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                redraw = true;
            },
            Some(Input::Unknown(225)) | Some(Input::Unknown(229)) => { // [Ctrl +] Alt + Down
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let new_row_id = document.insert_row(document.row_numbers[current_row_id] + 1);
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                    upd_view.rows.insert(index + 1, new_row_id);
                }
                cursor.row_index += 1;
                get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                redraw = true;
            },
            Some(Input::Unknown(221)) => { // Ctrl + Delete
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let current_col_id = document.views.top().cols[cursor.col_index];

                // Delete row if empty
                if document.views.top().rows.len() > 1 && document.data[current_row_id].iter().all(|cell| cell.text.is_empty()) {
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                        upd_view.rows.remove(index);
                    }
                    if cursor.row_index >= document.views.top().rows.len() {
                        cursor.row_index -= 1;
                        get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                    }
                    let deleted_row_num = document.row_numbers[current_row_id];
                    for row_num in &mut document.row_numbers {
                        if *row_num > deleted_row_num {
                            *row_num -= 1;
                        }
                    }
                    redraw = true;
                }

                // Delete column if empty
                if document.views.top().cols.len() > 0 && document.data.iter().all(|row| row[current_col_id].text.is_empty()) {
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                        upd_view.cols.remove(index);
                    }
                    if cursor.col_index >= document.views.top().cols.len() {
                        cursor.col_index -= 1;
                        cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                        cursor.in_cell_pos = TextPosition::end(get_cell(&document,&cursor));
                    }
                    let deleted_col_num = document.col_numbers[current_col_id];
                    for col_num in &mut document.col_numbers {
                        if *col_num > deleted_col_num {
                            *col_num -= 1;
                        }
                    }
                    
                    redraw = true;
                }
            },
            // ---------------------- Mouse Input ------------------------
            Some(Input::KeyMouse) => {
                let event = match getmouse() {
                    Ok(event) => event,
                    Err(_) => {
                        // TODO: figure out scrolling, which is triggering this
                        continue;
                    }
                };
                // TODO: when are multiple bits set?
                if event.bstate & 2 != 0 { // 2 is left button down
                    undo_state.prepare_edit(None, &document, &cursor);
                    // TODO: What is the z coordinate? What is the id?
                    let hit_row = if (event.y as usize) < document.views.top().headers {
                        event.y as usize
                    } else {
                        event.y as usize + offset_y
                    };
                    let hit_column = event.x as usize + offset_x;

                    if hit_row < document.views.top().rows.len() {
                        cursor.row_index = hit_row;
                        cursor.col_index = 0;
                        cursor.cell_display_column = 0;
                        while hit_column > cursor.cell_display_column + document.column_widths[document.views.top().cols[cursor.col_index]] && cursor.col_index + 1 < document.views.top().cols.len() {
                            cursor.cell_display_column += document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                            cursor.col_index += 1;
                        }
                        cursor.in_cell_pos.display_column = hit_column.saturating_sub(cursor.cell_display_column);
                        get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                        try_fit_x = true;
                    }
                }
            },
            // ------------------------------------- Fallback/Debugging ------------------------------------
            Some(input) => {
                // TODO: just debugging, remove this
                debug_print(&window, &format!("> {:?}", input));
            },
            _ => { }
        } },
        Mode::Quitting => match input {
                Some(Input::Character('y')) => {
                    // TODO: track renames and follow the file
                    // TODO: display error to the user
                    if let Ok(_) = document.save_to(&file_name) {
                        break;
                    }
                },
                Some(Input::Character('n')) => {
                    break;
                },
                Some(Input::Character('\u{1b}')) => { // Escape
                    new_mode = Mode::Normal;
                    redraw = true;
                },
                _ => {
                    new_mode = Mode::Quitting;
                }
            }
        }
        mode = new_mode;

        let rows_shown = height - 1;

        // Scrolling
        let target_x = cursor.cell_display_column + cmp::min(get_cell(&document, &cursor).total_width, cursor.in_cell_pos.display_column);
        let target_y = cursor.row_index;
        if offset_x > target_x || offset_x + width <= target_x {
            // Whenever we scroll, we try to preserve the screen position, with the slight modification that getting the whole cell in
            // view is prefererable, including any separators on the sides
            offset_x = target_x.saturating_sub(screen_x);
            redraw = true;
            try_fit_x = true;
        }
        if offset_y + document.views.top().headers > target_y || offset_y + rows_shown <= target_y {
            offset_y = target_y.saturating_sub(screen_y);
            if target_y >= document.views.top().headers && offset_y + document.views.top().headers > target_y {
                offset_y = target_y - document.views.top().headers;
            }
            redraw = true;
        }
        if try_fit_x {
            let mut cell_start = cursor.cell_display_column;
            let mut cell_end = cell_start + document.column_widths[document.views.top().cols[cursor.col_index]];
            if cursor.col_index > 0 {
               cell_start -= 2;
            }
            if cursor.col_index + 1 < document.views.top().cols.len() {
               cell_end += 2;
            }
            // If we can't fit the cell, don't try and end up messing things up.
            if cell_end - cell_start <= width {
                if offset_x > cell_start {
                    offset_x = cell_start;
                    redraw = true;
                } else if offset_x + width < cell_end {
                    offset_x = cell_end - width;
                    redraw = true;
                }
            }
        }
        screen_x = target_x - offset_x;
        screen_y = target_y - offset_y;

        if redraw {
            window.erase();

            for y in 0..document.views.top().headers {
                display_row(&document, document.views.top().rows[y], &window, y, offset_x, offset_x + width, Attributes::new() | Attribute::Bold);
            }

            for (row_i, &row) in document.views.top().rows.iter().skip(offset_y + document.views.top().headers).take(rows_shown - document.views.top().headers).enumerate() {
                display_row(&document, row, &window, row_i + document.views.top().headers, offset_x, offset_x + width, Attributes::new());
            }
        }

        window.attrset(A_NORMAL);
        window.mv(height as i32 - 1, 0);
        window.clrtoeol();
        if let Mode::Filter { ref query, .. } = mode {
            window.mvaddstr(height as i32 - 1, 0, "Find rows containing: ");
            window.addstr(&query.text);
        } else if let Mode::Quitting = mode {
            window.mvaddstr(height as i32 - 1, 0, "Save before quitting [y/n]? ");
        } else {
            // TODO(efficiency): avoid allocations
            let status = format!(
                "[ row {}/{}, col {}/{}, char {}/{}, last_input: {:?} ]",
                document.row_numbers[document.views.top().rows[cursor.row_index]] + 1, document.height(),
                document.col_numbers[document.views.top().cols[cursor.col_index]] + 1, document.width(),
                cursor.in_cell_pos.grapheme_index, get_cell(&document, &cursor).grapheme_info.len(),
                input
            );
            window.mvaddstr(height as i32 - 1, ((width.saturating_sub(status.len())) / 2) as i32, &status);
        }

        if let Mode::Normal = mode {
            window.mv(screen_y as i32, screen_x as i32);
        } else if let Mode::Filter { ref query, query_pos, .. } = mode {
            window.mv(height as i32 - 1, 22 + query.display_column(query_pos.grapheme_index) as i32);
        }
        window.refresh();
    }
}

/*
fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut screen = recurses::Screen::new(&stdin, &stdout).unwrap();

    loop {
       let mut byte = [0];
       screen.input.read_exact(&mut byte).unwrap();
       write!(screen.output, "{:02x}\r\n", byte[0]);
       if (byte[0] == 3) {
           break;
       }
    }    
}
*/
