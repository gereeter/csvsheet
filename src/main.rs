#![feature(nll)] // TODO: consider stabilization?
// nll is jusr convenience

extern crate ncurses;
extern crate csv;
extern crate unicode_segmentation;
extern crate unicode_width;
extern crate terminfo;
extern crate clap;
extern crate tempfile;
extern crate xattr;
extern crate smallvec;
#[macro_use] extern crate const_cstr;

mod indexed_vec;
mod stack;
mod curses;
#[macro_use] mod input;
mod string;
//mod recurses;

use indexed_vec::{Idx, IndexVec};
use stack::RefillingStack;
use curses::Window;
use string::SmallString;
use input::Input;

use std::cmp;
use std::iter;
use std::path::Path;
use std::borrow::Cow;

use csv::ReaderBuilder;
use unicode_segmentation::GraphemeCursor;
use unicode_width::{UnicodeWidthStr, UnicodeWidthChar};

use ncurses::{A_BOLD, A_NORMAL};

#[derive(Clone)]
struct ShapedString {
    text: SmallString,
    total_width: usize
}

impl ShapedString {
    fn new() -> Self {
        ShapedString {
            text: SmallString::new(),
            total_width: 0
        }
    }

    fn from_string(text: SmallString) -> Self {
         let width = UnicodeWidthStr::width(&*text);
         ShapedString {
            text: text,
            total_width: width
        }
    }

    fn at_beginning(&self, position: &TextPosition) -> bool {
        position.grapheme_cursor.cur_cursor() == 0
    }

    fn at_end(&self, position: &TextPosition) -> bool {
        position.grapheme_cursor.cur_cursor() >= self.text.len()
    }

    fn move_left(&self, position: &mut TextPosition) {
        let after_offset = position.grapheme_cursor.cur_cursor();
        if let Ok(Some(before_offset)) = position.grapheme_cursor.prev_boundary(&self.text, 0) {
            position.display_column -= UnicodeWidthStr::width(&self.text[before_offset..after_offset]);
            position.movement_column = position.display_column;
        }
    }

    fn move_right(&self, position: &mut TextPosition) {
        let before_offset = position.grapheme_cursor.cur_cursor();
        if let Ok(Some(after_offset)) = position.grapheme_cursor.next_boundary(&self.text, 0) {
            position.display_column += UnicodeWidthStr::width(&self.text[before_offset..after_offset]);
            position.movement_column = position.display_column;
        }
    }

    fn move_vert(&self, position: &mut TextPosition) {
        // We will iterate end-to-start to find the grapheme that is selected
        position.grapheme_cursor = GraphemeCursor::new(self.text.len(), self.text.len(), true);
        position.display_column = self.total_width;

        // TODO: precompute column-to-offset mappings for long strings to bring down complexity here?
        let mut after_offset = self.text.len();
        while position.display_column > position.movement_column {
            // Move leftward until we pass our target
            if let Ok(Some(before_offset)) = position.grapheme_cursor.prev_boundary(&self.text, 0) {
                position.display_column -= UnicodeWidthStr::width(&self.text[before_offset..after_offset]);
                after_offset = before_offset;
            } else {
                return;
            }
        }
    }

    fn delete_left(&mut self, position: &mut TextPosition) {
        // TODO: RTL text
        let after_offset = position.grapheme_cursor.cur_cursor();
        if let Ok(Some(before_offset)) = position.grapheme_cursor.prev_boundary(&self.text, 0) {
            let col_width_removed = UnicodeWidthStr::width(&self.text[before_offset..after_offset]);

            self.text.remove_range(before_offset..after_offset);
            self.total_width -= col_width_removed;

            position.display_column -= col_width_removed;
            position.movement_column = position.display_column;
            position.grapheme_cursor = GraphemeCursor::new(before_offset, self.text.len(), true);
        }
    }

    fn delete_right(&mut self, position: &mut TextPosition) {
        // TODO: RTL text
        let before_offset = position.grapheme_cursor.cur_cursor();
        if let Ok(Some(after_offset)) = position.grapheme_cursor.next_boundary(&self.text, 0) {
            let col_width_removed = UnicodeWidthStr::width(&self.text[before_offset..after_offset]);

            self.text.remove_range(before_offset..after_offset);
            self.total_width -= col_width_removed;

            position.movement_column = position.display_column;
            position.grapheme_cursor = GraphemeCursor::new(before_offset, self.text.len(), true);
        }
    }

    fn insert(&mut self, position: &mut TextPosition, chr: char) {
        let col_width_inserted = UnicodeWidthChar::width(chr).unwrap_or(0);
        let insertion_point = position.grapheme_cursor.cur_cursor();
        let tail_bytes = self.text.len() - insertion_point;

        self.text.insert(insertion_point, chr);
        self.total_width += col_width_inserted;

        position.display_column += col_width_inserted;
        position.movement_column = position.display_column;
        position.grapheme_cursor = GraphemeCursor::new(self.text.len() - tail_bytes, self.text.len(), true);
    }
}

#[derive(Clone)]
struct TextPosition {
    grapheme_cursor: GraphemeCursor,
    display_column: usize,
    movement_column: usize // For vertical movement, what column we are in. This may be in the middle of a grapheme or even out of bounds
}

impl TextPosition {
    fn beginning() -> Self {
        TextPosition {
            // TODO: Since we don't know the size we just say it could be followed arbitrarily. Is this correct?
            // Should we just require the string for creation?
            grapheme_cursor: GraphemeCursor::new(0, usize::max_value(), true),
            display_column: 0,
            movement_column: 0
        }
    }

    fn end(str: &ShapedString) -> Self {
        TextPosition {
            grapheme_cursor: GraphemeCursor::new(str.text.len(), str.text.len(), true),
            display_column: str.total_width,
            movement_column: str.total_width
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

        for &other_col in self.views.base().cols.iter() {
            if self.col_numbers[other_col] >= col_num {
                self.col_numbers[other_col] += 1;
            }
        }

        self.column_widths.push(0);
        self.col_numbers.push(col_num)
    }

    fn insert_row(&mut self, row_num: usize) -> RowId {
        self.modified = true;
        self.data.push(IndexVec::from_vec(vec![ShapedString::new(); self.width()]));

        for &other_row in self.views.base().rows.iter() {
            if self.row_numbers[other_row] >= row_num {
                self.row_numbers[other_row] += 1;
            }
        }

        self.row_numbers.push(row_num)
    }

    fn delete_col(&mut self, col: ColId) {
        for upd_view in self.views.iter_mut() {
            if let Some(index) = upd_view.cols.iter().position(|&col_id| col_id == col) {
                upd_view.cols.remove(index);
            }
        }

        let deleted_col_num = self.col_numbers[col];
        for &other_col in self.views.base().cols.iter() {
            if self.col_numbers[other_col] > deleted_col_num {
                self.col_numbers[other_col] -= 1;
            }
        }

        self.modified = true;
    }

    fn delete_row(&mut self, row: RowId) {
        for upd_view in self.views.iter_mut() {
            if let Some(index) = upd_view.rows.iter().position(|&row_id| row_id == row) {
                upd_view.rows.remove(index);
            }
        }

        let deleted_row_num = self.row_numbers[row];
        for &other_row in self.views.base().rows.iter() {
            if self.row_numbers[other_row] > deleted_row_num {
                self.row_numbers[other_row] -= 1;
            }
        }

        self.modified = true;
    }

    fn resize_column(&mut self, col: ColId) {
        self.column_widths[col] = self.data.iter().map(|row| row[col].total_width).max().unwrap_or(0);
    }

    fn save_to(&mut self, path: &Path) -> Result<(), std::io::Error> {
        let named_temp_file = tempfile::NamedTempFile::new_in(path.parent().ok_or(std::io::ErrorKind::Other)?)?;
        // FIXME: There is a race condition here where the permissions might get modified in between these calls. I'm not sure how to fix that.
        // FIXME: Copy other metadata?
        let permissions = std::fs::metadata(path)?.permissions();
        std::fs::set_permissions(named_temp_file.path(), permissions)?;
        for xattr_name in xattr::list(path)? {
            if let Some(value) = xattr::get(path, &xattr_name)? {
                xattr::set(named_temp_file.path(), &xattr_name, &value)?;
            }
        }
        let mut temp_file = named_temp_file.reopen()?;
        let temp_path = named_temp_file.into_temp_path();
        {
            let mut writer = csv::WriterBuilder::new().delimiter(self.delimiter)
                                                      .from_writer(&mut temp_file);
            for &row_id in &self.views.base().rows {
                writer.write_record(self.views.base().cols.iter().map(|&col_id| self.data[row_id][col_id].text.as_bytes()))?;
            }
        }
        temp_file.sync_data()?;
        drop(temp_file);

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
    },
    InsertRow(RowId),
    DeleteRow(RowId),
    InsertCol(ColId),
    DeleteCol(ColId),
    Group(Vec<UndoOp>),
}

impl UndoOp {
    fn apply_to(self, document: &mut Document, cursor: &mut Cursor) -> UndoOp {
        document.modified = true;
        match self {
            UndoOp::Edit { row_id, col_id, before_in_cell_pos, after_in_cell_pos, before_text } => {
                let before_width = before_text.total_width;
                let after_text = std::mem::replace(&mut document.data[row_id][col_id], before_text);
                let after_width = after_text.total_width;

                let after_column_width = document.column_widths[col_id];
                if before_width > after_column_width || (after_width == after_column_width && before_width < after_column_width) {
                    document.resize_column(col_id);
                }

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
                    in_cell_pos: before_in_cell_pos.clone()
                };

                UndoOp::Edit {
                    row_id: row_id,
                    col_id: col_id,
                    before_in_cell_pos: after_in_cell_pos,
                    after_in_cell_pos: before_in_cell_pos,
                    before_text: after_text
                }
            },
            UndoOp::InsertRow(id) => {
                // TODO: Instead of undoing everything, reapply views as long as the new row matches
                let base = document.views.clear_to_base();
                let index = document.row_numbers[id];
                for &row_id in &base.rows {
                    if document.row_numbers[row_id] >= index {
                        document.row_numbers[row_id] += 1;
                    }
                }
                base.rows.insert(index, id);

                cursor.row_index = index;
                get_cell(document, cursor).move_vert(&mut cursor.in_cell_pos);
                UndoOp::DeleteRow(id)
            },
            UndoOp::DeleteRow(id) => {
                let deleted_index = loop {
                    if let Some(index) = document.views.top().rows.iter().position(|&row_id| row_id == id){
                        break index;
                    } else {
                        document.views.pop();
                    }
                };
                document.delete_row(id);
                if deleted_index < document.views.top().rows.len() {
                    cursor.row_index = deleted_index;
                } else {
                    cursor.row_index = document.views.top().rows.len() - 1;
                }
                get_cell(document, cursor).move_vert(&mut cursor.in_cell_pos);
                UndoOp::InsertRow(id)
            },
            UndoOp::InsertCol(id) => {
                // TODO: Instead of undoing everything, reapply views as long as the new row matches
                let base = document.views.clear_to_base();
                let index = document.col_numbers[id];
                for &col_id in &base.cols {
                    if document.col_numbers[col_id] >= index {
                        document.col_numbers[col_id] += 1;
                    }
                }
                base.cols.insert(index, id);

                cursor.col_index = index;
                cursor.cell_display_column = document.views.top().cols[..cursor.col_index].iter().map(|&col_id| document.column_widths[col_id] + 3).sum();
                cursor.in_cell_pos = TextPosition::end(get_cell(document, cursor));
                UndoOp::DeleteCol(id)
            },
            UndoOp::DeleteCol(id) => {
                let deleted_index = loop {
                    if let Some(index) = document.views.top().cols.iter().position(|&col_id| col_id == id){
                        break index;
                    } else {
                        document.views.pop();
                    }
                };
                document.delete_col(id);
                if deleted_index < document.views.top().cols.len() {
                    cursor.col_index = deleted_index;
                    cursor.cell_display_column = document.views.top().cols[..cursor.col_index].iter().map(|&col_id| document.column_widths[col_id] + 3).sum();
                    cursor.in_cell_pos = TextPosition::beginning();
                } else {
                    cursor.col_index = document.views.top().rows.len() - 1;
                    cursor.cell_display_column = document.views.top().cols[..cursor.col_index].iter().map(|&col_id| document.column_widths[col_id] + 3).sum();
                    cursor.in_cell_pos = TextPosition::end(get_cell(document, cursor));
                }
                UndoOp::InsertCol(id)
            },
            UndoOp::Group(mut ops) => {
                let mut rev_ops = Vec::with_capacity(ops.len());
                while let Some(op) = ops.pop() {
                    rev_ops.push(op.apply_to(document, cursor));
                }
                UndoOp::Group(rev_ops)
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

    fn push(&mut self, op: UndoOp) {
        if let Some(height) = self.pristine_state {
            if height > self.undo_stack.len() {
                self.pristine_state = None;
            }
        }

        self.redo_stack.clear();
        self.undo_stack.push(op);
    }

    fn prepare_edit(&mut self, edit_type: Option<EditType>, document: &Document, cursor: &Cursor) {
        if edit_type != self.current_edit_type {
            self.current_edit_type = edit_type;
            if let Some(&mut UndoOp::Edit { ref mut after_in_cell_pos, .. }) = self.undo_stack.last_mut() {
                *after_in_cell_pos = cursor.in_cell_pos.clone();
            }
            if edit_type.is_some() {
                let row_id = document.views.top().rows[cursor.row_index];
                let col_id = document.views.top().cols[cursor.col_index];

                self.push(UndoOp::Edit {
                    row_id: row_id,
                    col_id: col_id,
                    before_in_cell_pos: cursor.in_cell_pos.clone(),
                    after_in_cell_pos: cursor.in_cell_pos.clone(), // TODO: Is this clone expensive? It is just going to be overwritten.
                    before_text: document.data[row_id][col_id].clone()
                });
            }
        }
    }
}

/*
fn draw_clipped_string_ascii(window: &mut Window, x: usize, y: usize, left: usize, right: usize, value: &str) {
    // Fast path early out
    if x >= right || x + value.len() <= left {
        return;
    }

    // TODO: Consider binary search
    let start_byte = left.saturating_sub(x);
    let start_col = cmp::max(x, left);
    let end_byte = cmp::min(value.len(), right - x);
    window.mv_add_str(y as i32, (start_col - left) as i32, &value[start_byte..end_byte]);
}
*/

// TODO: right-to-left text?
fn draw_clipped_string(window: &mut Window, x: usize, y: usize, left: usize, right: usize, value: &ShapedString) {
    // Fast path early out
    if x >= right || x + value.total_width <= left {
        return;
    }

    let mut clipped_chars = value.text.chars();

    // TODO: Consider binary search
    let mut start_col = x;
    while left > start_col {
        if let Some(chr) = clipped_chars.next() {
            start_col += UnicodeWidthChar::width(chr).unwrap_or(0);
        } else {
            // We've clipped out the entire string
            return;
        }
    }

    let mut end_col = x + value.total_width;
    while right < end_col {
        if let Some(chr) = clipped_chars.next_back() {
            end_col -= UnicodeWidthChar::width(chr).unwrap_or(0);
        } else {
            // We've clipped out the entire string
            return;
        }
    }

    window.mv_add_str(y as i32, (start_col - left) as i32, &clipped_chars.as_str());
}

fn display_row(document: &Document, row: RowId, window: &mut Window, y: usize, left: usize, right: usize, attributes: ncurses::attr_t) {
    let single_sep = ShapedString::from_string(SmallString::from_str(" │ "));
    let double_sep = ShapedString::from_string(SmallString::from_str(" ║ "));
    let mut x = 0usize;
    let mut prev_col_num = None;
    for &col in &document.views.top().cols {
        if let Some(num) = prev_col_num {
            window.set_attrs(A_NORMAL());
            let sep = if num + 1 == document.col_numbers[col] {
                &single_sep
            } else {
                &double_sep
            };
            draw_clipped_string(window, x, y, left, right, sep);
            x += 3;
        }
        window.set_attrs(attributes);
        draw_clipped_string(window, x, y, left, right, &document.data[row][col]);
        x += document.column_widths[col];
        prev_col_num = Some(document.col_numbers[col]);
    }
}

fn get_cell<'a>(document: &'a Document, cursor: &Cursor) -> &'a ShapedString {
    &document.data[document.views.top().rows[cursor.row_index]][document.views.top().cols[cursor.col_index]]
}

enum Mode {
    Normal,
    Filter {
        query: ShapedString,
        query_pos: TextPosition,
    },
    Quitting,
    Help
}

fn handle_editing(input: Option<(bool, bool, bool, Input)>, text: &mut ShapedString, position: &mut TextPosition) -> bool {
    match input {
        Some((false, false, _, Input::Special(ncurses::KEY_BACKSPACE))) => if !text.at_beginning(position) {
            text.delete_left(position);
        },
        Some((false, false, _, Input::Special(ncurses::KEY_DC))) if !text.at_end(position) => {
            text.delete_right(position);
        },
        Some((false, false, _, Input::Character(chr))) if !chr.is_control() => {
            text.insert(position, chr);
        },
        _ => {
            return false;
        }
    }
    true
}

#[derive(Copy, Clone, Debug)]
enum Direction {
    Left,
    Right,
    Up,
    Down
}

impl Direction {
    fn is_horizontal(self) -> bool {
        match self {
            Direction::Left | Direction::Right => true,
            Direction::Up | Direction::Down => false
        }
    }
}

enum Skip {
    One,
    Many,
    All
}

// FIXME: read terminfo instead of using hardcoded values
fn handle_navigation<'a, F: FnOnce(Direction, Skip) -> Option<&'a ShapedString>>(input: Option<(bool, bool, bool, Input)>, text: &'a ShapedString, position: &mut TextPosition, navigate: F) -> bool {
    // Parse the keystroke into its meaning. If we can do the action within the cell, do it and immediately return.
    let (direction, skip) = match input {
        Some(key!([Shift +] KEY_LEFT)) if !text.at_beginning(position) => {
            text.move_left(position);
            return false;
        },
        Some(key!([Shift +] KEY_HOME)) if !text.at_beginning(position) => {
            *position = TextPosition::beginning();
            return false;
        },
        Some(key!([Ctrl +] [Shift +] KEY_LEFT)) => (Direction::Left, Skip::One), // [Ctrl +] Left
        Some(key!([Shift +] KEY_HOME))          => (Direction::Left, Skip::All),

        Some(key!([Shift +] KEY_RIGHT)) if !text.at_end(position) => {
            text.move_right(position);
            return false;
        },
        Some(key!([Shift +] KEY_END)) if !text.at_end(position) => {
            *position = TextPosition::end(text);
            return false;
        },
        Some(key!([Ctrl +] [Shift +] KEY_RIGHT)) => (Direction::Right, Skip::One), // [Ctrl +] Right
        Some(key!([Shift +] KEY_END))            => (Direction::Right, Skip::All),

        Some(key!([Ctrl +] [Shift +] KEY_UP))        => (Direction::Up, Skip::One), // [Ctrl +] Up
        Some(key!([Shift +] KEY_PPAGE)) => (Direction::Up, Skip::Many), // PageUp
        Some(key!(Ctrl + [Shift +] KEY_HOME))   => (Direction::Up, Skip::All), // Ctrl + Home

        Some(key!([Ctrl +] [Shift +] KEY_DOWN))      => (Direction::Down, Skip::One), // [Ctrl +] Down
        Some(key!([Shift +] KEY_NPAGE)) => (Direction::Down, Skip::Many), // PageDown
        Some(key!(Ctrl + [Shift +] KEY_END))    => (Direction::Down, Skip::All), // Ctrl + End

        _ => {
            return false
        }
    };

    // Try to move cells
    if let Some(new_text) = navigate(direction, skip) {
        match direction {
            Direction::Up | Direction::Down => new_text.move_vert(position),
            Direction::Left => *position = TextPosition::end(new_text),
            Direction::Right => *position = TextPosition::beginning()
        }
        false
    } else {
        // If we failed, go as far as we can within the cell
        match direction {
            Direction::Up | Direction::Down => { },
            Direction::Left => *position = TextPosition::beginning(),
            Direction::Right => *position = TextPosition::end(text)
        }
        direction.is_horizontal()
    }
}

const HELP_TEXT: &str = include_str!("help.md");
const READ_ONLY_EDIT_MSG: Cow<str> = Cow::Borrowed("Edit forbidden: document opened in read-only mode");

fn main() {
    let arg_matches = clap::App::new("CSVsheet")
                                    .version("0.1")
                                    .author("Jonathan S <gereeter+code@gmail.com>")
                                    .about("View and edit CSV/DSV/TSV files")
                                    .arg(clap::Arg::with_name("delimiter")
                                        .short("d")
                                        .long("delimiter")
                                        .help("Sets the delimiter to split a line into records")
                                        .takes_value(true))
                                    .arg(clap::Arg::with_name("read-only")
                                        .long("read-only")
                                        .help("Open the file in view-only mode where edits are forbidden"))
                                    .arg(clap::Arg::with_name("FILE")
                                        .help("Sets the file to view/edit")
                                        .required(true)
                                        .index(1))
                                    .get_matches();

    let file_name_arg = arg_matches.value_of_os("FILE").unwrap();
    let file_name = std::fs::canonicalize(&file_name_arg).expect("Unable to reach file");

    let read_only = arg_matches.is_present("read-only");
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
                        .map(|s| ShapedString::from_string(SmallString::from_str(s)))
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

    // TODO: check for errors!
    let mut window = unsafe { curses::Window::init_screen() };
    let mut input_stream = unsafe { input::InputStream::init(&mut window) };

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

    let mut mode = Mode::Normal;
    let mut undo_state = UndoState::new();
    let mut pre_paste_undos = Vec::new();
    let mut inside_paste = false;

    let mut startup = true;
    loop {
        // As a safety feature, make sure that we don't accidentally let edits through in read-only mode
        if read_only && document.modified {
            drop(input_stream);
            drop(window);
            panic!("BUG: an edit was made while in read-only mode!");
        }

        let mut redraw = false;
        let mut new_mode = Mode::Normal;
        let mut warn_message: Option<Cow<'static, str>> = if startup {
            startup = false;
            Some("Welcome to CSVsheet. Press F1 for help.".into())
        } else {
            None
        };

        let input = input_stream.get(&mut window).ok();

        if let Some(key!(KEY_RESIZE)) = input {
            let (new_height, new_width) = window.get_max_yx();
            height = new_height as usize;
            width = new_width as usize;
            screen_x = cmp::min(screen_x, width);
            screen_y = cmp::min(screen_y, height);
            window.set_clear_ok(true);
            redraw = true;
        }

        // Keys with modifiers (at least on xterm, utf8 mode)
        //        Left     Up     Right     Down     Delete  Backspace
        // None   KEY_LEFT KEY_UP KEY_RIGHT KEY_DOWN KEY_DC  KEY_BACKSPACE
        // Shift  KeySLeft KeySR  KeySRight KeySF    KeySDC  KEY_BACKSPACE
        // Ctrl   553      574    568       531      525     \u{7f}
        // Both   554      575    569       532      526     \u{7f}
        // Alt    551      572    564       529      523     \u{88}
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

                        if document_data[row].iter().any(|str| str.text.contains(&*query.text)) {
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
                } else if let Some(key!('\u{1b}')) = input { // Escape
                    let cursor_row = document.views.top().rows[cursor.row_index];
                    document.views.pop();
                    cursor.row_index = document.views.top().rows.iter().position(|&row| row == cursor_row).expect("BUG: unfiltered view does not contain cursor!");
                    new_mode = Mode::Normal;
                    redraw = true;
                } else if let Some(key!('\n')) = input {
                    new_mode = Mode::Normal;
                    redraw = true;
                } else if let Some(key!(KEY_EXIT)) | Some(key!(Ctrl + 'q')) = input { // Ctrl + Q
                    undo_state.prepare_edit(None, &document, &cursor);
                    if document.modified {
                        new_mode = Mode::Quitting;
                        redraw = true;
                    } else {
                        break;
                    }
                } else {
                    new_mode = Mode::Filter { query, query_pos };
                }
            },
        Mode::Normal => {
            // Undo management
            // TODO: don't duplicate the knowledge of what keys do what
            match input {
                Some(key!([Shift +] KEY_DC)) | Some(key!(KEY_BACKSPACE)) => if read_only {
                    warn_message = Some(READ_ONLY_EDIT_MSG);
                } else {
                    undo_state.prepare_edit(Some(EditType::Delete), &document, &cursor);
                },
                Some((false, false, false, Input::Character(c))) if !c.is_control() => if read_only {
                    warn_message = Some(READ_ONLY_EDIT_MSG);
                } else {
                    undo_state.prepare_edit(Some(EditType::Insert), &document, &cursor);
                },
                Some((_, _, _, Input::Special(ncurses::KEY_LEFT))) | Some((_, _, _, Input::Special(ncurses::KEY_RIGHT))) |
                Some((_, _, _, Input::Special(ncurses::KEY_UP))) | Some((_, _, _, Input::Special(ncurses::KEY_DOWN))) |
                Some((_, _, _, Input::Special(ncurses::KEY_HOME))) | Some((_, _, _, Input::Special(ncurses::KEY_END))) |
                Some((_, _, _, Input::Special(ncurses::KEY_PPAGE))) | Some((_, _, _, Input::Special(ncurses::KEY_NPAGE))) => {
                    undo_state.prepare_edit(None, &document, &cursor);
                },
                _ => { }
            }

            // Editing
            if !read_only {
                let cell = &mut document.data[document.views.top().rows[cursor.row_index]][document.views.top().cols[cursor.col_index]];
                let old_cell_width = cell.total_width;
                let column_width = document.column_widths[document.views.top().cols[cursor.col_index]];
                let changed = handle_editing(input, cell, &mut cursor.in_cell_pos);
                if changed {
                    document.modified = true;
                    redraw = true;
                    // The first check ensures correctness on deletion, while the second check is for insertions.
                    if old_cell_width == column_width || cell.total_width > column_width {
                        document.resize_column(document.views.top().cols[cursor.col_index]);
                    }
                }
            }
            // Navigation
            if !inside_paste {
                let mut new_pos = cursor.in_cell_pos.clone();
                try_fit_x = handle_navigation(input, get_cell(&document, &cursor), &mut new_pos, |dir, skip| {
                    match dir {
                        Direction::Left if cursor.col_index > 0 => match skip {
                            Skip::Many | Skip::All => {
                                cursor.col_index = 0;
                                cursor.cell_display_column = 0;
                            },
                            Skip::One => {
                                cursor.col_index -= 1;
                                cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                            }
                        },
                        Direction::Right if cursor.col_index + 1 < document.views.top().cols.len() => match skip {
                            Skip::Many | Skip::All => {
                                cursor.col_index = document.views.top().cols.len() - 1;
                                cursor.cell_display_column = document.views.top().cols[..cursor.col_index].iter().map(|&col_id| document.column_widths[col_id]).sum::<usize>() + 3 * cursor.col_index;
                            },
                            Skip::One => {
                                cursor.col_index += 1;
                                cursor.cell_display_column += document.column_widths[document.views.top().cols[cursor.col_index - 1]] + 3;
                            }
                        },
                        Direction::Up if cursor.row_index > 0 => match skip { // TODO: consider jumping farther/over empty spots for Ctrl + Up?
                            Skip::All => {
                                cursor.row_index = 0;
                            },
                            Skip::Many => {
                                let page_size = height - document.views.top().headers;
                                cursor.row_index = cursor.row_index.saturating_sub(page_size);
                            },
                            Skip::One => {
                                cursor.row_index -= 1;
                            }
                        },
                        Direction::Down if cursor.row_index + 1 < document.views.top().rows.len() => match skip {
                            Skip::All => {
                                cursor.row_index = document.views.top().rows.len() - 1;
                            },
                            Skip::Many => {
                                let page_size = height - document.views.top().headers;
                                cursor.row_index = cmp::min(cursor.row_index + page_size, document.views.top().rows.len() - 1);
                            },
                            Skip::One => {
                                cursor.row_index += 1;
                            }
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
            }
        match input {
            Some(key!('\t'))  => {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                if cursor.col_index + 1 == document.views.top().cols.len() && !read_only {
                    // TODO: is creating a new column really the right behaviour?
                    let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                        upd_view.cols.insert(index + 1, new_col_id);
                    }
                    undo_state.push(UndoOp::DeleteCol(new_col_id));
                    redraw = true;
                }
                if cursor.col_index + 1 < document.views.top().cols.len() {
                    cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                    cursor.col_index += 1;
                    // TODO: or jump to the beginning?
                    cursor.in_cell_pos = TextPosition::end(get_cell(&document, &cursor));
                }
            },
            Some(key!('\n')) => {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                if cursor.row_index + 1 == document.views.top().rows.len() && !read_only {
                    let new_row_id = document.insert_row(document.row_numbers[current_row_id] + 1);
                    for upd_view in document.views.iter_mut() {
                        let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                        upd_view.rows.insert(index + 1, new_row_id);
                    }
                    undo_state.push(UndoOp::DeleteRow(new_row_id));
                    redraw = true;
                }
                if cursor.row_index + 1 < document.views.top().rows.len() {
                    cursor.row_index += 1;
                    cursor.col_index = data_entry_start_index;
                    cursor.cell_display_column = data_entry_start_display_column;
                    // TODO: or jump to the beginning
                    cursor.in_cell_pos = TextPosition::end(get_cell(&document, &cursor));
                }
            },
            Some((false, false, false, Input::Special(2000))) => { // Start bracketed paste
                undo_state.prepare_edit(None, &document, &cursor);
                pre_paste_undos = std::mem::replace(&mut undo_state.undo_stack, Vec::new());
                inside_paste = true;
                data_entry_start_index = cursor.col_index;
                data_entry_start_display_column = cursor.cell_display_column;
            },
            Some((false, false, false, Input::Special(2001))) => { // End bracketed paste
                undo_state.prepare_edit(None, &document, &cursor);
                inside_paste = false;
                let paste_ops = std::mem::replace(&mut undo_state.undo_stack, pre_paste_undos);
                if !paste_ops.is_empty() {
                    undo_state.push(UndoOp::Group(paste_ops));
                }
                pre_paste_undos = Vec::new();
                redraw = true;
            },
            Some(_) if inside_paste => { }, // Everything past this point is special actions, so ignore them
            Some(key!(KEY_RESIZE)) => {
                let (new_height, new_width) = window.get_max_yx();
                height = new_height as usize;
                width = new_width as usize;
                screen_x = cmp::min(screen_x, width);
                screen_y = cmp::min(screen_y, height);
                window.set_clear_ok(true);
                redraw = true;
            },
            Some(key!(KEY_F1)) => {
                undo_state.prepare_edit(None, &document, &cursor);
                new_mode = Mode::Help;
                redraw = true;
            },
            Some(key!(KEY_EXIT)) | Some(key!(Ctrl + [Shift +] 'q')) => { // Ctrl + Q
                undo_state.prepare_edit(None, &document, &cursor);
                if document.modified {
                    new_mode = Mode::Quitting;
                    redraw = true;
                } else {
                    break;
                }
            },
            // FIXME: This triggers on Ctrl + Z /and/ Ctrl + Shift + Z, but we'd like the latter to be redo. For now we settle for Ctrl + Alt + Z,
            // but it would be much much better to detect the shift key.
            Some(key!(KEY_UNDO)) | Some(key!(Ctrl + [Shift +] 'z')) => { // Ctrl + [Shift +] Z
                undo_state.prepare_edit(None, &document, &cursor);
                if let Some(op) = undo_state.undo_stack.pop() {
                    let inverse_op = op.apply_to(&mut document, &mut cursor);
                    undo_state.redo_stack.push(inverse_op);
                    redraw = true;
                    if undo_state.is_pristine() {
                        document.modified = false;
                    }
                } else {
                    warn_message = Some("Nothing to undo.".into());
                }
            },
            Some(key!(KEY_REDO)) | Some(key!(Ctrl + Alt + [Shift +] 'z')) => { // Ctrl + Alt + Z
                undo_state.prepare_edit(None, &document, &cursor);
                if let Some(op) = undo_state.redo_stack.pop() {
                    let inverse_op = op.apply_to(&mut document, &mut cursor);
                    undo_state.undo_stack.push(inverse_op);
                    redraw = true;
                    if undo_state.is_pristine() {
                        document.modified = false;
                    }
                } else {
                    warn_message = Some("Nothing to redo.".into());
                }
            },
            Some(key!(KEY_COPY)) | Some(key!(Ctrl + [Shift +] 'c')) => { // Ctrl + C
                warn_message = Some("Nothing selected to copy. [NOTE: Selection is currently unimplemented.]".into());
            },
            Some(key!(KEY_FIND)) | Some(key!(Ctrl + [Shift +] 'f')) => { // Ctrl + F
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
            Some(key!(Ctrl + [Shift +] 'k')) => { // Ctrl + K
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
                } else {
                    warn_message = Some("Cannot hide the only row on the screen.".into());
                }
            },
            Some(key!(Ctrl + Alt + [Shift +] 'k')) => if read_only { // Ctrl + Alt + K
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                if document.views.top().rows.len() > 1 {
                    let current_row_id = document.views.top().rows[cursor.row_index];
                    document.delete_row(current_row_id);
                    undo_state.push(UndoOp::InsertRow(current_row_id));

                    if cursor.row_index >= document.views.top().rows.len() {
                        cursor.row_index -= 1;
                        get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                    }
                    redraw = true;
                } else {
                    warn_message = Some("Cannot delete the only row on the screen.".into());
                }
            },
            Some(key!(Ctrl + [Shift +] 't')) => if read_only { // Ctrl + T
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                undo_state.push(UndoOp::DeleteCol(new_col_id));
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index + 1, new_col_id);
                }
                cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                cursor.row_index = 0;
                cursor.col_index += 1;
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(key!(Ctrl + [Shift +] 'w')) => { // Ctrl + W
                undo_state.prepare_edit(None, &document, &cursor);
                if document.views.top().cols.len() > 1 {
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
                } else {
                    warn_message = Some("Cannot hide the only column on the screen.".into());
                }
            },
            Some(key!(Ctrl + Alt + [Shift +] 'w')) => if read_only { // Ctrl + Alt + W
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                if document.views.top().cols.len() > 1 {
                    let current_col_id = document.views.top().cols[cursor.col_index];
                    document.delete_col(current_col_id);
                    undo_state.push(UndoOp::InsertCol(current_col_id));

                    if cursor.col_index >= document.views.top().cols.len() {
                        cursor.col_index -= 1;
                        cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                        cursor.in_cell_pos = TextPosition::end(get_cell(&document,&cursor));
                    }
                    redraw = true;
                } else {
                    warn_message = Some("Cannot delete the only column on the screen.".into());
                }
            },
            Some(key!(KEY_CANCEL)) | Some(key!('\u{1b}')) => { // Escape
                undo_state.prepare_edit(None, &document, &cursor);
                if document.views.is_at_base() {
                    warn_message = Some("No views to pop. Press Ctrl+Q to exit.".into());
                } else {
                    let cursor_row = document.views.top().rows[cursor.row_index];
                    let cursor_col = document.views.top().cols[cursor.col_index];
                    document.views.pop();
                    cursor.row_index = document.views.top().rows.iter().position(|&row| row == cursor_row).expect("BUG: old view does not contain cursor!");
                    cursor.col_index = document.views.top().cols.iter().position(|&col| col == cursor_col).expect("BUG: old view does not contain cursor!");
                    cursor.cell_display_column = document.views.top().cols.iter().take(cursor.col_index).map(|&col| document.column_widths[col]).sum::<usize>() + 3 * cursor.col_index;
                    redraw = true;
                }
            },
            Some(key!(KEY_SAVE)) | Some(key!(Ctrl + [Shift +] 's')) if !read_only => { // Ctrl + S
                undo_state.prepare_edit(None, &document, &cursor);
                // TODO: track file moves and follow the file
                match document.save_to(&file_name) {
                    Ok(_) => {
                        undo_state.pristine_state = Some(undo_state.undo_stack.len());
                    },
                    Err(err) => {
                        warn_message = Some(format!("Failed to save: {}", err).into());
                    }
                }
            },
            // ------------------------------------------ Navigation ----------------------------------------------
            Some(key!([Ctrl +] Alt + [Shift +] KEY_LEFT)) => if read_only { // [Ctrl +] Alt + Left
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id]);
                undo_state.push(UndoOp::DeleteCol(new_col_id));
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index, new_col_id);
                }
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(key!([Ctrl +] Alt + [Shift +] KEY_RIGHT)) => if read_only { // [Ctrl +] Alt + Right
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_col_id = document.views.top().cols[cursor.col_index];
                let new_col_id = document.insert_col(document.col_numbers[current_col_id] + 1);
                undo_state.push(UndoOp::DeleteCol(new_col_id));
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.cols.iter().position(|&col_id| col_id == current_col_id).expect("Older view not superset of new view!");
                    upd_view.cols.insert(index + 1, new_col_id);
                }
                cursor.cell_display_column += document.column_widths[current_col_id] + 3;
                cursor.col_index += 1;
                cursor.in_cell_pos = TextPosition::beginning();
                redraw = true;
            },
            Some(key!([Ctrl +] Alt + [Shift +] KEY_UP)) => if read_only { // [Ctrl +] Alt + Up
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let new_row_id = document.insert_row(document.row_numbers[current_row_id]);
                undo_state.push(UndoOp::DeleteRow(new_row_id));
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                    upd_view.rows.insert(index, new_row_id);
                }
                get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                redraw = true;
            },
            Some(key!([Ctrl +] Alt + [Shift +] KEY_DOWN)) => if read_only { // [Ctrl +] Alt + Down
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let new_row_id = document.insert_row(document.row_numbers[current_row_id] + 1);
                undo_state.push(UndoOp::DeleteRow(new_row_id));
                for upd_view in document.views.iter_mut() {
                    let index = upd_view.rows.iter().position(|&row_id| row_id == current_row_id).expect("Older view not superset of new view!");
                    upd_view.rows.insert(index + 1, new_row_id);
                }
                cursor.row_index += 1;
                get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                redraw = true;
            },
            Some(key!(Ctrl + [Shift +] KEY_DC)) => if read_only { // Ctrl + Delete
                warn_message = Some(READ_ONLY_EDIT_MSG);
            } else {
                undo_state.prepare_edit(None, &document, &cursor);
                let current_row_id = document.views.top().rows[cursor.row_index];
                let current_col_id = document.views.top().cols[cursor.col_index];

                let mut changed = false;

                // Delete row if empty
                if document.views.top().rows.len() > 1 && document.data[current_row_id].iter().all(|cell| cell.text.is_empty()) {
                    document.delete_row(current_row_id);
                    if cursor.row_index >= document.views.top().rows.len() {
                        cursor.row_index -= 1;
                        get_cell(&document, &cursor).move_vert(&mut cursor.in_cell_pos);
                    }
                    changed = true;
                }

                // Delete column if empty
                if document.views.top().cols.len() > 1 && document.data.iter().all(|row| row[current_col_id].text.is_empty()) {
                    document.delete_col(current_col_id);
                    if cursor.col_index >= document.views.top().cols.len() {
                        cursor.col_index -= 1;
                        cursor.cell_display_column -= document.column_widths[document.views.top().cols[cursor.col_index]] + 3;
                        cursor.in_cell_pos = TextPosition::end(get_cell(&document,&cursor));
                    }
                    changed = true;
                }

                if changed {
                    redraw = true;
                } else {
                    warn_message = Some("Only rows/columns that are empty can be deleted.".into());
                }
            },
            // ---------------------- Mouse Input ------------------------
            Some(key!(KEY_MOUSE)) => {
                let event = match curses::get_mouse() {
                    Ok(event) => event,
                    Err(_) => continue,
                };
                // TODO: when are multiple bits set?
                if event.bstate & ncurses::BUTTON1_PRESSED as ncurses::mmask_t != 0 {
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
                // TODO: allow scrolling past what fits the cursor on the screen
                } else if event.bstate & ncurses::BUTTON4_PRESSED as ncurses::mmask_t != 0 {
                    if offset_y > 0 {
                        offset_y -= 1;
                        redraw = true;
                    }
                } else if event.bstate & ncurses::BUTTON5_PRESSED as ncurses::mmask_t != 0 {
                    offset_y += 1;
                    redraw = true;
                }
            },
            // ------------------------------------- Fallback/Debugging ------------------------------------
            _ => { }
        } },
            Mode::Quitting => match input {
                Some(key!([Shift +] 'y')) => {
                    // TODO: track renames and follow the file
                    // TODO: display error to the user
                    match document.save_to(&file_name) {
                        Ok(_) => break,
                        Err(err) => {
                            new_mode = Mode::Normal;
                            warn_message = Some(format!("Failed to save: {}", err).into());
                        }
                    }
                },
                Some(key!([Shift +] 'n')) => {
                    break;
                },
                Some(key!('\u{1b}')) => { // Escape
                    new_mode = Mode::Normal;
                },
                _ => {
                    new_mode = Mode::Quitting;
                }
            },
            Mode::Help => match input {
                Some(key!('\u{1b}')) | Some(key!(KEY_EXIT)) | Some(key!(Ctrl + [Shift +] 'q')) => { // Escape or Ctrl + Q
                    new_mode = Mode::Normal;
                    redraw = true;
                },
                _ => {
                    new_mode = Mode::Help;
                }
            }
        }
        mode = new_mode;

        let rows_shown = height - 1;

        // Scrolling
        let target_x = cursor.cell_display_column + cursor.in_cell_pos.display_column;
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

        if !inside_paste {
            if redraw {
                window.erase();

                if let Mode::Help = mode {
                    window.mv_add_str(0, 0, HELP_TEXT);
                } else {
                    for y in 0..document.views.top().headers {
                        display_row(&document, document.views.top().rows[y], &mut window, y, offset_x, offset_x + width, A_BOLD());
                    }

                    for (row_i, &row) in document.views.top().rows.iter().skip(offset_y + document.views.top().headers).take(rows_shown - document.views.top().headers).enumerate() {
                        display_row(&document, row, &mut window, row_i + document.views.top().headers, offset_x, offset_x + width, A_NORMAL());
                    }
                }
            }

            window.set_attrs(A_NORMAL());
            window.mv(height as i32 - 1, 0);
            window.clear_to_end_of_line();
            if let Mode::Filter { ref query, .. } = mode {
                window.mv_add_str(height as i32 - 1, 0, "Find rows containing: ");
                window.add_str(&query.text);
            } else if let Mode::Quitting = mode {
                window.mv_add_str(height as i32 - 1, 0, "Save before quitting [y/n/Esc]? ");
            } else if let Some(message) = warn_message {
                window.mv_add_str(height as i32 - 1, ((width.saturating_sub(message.len())) / 2) as i32, &message);
            } else {
                // TODO(efficiency): avoid allocations
                let status = format!(
                    "[ row {}/{}, col {}/{}, byte {}/{}, last_input: {:?} ]",
                    document.row_numbers[document.views.top().rows[cursor.row_index]] + 1, document.height(),
                    document.col_numbers[document.views.top().cols[cursor.col_index]] + 1, document.width(),
                    cursor.in_cell_pos.grapheme_cursor.cur_cursor(), get_cell(&document, &cursor).text.len(), // TODO: count graphemes?
                    input
                );
                window.mv_add_str(height as i32 - 1, ((width.saturating_sub(status.len())) / 2) as i32, &status);
            }

            if let Mode::Normal = mode {
                window.mv(screen_y as i32, screen_x as i32);
            } else if let Mode::Filter { ref query_pos, .. } = mode {
                window.mv(height as i32 - 1, 22 + query_pos.display_column as i32);
            }
            window.refresh();
        }
    }
}
