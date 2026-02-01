#![warn(missing_docs)]
//! Terminal input with Windows style editing and optional history.
//!
//! Provides the ability to accept a line of input text from the
//! terminal. The input text can be edited using common, obvious
//! keys such as backspace, delete, the arrow keys, home, and end.
//! The editing command style is similar to that used in most
//! GUI apps and the various Windows command shells.
//!
//! Several optional features can be customized and/or enabled by
//! providing a set of options, including:
//! - An optional prompt may be specified (currently a single character)
//! - Optional initial text may be specified (e.g., whitespace prefix
//!   for "auto-indent")
//! - The line history stack feature may be enabled or disabled.
//!
//! The intent is to be a simple, easy to use (both for the programmer
//! and the end user) utility similar to libraries like readline,
//! rustyline, etc., but not requiring the end user to know emacs or vi
//! shortcuts.
mod history_stack;
mod renderer;

use std::collections::HashMap;
use std::io::{self, Write, BufRead};
use std::ops::ControlFlow;
use std::sync::LazyLock;
use std::time::Duration;

use crossterm::cursor::{self};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;

use regex::Regex;

use unicode_segmentation::UnicodeSegmentation;

use unicode_width::UnicodeWidthChar;

use crate::history_stack::HistoryStack;
use crate::renderer::Coord2D;
use crate::renderer::DimWH;
use crate::renderer::View;

/// The `LineEdit` trait allows for accepting a line of input text.
///
/// Implementors of the `LineEdit` trait are called 'line editors'.
///
/// Line editors are defined by one required method [`read_line()`].
/// Each call to [`read_line()`] will attempt to accept input text
/// characters into a provided buffer, possibly with the line editor's
/// behavior modified by a set of specified options.
///
/// [`read_line()`]: LineEdit::read_line
pub trait LineEdit {
    /// Accept input text characters into a provided buffer,
    /// returning the number of bytes read.
    /// # Errors
    ///
    /// Will return `io::Error` if an error is encountered reading a line
    fn read_line(
        &mut self,
        buffer: &mut String,
        options: Option<&EditorOptions>,
    ) -> io::Result<usize>;
}

/// A `LineEdit` implementation accepting user input from a terminal.
///
/// `LineEditor` implements a set of line editing commands and optional
/// line history functionality.
///
///
#[derive(Debug, Clone, PartialEq)]
pub struct LineEditor {
    line: String,
    line_cursor: usize,
    unicode: String,
    unicode_cursor: Option<usize>,
    history: Option<HistoryStack>,
    key_bindings: KeyMap,
}

/// Line editor options.
#[derive(Debug, Default, Clone)]
pub struct EditorOptions {
    /// Prompt character displayed before user input
    pub prompt: Option<char>,
    /// 'true' if line history should be enbled, false if not
    pub history: bool,
    /// Initial input buffer text.
    pub prefill: Option<String>,
}

/// Returns the native text line terminator sequence for the
/// execution environment. This will be "\r\n" when called on
/// a Windows sytem, "\n" otherwise.
#[must_use]
pub fn native_eol() -> &'static str {
    if std::env::consts::FAMILY == "windows" { "\r\n" } else { "\n" }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum SpanType {
    Empty,
    Word,
    Space,
    Symbol,
    Other,
}

static SYMBOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{S}\p{P}]").unwrap());

fn span_type(s: &str) -> SpanType {
    if s.is_empty() {
        return SpanType::Empty;
    }
    if s.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        SpanType::Word
    } else if s.starts_with(char::is_whitespace) {
        SpanType::Space
    } else if SYMBOL.is_match(s) {
        SpanType::Symbol
    } else {
        SpanType::Other
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        LineEditor {
            line: String::with_capacity(128),
            line_cursor: 0,
            unicode: String::with_capacity(6),
            unicode_cursor: None,
            history: None,
            key_bindings: KeyMap::default(),
        }
    }
}
impl LineEditor {
    /// Creates a new `LineEditor`.
    ///
    /// The new instance will not allocate space to store
    /// history until a read is done with history enabled.
    #[must_use]
    pub fn new() -> LineEditor {
        LineEditor { ..Default::default() }
    }

    /*
    #[cfg(test)]
    fn with_history(history: Option<HistoryStack>) -> LineEditor {
        LineEditor { history, ..Default::default() }
    }
    */

    /// Returns the terminal size (columns, rows).
    ///
    /// The dimensions are 1 based.
    ///
    /// # Errors
    ///
    /// Will return `io::Error` if one is encountered determining the size.
    pub fn terminal_size() -> io::Result<(u16, u16)> {
        terminal::size()
    }

    fn accept_line(
        &mut self,
        output_buffer: &mut String,
        options: Option<&EditorOptions>,
    ) -> io::Result<usize> {
        let term_size: DimWH = Self::terminal_size()?.into();
        let (_, first_display_line) = cursor::position()?;

        // View has Drop impl to ensure terminal reset to cooked
        // and cursor not hidden.
        let prompt = options.and_then(|o| o.prompt);
        let mut view = View::new(term_size, first_display_line, prompt);
        terminal::enable_raw_mode()?;

        let disable_history = !options.is_some_and(|o| o.history);
        if let Some(history) = &mut self.history {
            history.disabled = disable_history;
        } else if !disable_history {
            self.history = Some(HistoryStack::new());
        }

        if let Some(prefill) = options.and_then(|o| o.prefill.as_ref()) {
            self.line.push_str(prefill);
            self.line_cursor = self.line.len();
            view.invalidate();
        }

        view.repaint(self)?;
        while self.pump_event(&mut view)?.is_continue() {
            view.repaint(self)?;
        }

        let _ = self.do_cursor_to_end(&mut view);
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\r\n")?;
        stdout.flush()?;

        let prev_bytes = output_buffer.len();
        output_buffer.push_str(&self.line);
        output_buffer.push_str(native_eol());
        self.line.clear();
        let new_line_capacity = usize::from(view.size().0).next_multiple_of(64);
        self.line.shrink_to(new_line_capacity);
        Ok(output_buffer.len() - prev_bytes)
    }

    fn pump_event(&mut self, view: &mut View) -> io::Result<ControlFlow<()>> {
        let event = event::read()?;
        self.handle_event(view, &event)
    }

    fn handle_event(
        &mut self,
        view: &mut View,
        event: &Event,
    ) -> io::Result<ControlFlow<()>> {
        match event {
            Event::Key(event) if event.is_press() => {
                Ok(self.handle_key_pressed((event.code, event.modifiers), view))
            }
            &Event::Resize(mut w, mut h) => {
                while let Ok(true) = event::poll(Duration::from_millis(50)) {
                    if let Event::Resize(w1, h1) = event::read()? {
                        (w, h) = (w1, h1);
                    }
                }
                let cursor_position: Coord2D = cursor::position()?.into();
                view.resize(DimWH(w, h), cursor_position, self);
                Ok(ControlFlow::Continue(()))
            }
            Event::Key(_)
            | Event::FocusGained
            | Event::FocusLost
            | Event::Mouse(_)
            | Event::Paste(_) => Ok(ControlFlow::Continue(())),
        }
    }

    fn handle_key_pressed(
        &mut self,
        key: (KeyCode, KeyModifiers),
        view: &mut View,
    ) -> ControlFlow<()> {
        let Some(command) = self.key_bindings.get(key) else {
            return ControlFlow::Continue(());
        };

        match command {
            EditCommand::CharInput(ch) => self.do_char_input(view, ch),
            EditCommand::Backspace => self.do_backspace(view),
            EditCommand::Delete => self.do_delete(view),
            EditCommand::HistoryNextBack => self.do_history_next_back(view),
            EditCommand::HistoryNext => self.do_history_next(view),
            EditCommand::Escape => self.do_escape(view),
            EditCommand::CursorBack => self.do_cursor_back(view),
            EditCommand::CursorForward => self.do_cursor_forward(view),
            EditCommand::CursorToStart => self.do_cursor_to_start(view),
            EditCommand::CursorToEnd => self.do_cursor_to_end(view),
            EditCommand::DeleteToStart => self.do_delete_to_start(view),
            EditCommand::DeleteToEnd => self.do_delete_to_end(view),
            EditCommand::AcceptInput => self.do_accept_input(view),
            EditCommand::HistoryRFind => self.do_history_rfind(view),
            EditCommand::HistoryFind => self.do_history_find(view),
            EditCommand::Indent => self.do_indent(view),
            EditCommand::Dedent => self.do_dedent(view),
            EditCommand::CursorSpanBack => self.do_cursor_span_back(view),
            EditCommand::CursorSpanForward => self.do_cursor_span_forward(view),
            EditCommand::DeleteSpanBack => self.do_delete_span_back(view),
            EditCommand::DeleteSpanForward => self.do_delete_span_forward(view),
            EditCommand::UnicodeInputMode => self.do_unicode_input_mode(view),
        }
    }

    fn do_accept_input(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            let cp = u32::from_str_radix(&self.unicode, 16).unwrap();
            self.unicode.clear();
            self.unicode_cursor = None;
            view.invalidate();

            return char::from_u32(cp)
                .map_or(ControlFlow::Continue(()), |ch| {
                    self.do_char_input(view, ch)
                });
        } else if let Some(ref mut history) = self.history
            && !history.disabled
        {
            history.rewind();
            if !self.line.is_empty()
                && history.last().is_none_or(|last| last != self.line)
            {
                history.push(self.line.clone());
            }
        }
        ControlFlow::Break(())
    }

    fn do_escape(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            self.unicode_cursor = None;
            self.unicode.clear();
            view.invalidate();
        } else if let Some(draft) =
            self.history.as_mut().and_then(HistoryStack::rewind)
        {
            self.line.replace_range(.., &draft);
            self.line_cursor = self.line.len();
            view.invalidate();
        }
        ControlFlow::Continue(())
    }

    fn do_history_next(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No history during Unicode input
            return ControlFlow::Continue(());
        }

        if let Some(history_line) =
            self.history.as_mut().and_then(|h| h.next_newer())
        {
            self.line.replace_range(.., history_line);
            self.line_cursor = self.line.len();
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_history_next_back(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No history during Unicode input
            return ControlFlow::Continue(());
        }

        if let Some(line) =
            self.history.as_mut().and_then(|h| h.next_older(&self.line))
        {
            self.line.replace_range(.., line);
            self.line_cursor = self.line.len();
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_char_input(&mut self, view: &mut View, c: char) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                if !c.is_ascii_hexdigit() || *unicode_cursor == 6 {
                    return ControlFlow::Continue(());
                }
                (unicode_cursor, &mut self.unicode)
            } else {
                // if char is zero width, but no previous chars exist to
                //  which it can  be combined, do nothing (i.e., don't accept
                // the input)
                if c != '\t'
                    && c.width().unwrap_or(0) == 0
                    && !self.line[..self.line_cursor]
                        .chars()
                        .rev()
                        .take_while(|c| *c != '\t')
                        .any(|c| c.width().unwrap_or(0) > 0)
                {
                    return ControlFlow::Continue(());
                }
                (&mut self.line_cursor, &mut self.line)
            };

        text.insert(*cursor, c);
        *cursor += c.len_utf8();
        view.invalidate();

        ControlFlow::Continue(())
    }

    fn do_backspace(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        if *cursor != 0
            && let Some((i, _)) = text[..*cursor].char_indices().next_back()
        {
            text.remove(i);
            *cursor = i;
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_cursor_back(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        if *cursor != 0
            && let Some((prev_idx, _)) = text[..*cursor]
                .char_indices()
                .rfind(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
        {
            *cursor = prev_idx;
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_cursor_forward(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        // If aleady at end, nothing to do
        if *cursor != text.len() {
            let next_idx = text[*cursor..]
                .char_indices()
                .skip(1)
                .find(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
                .map_or_else(|| text.len(), |(i, _)| i + *cursor);
            *cursor = next_idx;
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_delete(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        // if at end of buffer, nothing to do
        let cur_idx = *cursor;
        if cur_idx != text.len() {
            let next_idx = text[*cursor..]
                .char_indices()
                .skip(1)
                .find(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
                .map_or_else(|| text.len(), |(i, _)| i + *cursor);
            text.replace_range(*cursor..next_idx, "");
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_delete_to_start(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        // if at start of buffer, nothing to do
        if *cursor != 0 {
            text.replace_range(..*cursor, "");
            *cursor = 0;
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_delete_to_end(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        // if at end of buffer, nothing to do
        if *cursor != text.len() {
            text.replace_range(*cursor.., "");
            *cursor = text.len();
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_cursor_to_start(&mut self, view: &mut View) -> ControlFlow<()> {
        let cursor =
            self.unicode_cursor.as_mut().unwrap_or(&mut self.line_cursor);

        if *cursor != 0 {
            *cursor = 0;
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_cursor_to_end(&mut self, view: &mut View) -> ControlFlow<()> {
        let (cursor, text) =
            if let Some(unicode_cursor) = self.unicode_cursor.as_mut() {
                (unicode_cursor, &mut self.unicode)
            } else {
                (&mut self.line_cursor, &mut self.line)
            };

        if *cursor != text.len() {
            *cursor = text.len();
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_history_find(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No history during Unicode input
            return ControlFlow::Continue(());
        }

        if let Some(line) = self
            .history
            .as_mut()
            .and_then(|h| h.find(&self.line[..self.line_cursor]))
        {
            self.line.replace_range(.., line);
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_history_rfind(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No history during Unicode input
            return ControlFlow::Continue(());
        }

        if let Some(line) = self
            .history
            .as_mut()
            .and_then(|h| h.rfind(&self.line[..self.line_cursor]))
        {
            self.line.replace_range(.., line);
            view.invalidate();
        }

        ControlFlow::Continue(())
    }

    fn do_indent(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No indent during Unicode input
            return ControlFlow::Continue(());
        }

        // If the first buffer char is tab ('\t'), insert one additional
        // tab at start of line. If not, insert up to 4 space (' ') chars
        // at start of line, so that leading spaces are the next multiple
        // of four.
        if self.line.starts_with('\t') {
            self.line.insert(0, '\t');
            self.line_cursor += 1;
        } else {
            let leading_spaces =
                self.line.chars().take_while(|c| *c == ' ').count();
            let next_stop = (leading_spaces + 1).next_multiple_of(4);
            let to_add = next_stop - leading_spaces;
            self.line.insert_str(0, &"    "[..to_add]);
            self.line_cursor += to_add;
        }
        view.invalidate();
        ControlFlow::Continue(())
    }

    fn do_dedent(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // No dedent during Unicode input
            return ControlFlow::Continue(());
        }

        // If the first buffer char is tab ('\t'), delete it.
        // If not, delete up to 4 leading spaces so that the
        // number of remaining leading spaces is a multple of four.
        if self.line.starts_with('\t') {
            self.line.remove(1);
            self.line_cursor = self.line_cursor.saturating_sub(1);
            view.invalidate();
        } else if self.line.starts_with(' ') {
            let leading_spaces =
                self.line.chars().take_while(|c| *c == ' ').count();
            let previous_stop = (leading_spaces / 4).saturating_sub(1) * 4;
            let to_remove = leading_spaces.saturating_sub(previous_stop);
            self.line.replace_range(..to_remove, "");
            self.line_cursor = self.line_cursor.saturating_sub(to_remove);
            view.invalidate();
        }
        ControlFlow::Continue(())
    }

    fn do_cursor_span_back(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // Span based commands are NOP during Unicode input
            return ControlFlow::Continue(());
        }

        if self.line_cursor == 0 {
            return ControlFlow::Continue(());
        }

        let mut gr_idxs = self.line[..self.line_cursor]
            .grapheme_indices(true)
            .rev()
            .skip_while(|(_, gr)| span_type(gr) == SpanType::Space);

        self.line_cursor = if let Some((idx, target_span_type)) =
            gr_idxs.next().map(|(idx, gr)| (idx, span_type(gr)))
        {
            gr_idxs
                .take_while(|(_, gr)| span_type(gr) == target_span_type)
                .last()
                .map_or(idx, |(i, _)| i)
        } else {
            0
        };
        view.invalidate();

        ControlFlow::Continue(())
    }

    fn do_cursor_span_forward(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // Span based commands are NOP during Unicode input
            return ControlFlow::Continue(());
        }

        let mut gr_idxs = self
            .line
            .grapheme_indices(true)
            .skip_while(|(i, _)| *i < self.line_cursor);
        let mut current_span_type =
            gr_idxs.next().map_or(SpanType::Empty, |(_, gr)| span_type(gr));
        if current_span_type != SpanType::Empty {
            self.line_cursor = gr_idxs
                .find(|(_, gr)| match span_type(gr) {
                    SpanType::Space => {
                        current_span_type = SpanType::Space;
                        false
                    }
                    st => st != current_span_type,
                })
                .map_or(self.line.len(), |(i, _)| i);
            view.invalidate();
        }
        ControlFlow::Continue(())
    }

    fn do_delete_span_back(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // Span based commands are NOP during Unicode input
            return ControlFlow::Continue(());
        }

        if self.line_cursor == 0 {
            return ControlFlow::Continue(());
        }

        let mut gr_idxs = self.line[..self.line_cursor]
            .grapheme_indices(true)
            .rev()
            .skip_while(|(_, gr)| span_type(gr) == SpanType::Space);
        let (idx, target_span_type) = gr_idxs
            .next()
            .map_or((0, SpanType::Space), |(idx, gr)| (idx, span_type(gr)));

        let span_start = gr_idxs
            .take_while(|(_, gr)| span_type(gr) == target_span_type)
            .last()
            .map_or(idx, |(i, _)| i);
        self.line.replace_range(span_start..self.line_cursor, "");
        self.line_cursor = span_start;
        view.invalidate();
        ControlFlow::Continue(())
    }

    fn do_delete_span_forward(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_some() {
            // Span based commands are NOP during Unicode input
            return ControlFlow::Continue(());
        }

        let mut gr_idxs = self
            .line
            .grapheme_indices(true)
            .skip_while(|(i, _)| *i < self.line_cursor);
        let mut current_span_type =
            gr_idxs.next().map_or(SpanType::Empty, |(_, gr)| span_type(gr));
        if current_span_type != SpanType::Empty {
            let span_end = gr_idxs
                .find(|(_, gr)| match span_type(gr) {
                    SpanType::Space => {
                        current_span_type = SpanType::Space;
                        false
                    }
                    st => st != current_span_type,
                })
                .map_or(self.line.len(), |(i, _)| i);
            self.line.replace_range(self.line_cursor..span_end, "");
            view.invalidate();
        }
        ControlFlow::Continue(())
    }

    fn do_unicode_input_mode(&mut self, view: &mut View) -> ControlFlow<()> {
        if self.unicode_cursor.is_none() {
            self.unicode_cursor = Some(0);
            view.invalidate();
        }
        ControlFlow::Continue(())
    }
}

impl LineEdit for LineEditor {
    fn read_line(
        &mut self,
        buffer: &mut String,
        options: Option<&EditorOptions>,
    ) -> io::Result<usize> {
        self.accept_line(buffer, options)
    }
}

impl<T> LineEdit for T
where
    T: BufRead,
{
    fn read_line(
        &mut self,
        buffer: &mut String,
        _options: Option<&EditorOptions>,
    ) -> io::Result<usize> {
        BufRead::read_line(self, buffer)
    }
}


#[derive(Debug, Copy, Clone, PartialEq)]
enum EditCommand {
    CharInput(char),
    Backspace,
    Delete,
    HistoryNextBack,
    HistoryNext,
    Escape,
    CursorBack,
    CursorForward,
    CursorToStart,
    CursorToEnd,
    DeleteToStart,
    DeleteToEnd,
    AcceptInput,
    HistoryRFind,
    HistoryFind,
    Indent,
    Dedent,
    CursorSpanBack,
    CursorSpanForward,
    DeleteSpanBack,
    DeleteSpanForward,
    UnicodeInputMode,
}

#[derive(Debug, Clone, PartialEq)]
struct KeyMap {
    bindings: HashMap<(KeyCode, KeyModifiers), EditCommand>,
}

impl From<&str> for LineEditor {
    fn from(value: &str) -> Self {
        LineEditor {
            line: value.to_owned(),
            line_cursor: value.len(),
            ..Default::default()
        }
    }
}

impl KeyMap {
    fn get(&self, key: (KeyCode, KeyModifiers)) -> Option<EditCommand> {
        let cmd = self.bindings.get(&key).copied();

        if cmd.is_some() {
            return cmd;
        }

        if let (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) =
            key
        {
            return Some(EditCommand::CharInput(ch));
        }

        None
    }
}

impl Default for KeyMap {
    #[allow(clippy::too_many_lines)]
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // Common
        bindings.insert(
            (KeyCode::Enter, KeyModifiers::NONE),
            EditCommand::AcceptInput,
        );
        bindings
            .insert((KeyCode::Tab, KeyModifiers::NONE), EditCommand::Indent);
        bindings.insert(
            (KeyCode::BackTab, KeyModifiers::SHIFT),
            EditCommand::Dedent,
        );
        bindings.insert(
            (KeyCode::Char('i'), KeyModifiers::CONTROL),
            EditCommand::CharInput('\t'),
        );
        bindings.insert(
            (KeyCode::Backspace, KeyModifiers::NONE),
            EditCommand::Backspace,
        );
        bindings.insert(
            (KeyCode::Char('u'), KeyModifiers::CONTROL),
            EditCommand::UnicodeInputMode,
        );

        // Windows style
        bindings.insert(
            (KeyCode::Left, KeyModifiers::NONE),
            EditCommand::CursorBack,
        );
        bindings.insert(
            (KeyCode::Right, KeyModifiers::NONE),
            EditCommand::CursorForward,
        );
        bindings.insert(
            (KeyCode::Home, KeyModifiers::NONE),
            EditCommand::CursorToStart,
        );
        bindings.insert(
            (KeyCode::Home, KeyModifiers::CONTROL),
            EditCommand::DeleteToStart,
        );
        bindings.insert(
            (KeyCode::End, KeyModifiers::NONE),
            EditCommand::CursorToEnd,
        );
        bindings.insert(
            (KeyCode::End, KeyModifiers::CONTROL),
            EditCommand::DeleteToEnd,
        );
        bindings.insert(
            (KeyCode::Backspace, KeyModifiers::NONE),
            EditCommand::Backspace,
        );
        bindings
            .insert((KeyCode::Delete, KeyModifiers::NONE), EditCommand::Delete);
        bindings.insert(
            (KeyCode::Up, KeyModifiers::NONE),
            EditCommand::HistoryNextBack,
        );
        bindings.insert(
            (KeyCode::Down, KeyModifiers::NONE),
            EditCommand::HistoryNext,
        );
        bindings.insert(
            (KeyCode::F(8), KeyModifiers::NONE),
            EditCommand::HistoryRFind,
        );
        bindings.insert(
            (KeyCode::F(8), KeyModifiers::SHIFT),
            EditCommand::HistoryFind,
        );
        bindings
            .insert((KeyCode::Esc, KeyModifiers::NONE), EditCommand::Escape);
        bindings.insert(
            (KeyCode::Left, KeyModifiers::CONTROL),
            EditCommand::CursorSpanBack,
        );
        bindings.insert(
            (KeyCode::Right, KeyModifiers::CONTROL),
            EditCommand::CursorSpanForward,
        );
        bindings.insert(
            (KeyCode::Backspace, KeyModifiers::CONTROL),
            EditCommand::DeleteSpanBack,
        );
        bindings.insert(
            (KeyCode::Delete, KeyModifiers::CONTROL),
            EditCommand::DeleteSpanForward,
        );

        // Bash/emacs style
        bindings.insert(
            (KeyCode::Char('b'), KeyModifiers::CONTROL),
            EditCommand::CursorBack,
        );
        bindings.insert(
            (KeyCode::Char('f'), KeyModifiers::CONTROL),
            EditCommand::CursorForward,
        );
        bindings.insert(
            (KeyCode::Char('a'), KeyModifiers::CONTROL),
            EditCommand::CursorToStart,
        );
        bindings.insert(
            (KeyCode::Char('e'), KeyModifiers::CONTROL),
            EditCommand::CursorToEnd,
        );
        bindings.insert(
            (KeyCode::Char('k'), KeyModifiers::CONTROL),
            EditCommand::DeleteToEnd,
        );
        bindings.insert(
            (KeyCode::Char('d'), KeyModifiers::CONTROL),
            EditCommand::Delete,
        );
        bindings.insert(
            (KeyCode::Char('p'), KeyModifiers::CONTROL),
            EditCommand::HistoryNextBack,
        );
        bindings.insert(
            (KeyCode::Char('n'), KeyModifiers::CONTROL),
            EditCommand::HistoryNext,
        );
        bindings.insert(
            (KeyCode::Char('r'), KeyModifiers::CONTROL),
            EditCommand::HistoryRFind,
        );
        bindings.insert(
            (KeyCode::Char('s'), KeyModifiers::CONTROL),
            EditCommand::HistoryFind,
        );
        bindings.insert(
            (KeyCode::Char('g'), KeyModifiers::CONTROL),
            EditCommand::Escape,
        );
        bindings.insert(
            (KeyCode::Char('b'), KeyModifiers::ALT),
            EditCommand::CursorSpanBack,
        );
        bindings.insert(
            (KeyCode::Char('f'), KeyModifiers::ALT),
            EditCommand::CursorSpanForward,
        );
        bindings.insert(
            (KeyCode::Delete, KeyModifiers::ALT),
            EditCommand::DeleteSpanBack,
        );
        bindings.insert(
            (KeyCode::Char('d'), KeyModifiers::ALT),
            EditCommand::DeleteSpanForward,
        );

        KeyMap { bindings }
    }
}
#[cfg(test)]
#[allow(clippy::unicode_not_nfc)]
mod tests {
    use super::*;
    use crate::history_stack::tests::HistoryStackBuilder;
    use crate::renderer::ViewState;
    use crate::renderer::tests::ViewBuilder;

    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use similar_asserts::assert_eq;

    #[test]
    fn can_read_line_from_bufread() {
        fn read(editor: &mut impl LineEdit, buf: &mut String) {
            editor.read_line(buf, None).unwrap();
        }
        
        let mut input = "foo\n".as_bytes();
        let mut buf = String::new();
        read(&mut input, &mut buf);
        assert_eq!(&buf, "foo\n");
    }
    #[test]
    fn unimplemented_event_ignored() {
        let mut editor = LineEditor::new();
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor.handle_event(&mut view, &Event::FocusLost).unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn unimplemented_key_event_ignored() {
        let mut editor = LineEditor::new();
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn enter_breaks_input_loop() {
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut ViewBuilder::new().build(),
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_break());
    }

    #[test]
    fn char_input_non_0w_inserts() {
        let mut editor = LineEditor::new();
        let expected_editor = LineEditor::from("🎸!");

        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('🎸'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('!'),
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();

        assert!(res.is_continue());

        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
        assert_eq!(editor.line_cursor, expected_editor.line.len());
    }

    #[test]
    fn char_input_0w_requires_base_char() {
        let mut editor = LineEditor::new();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('\u{0308}'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(editor.line.is_empty());
        assert_eq!(view, expected_view);

        let mut editor = LineEditor::from("a");
        let expected_editor = LineEditor::from("ä");

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('\u{0308}'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
    }

    #[test]
    fn backspace_0w() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("AëZ");
        editor.line_cursor = editor.line.len() - 1;
        let mut expected_editor = LineEditor::from("AeZ");
        expected_editor.line_cursor = 2;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn backspace_1w() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("AeZ");
        editor.line_cursor = editor.line.len() - 1;
        let mut expected_editor = LineEditor::from("AZ");
        expected_editor.line_cursor = 1;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn backspace_2w() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("a🎸z");
        editor.line_cursor = editor.line.len() - 1;
        let mut expected_editor = LineEditor::from("az");
        expected_editor.line_cursor = 1;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn backspace_at_input_start_does_nothing() {
        let mut editor = LineEditor::from("input text");
        editor.line_cursor = 0;
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn left_from_input_start_does_nothing() {
        let mut editor = LineEditor::from("12345");
        editor.line_cursor = 0;
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn left_moves_cursor_to_preceding_base_char() {
        let mut editor = LineEditor::from("aë🎸iou");
        editor.line_cursor = 8;
        let mut expected_editor = editor.clone();
        expected_editor.line_cursor = 4;

        let vb = ViewBuilder::new();

        let mut view = vb.build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        let mut view = vb.build();
        expected_editor.line_cursor = 1;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        let mut view = vb.build();
        expected_editor.line_cursor = 0;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
    }

    #[test]
    fn home_from_input_start_does_nothing() {
        let mut editor = LineEditor::from("input text");
        editor.line_cursor = 0;
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn home_moves_cursor_to_input_start() {
        let mut editor = LineEditor::from("input text");
        editor.line_cursor = 5;
        let mut expected_editor = editor.clone();
        expected_editor.line_cursor = 0;

        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
    }

    #[test]
    fn forward_at_buffer_end_does_nothing() {
        let mut editor = LineEditor::from("input text");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn right_moves_cursor_to_next_base_char_until_end() {
        let mut editor = LineEditor::from("aë🎸o");
        editor.line_cursor = 0;
        let mut expected_editor = editor.clone();

        let vb = ViewBuilder::new();

        let mut view = vb.build();
        expected_editor.line_cursor = 1;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        let mut view = vb.build();
        expected_editor.line_cursor = 4;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        let mut view = vb.build();
        expected_editor.line_cursor = 8;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        let mut view = vb.build();
        expected_editor.line_cursor = 9;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
    }

    #[test]
    fn end_at_buffer_end_does_nothing() {
        let mut editor = LineEditor::from("buffer text");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn end_moves_cursor_to_buffer_end() {
        let mut editor = LineEditor::from("buffer text");
        let expected_editor = editor.clone();
        editor.line_cursor = 3;

        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
        assert_eq!(editor.line_cursor, expected_editor.line.len());
    }

    #[test]
    fn delete_at_buffer_end_does_nothing() {
        let mut editor = LineEditor::from("aë🎸io");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_removes_chars_from_cursor_to_next_base_char() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("aë🎸io");
        editor.line_cursor = 1;
        let mut expected_editor = LineEditor::from("a🎸io");
        expected_editor.line_cursor = 1;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        expected_editor.line = "aio".to_owned();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor.line_cursor, 1);

        expected_editor.line = "ao".to_owned();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn delete_to_start_at_start_is_nop() {
        let mut editor = LineEditor::from("aë🎸io");
        editor.line_cursor = 0;
        let mut view = ViewBuilder::new().with_state(ViewState::Valid).build();
        let expected_editor = editor.clone();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Home,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_to_start_removes_chars_before_cursor() {
        let mut editor = LineEditor::from("aë🎸io");
        editor.line_cursor = 8;
        let mut expected_editor = LineEditor::from("io");
        expected_editor.line_cursor = 0;
        let mut view = ViewBuilder::new().with_state(ViewState::Valid).build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Home,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn delete_to_end_at_end_is_nop() {
        let mut editor = LineEditor::from("aë🎸io");
        let mut view = ViewBuilder::new().with_state(ViewState::Valid).build();
        let expected_editor = editor.clone();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_to_end_removes_chars_from_cursor_to_end() {
        let mut editor = LineEditor::from("aë🎸io");
        editor.line_cursor = 4;
        let mut view = ViewBuilder::new().with_state(ViewState::Valid).build();
        let expected_editor = LineEditor::from("aë");

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
        assert_eq!(editor.line_cursor, expected_editor.line.len());
    }

    #[test]
    fn up_nop_if_no_history() {
        let mut editor = LineEditor::from("abcdëf🎸");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue(),);
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn down_nop_if_no_history() {
        let mut editor = LineEditor::from("abcdëf🎸");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn esc_nop_if_no_history() {
        let mut editor = LineEditor::from("abcdëf🎸");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn down_nop_when_not_viewing_history() {
        let mut editor = LineEditor::from("abcdëf🎸");
        let expected_editor = editor.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn enter_adds_non_empty_input_to_history() {
        let mut view = ViewBuilder::new().build();

        let expected_hs =
            HistoryStackBuilder::new().with_entries(&["123456789abc"]).build();
        let mut editor = LineEditor::from("123456789abc");
        editor.history = Some(HistoryStack::new());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_break());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn up_editing_input_saves_input_and_views_most_recent_history() {
        let mut view = ViewBuilder::new().build();
        let mut editor = LineEditor::from("123456789abc");
        let mut expected_editor = LineEditor::from("baz");

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_draft(Some("123456789abc"));
        editor.history =
            Some(hs_builder.with_entries(&["foo", "bar", "baz"]).build());
        expected_editor.history = Some(hs_builder.with_index(Some(2)).build());

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn accepting_history_item_resets_history_stack() {
        let mut editor = LineEditor::from("ba");
        let mut expected_editor = LineEditor::from("foo");

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let hs = hs_builder.with_index(Some(1)).build();
        let expected_hs = hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_index(Some(0))
            .build();

        editor.history = Some(hs);
        expected_editor.history = Some(expected_hs);
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());

        expected_editor.history = Some(
            hs_builder
                .with_entries(&["foo", "bar", "baz", "foo"])
                .with_index(Some(4))
                .with_draft(None)
                .build(),
        );
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_break());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn up_viewing_history_views_next_oldest_history() {
        let mut editor = LineEditor::from("baz");
        let mut expected_editor = LineEditor::from("bar");

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(1)).build();
        let hs = hs_builder.with_index(Some(2)).build();

        editor.history = Some(hs);
        expected_editor.history = Some(expected_hs);
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn up_viewing_history_nop_after_oldest_history() {
        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(0)).build();
        let hs = hs_builder.build();

        let mut editor = LineEditor::from("foo");
        editor.history = Some(hs);
        let mut expected_editor = editor.clone();
        expected_editor.history = Some(expected_hs);
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn down_viewing_history_views_next_newest_history() {
        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(1)).build();
        let hs = hs_builder.with_index(Some(0)).build();

        let mut editor = LineEditor::from("foo");
        editor.history = Some(hs);
        let mut expected_editor = LineEditor::from("bar");
        expected_editor.history = Some(expected_hs);
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert!(!view.is_valid());
    }

    #[test]
    fn down_from_newest_history_returns_to_editing_draft() {
        let draft = "123456789abc";

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_entries(&["foo", "bar", "baz"]);
        let expected_hs = hs_builder.with_draft(Some(draft)).build();
        let hs = hs_builder.with_index(Some(2)).build();

        let mut editor = LineEditor::from("baz");
        editor.history = Some(hs);
        let mut expected_editor = LineEditor::from(draft);
        expected_editor.history = Some(expected_hs);
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn esc_editing_history_edits_draft() {
        let mut hs_builder = HistoryStackBuilder::new();
        let expected_hs =
            hs_builder.with_entries(&["foo", "bar", "baz"]).build();
        let hs = hs_builder
            .with_draft(Some("123456789abc"))
            .with_index(Some(0))
            .build();

        let mut expected_editor = LineEditor::from("123456789abc");
        expected_editor.history = Some(expected_hs);
        let mut editor = LineEditor::from("fo");
        editor.history = Some(hs);

        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn esc_nop_when_editing_input() {
        let mut editor = LineEditor::from("some text");
        let mut view = ViewBuilder::new().build();

        let expected_editor = editor.clone();
        let expected_view = view.clone();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn esc_viewing_history_after_editing_input_edits_input() {
        let mut editor = LineEditor::from("foo");
        let mut view = ViewBuilder::new().build();
        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_entries(&["foo", "bar", "baz"]);
        editor.history = Some(
            hs_builder
                .with_draft(Some("123456789abc"))
                .with_index(Some(0))
                .build(),
        );

        let mut expected_editor = LineEditor::from("123456789abc");
        expected_editor.history =
            Some(hs_builder.with_index(Some(3)).with_draft(None).build());

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn rfind_shows_match() {
        let mut editor = LineEditor::from("ol");
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(
                editor.line.len().try_into().unwrap(),
                23,
            ))
            .with_first_display_line(23)
            .build();

        editor.history = Some(
            HistoryStackBuilder::new()
                .with_entries(&["oldest", "older", "old", "newest"])
                .build(),
        );
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor.line, "old");
        assert_eq!(editor.line_cursor, 2);
    }

    #[test]
    fn rfind_uses_new_prefix() {
        let mut editor = LineEditor::from("ol");
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(
                editor.line.len().try_into().unwrap(),
                23,
            ))
            .with_first_display_line(23)
            .build();

        editor.history = Some(
            HistoryStackBuilder::new()
                .with_entries(&["oldest", "older", "old", "newest"])
                .build(),
        );
        let _ = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            )
            .unwrap();
        editor.line.replace_range(.., "ne");
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('r'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());

        assert_eq!(editor.line, "newest");
        assert_eq!(editor.line_cursor, 2);
        assert_eq!(editor.line_cursor, 2);
    }

    #[test]
    fn find_shows_match() {
        let mut editor = LineEditor::from("ol");
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(
                editor.line.len().try_into().unwrap(),
                23,
            ))
            .with_first_display_line(23)
            .build();

        editor.history = Some(
            HistoryStackBuilder::new()
                .with_entries(&["oldest", "older", "old", "newest"])
                .build(),
        );
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor.line, "oldest");
        assert_eq!(editor.line_cursor, 2);
    }

    #[test]
    fn find_uses_new_prefix() {
        let mut editor = LineEditor::from("ol");
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(
                editor.line.len().try_into().unwrap(),
                23,
            ))
            .with_first_display_line(23)
            .build();

        editor.history = Some(
            HistoryStackBuilder::new()
                .with_entries(&["oldest", "older", "old", "newest"])
                .build(),
        );
        let _ = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)),
            )
            .unwrap();
        editor.line.replace_range(.., "ne");
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('s'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "newest");
        assert_eq!(editor.line_cursor, 2);
    }

    #[test]
    fn ctrl_i_inputs_tab() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("text");
        editor.line_cursor = 1;
        let mut expected_editor = LineEditor::from("t\text");
        expected_editor.line_cursor = 2;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('i'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn tab_indents_with_tab() {
        let mut editor = LineEditor::from("\tline");
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        let expected_editor = LineEditor::from("\t\tline");
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn tab_indents_with_spaces() {
        let mut editor = LineEditor::from("line");
        editor.line_cursor = 2;
        let mut expected_editor = LineEditor::from("    line");
        expected_editor.line_cursor = 6;
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        let mut editor = LineEditor::from("     line");
        editor.line_cursor = 6;
        expected_editor.line = "        line".to_owned();
        expected_editor.line_cursor = 9;
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn tab_indents_correctly_with_mixed_leading_blanks() {
        let mut editor = LineEditor::from("     \tline");
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        let expected_editor = LineEditor::from("        \tline");
        assert_eq!(editor, expected_editor);

        let mut editor = LineEditor::from("\t\t  line");
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        let expected_editor = LineEditor::from("\t\t\t  line");
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn backtab_dedents_with_tab() {
        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::from("\t\tline");
        editor.line_cursor = 5;
        let mut expected_editor = LineEditor::from("\tline");
        expected_editor.line_cursor = 4;
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn backtab_dedents_with_spaces() {
        let mut editor = LineEditor::from("        line");
        editor.line_cursor = 10;
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&editor.line, "    line");
        assert_eq!(editor.line_cursor, 6);
    }

    #[test]
    fn backtab_nop_with_no_indent() {
        let mut editor = LineEditor::from("line");
        editor.line_cursor = 2;
        let mut view = ViewBuilder::new().build();
        let expected_editor = editor.clone();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn cursor_span_forward_jumps_to_next_word() {
        let mut editor = LineEditor::from("word \t  (())");
        editor.line_cursor = 2;
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(8, editor.line_cursor);
        assert!(!view.is_valid());

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(12, editor.line_cursor);
    }

    #[test]
    fn cursor_span_forward_nop_at_end() {
        let mut editor = LineEditor::from("chars");
        let expected_editor = editor.clone();
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(view, expected_view);
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn cursor_span_forward_nop_on_empty_buffer() {
        let mut editor = LineEditor::new();
        let expected_editor = editor.clone();
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(view, expected_view);
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn cursor_span_back_jumps_to_start_of_previous_word() {
        let mut editor = LineEditor::from("    word \t  (())");
        editor.line_cursor = editor.line.len() - 2;
        let mut view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(12, editor.line_cursor);
        assert!(!view.is_valid());

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(4, editor.line_cursor);

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(0, editor.line_cursor);
    }

    #[test]
    fn cursor_span_back_nop_at_start() {
        let mut editor = LineEditor::from("chars");
        editor.line_cursor = 0;
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(view, expected_view);
    }

    #[test]
    fn cursor_span_back_nop_on_empty_buffer() {
        let mut editor = LineEditor::new();
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_span_forward_nop_at_end() {
        let mut editor = LineEditor::from("    word    \t  (())");
        let expected_editor = editor.clone();
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_span_forward_deletes_to_next_span_end() {
        let mut editor = LineEditor::from("    word    \t  (())");
        editor.line_cursor = 2;
        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "  word    \t  (())");
        assert_eq!(editor.line_cursor, 2);

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "  (())");
        assert_eq!(editor.line_cursor, 2);

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "  ");
        assert_eq!(editor.line_cursor, 2);
    }

    #[test]
    fn delete_span_back_deletes_to_previous_span_start() {
        let mut editor = LineEditor::from("    word    \t  (())");
        editor.line_cursor = 17;
        let mut view = ViewBuilder::new().build();

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "    word    \t  ))");
        assert_eq!(editor.line_cursor, 15);

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "    ))");
        assert_eq!(editor.line_cursor, 4);

        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&editor.line, "))");
        assert_eq!(editor.line_cursor, 0);
    }

    #[test]
    fn delete_span_back_at_start_is_nop() {
        let mut editor = LineEditor::from("    word    \t  (())");
        editor.line_cursor = 0;
        let expected_editor = editor.clone();
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn do_unicode_input_mode_is_nop_if_repeated() {
        let mut editor = LineEditor::from("foo");
        let mut view = ViewBuilder::new().build();
        let _ = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('u'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();

        let mut view = ViewBuilder::new().build();
        editor.unicode = "42".to_owned();
        editor.unicode_cursor = Some(2);
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('u'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor.unicode_cursor, Some(2));
        assert!(view.is_valid());
    }

    #[test]
    fn do_unicode_input_mode_entry() {
        let mut editor = LineEditor::from("foo");
        let mut view = ViewBuilder::new().build();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('u'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor.unicode_cursor, Some(0));
        assert!(!view.is_valid());
    }

    #[test]
    fn do_escape_exits_unicode_input_mode() {
        let mut editor = LineEditor::from("foo");
        let mut view = ViewBuilder::new().build();

        let _ = editor.do_unicode_input_mode(&mut view);
        view = ViewBuilder::new().build();
        assert!(editor.unicode_cursor.is_some());

        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(editor.unicode.is_empty());
        assert!(editor.unicode_cursor.is_none());
        assert!(!view.is_valid());
    }

    #[test]
    fn do_accept_input_inserts_unicode_char() {
        let mut editor = LineEditor::from("ae");
        let mut view = ViewBuilder::new().build();

        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);

        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&editor.line, "aë");
        assert_eq!(editor.line_cursor, 4);
        assert!(editor.unicode.is_empty());
        assert!(editor.unicode_cursor.is_none());
        assert!(!view.is_valid());
    }

    #[test]
    fn history_cmds_nop_in_unicode_input() {
        let mut editor = LineEditor::from("foo");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);

        let hs = HistoryStackBuilder::new()
            .with_entries(&["foo", "bar", "baz"])
            .build();
        editor.history = Some(hs);

        let mut view = ViewBuilder::new().build();

        let expected_editor = editor.clone();
        let expected_view = view.clone();

        // NextBack
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);

        // Rfind
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);

        // Find
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);

        // Next
        let hs = HistoryStackBuilder::new()
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("frotz"))
            .with_index(Some(1))
            .build();
        editor.history = Some(hs);
        let expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn word_based_cmds_nop_in_unicode_input() {
        let mut editor = LineEditor::from("line input");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();
        assert!(view.is_valid());

        // Next Word
        editor.line_cursor = 0;
        let expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);

        // Prev Word
        editor.line_cursor = editor.line.len();
        let expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);

        // Delete Next Word
        editor.line_cursor = 0;
        let expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);

        // Delete Prev Word
        editor.line_cursor = editor.line.len();
        let expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(editor, expected_editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn navigate_begin_end_during_unicode_input() {
        let mut editor = LineEditor::from("line input");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        let mut view = ViewBuilder::new().build();

        // Cursor to Start
        let mut expected_editor = editor.clone();
        expected_editor.unicode_cursor = Some(0);
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        // Cursor to End
        expected_editor.unicode_cursor = Some(4);
        view = ViewBuilder::new().build();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn delete_begin_end_during_unicode_input() {
        let mut editor = LineEditor::from("line input");
        let mut view = ViewBuilder::new().build();

        // Delete to start
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(2);
        let mut expected_editor = editor.clone();
        expected_editor.unicode = "08".to_owned();
        expected_editor.unicode_cursor = Some(0);
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Home,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        // Delete to end
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(2);
        let mut expected_editor = editor.clone();
        expected_editor.unicode = "03".to_owned();
        view = ViewBuilder::new().build();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn navigate_next_and_back_during_unicode_input() {
        let mut editor = LineEditor::from("line input");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        let mut view = ViewBuilder::new().build();

        // Back
        assert!(view.is_valid());
        let mut expected_editor = editor.clone();
        expected_editor.unicode_cursor = Some(3);
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        // Next
        expected_editor.unicode_cursor = Some(1);
        editor.unicode_cursor = Some(0);
        view = ViewBuilder::new().build();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn delete_char_next_and_back_during_unicode_input() {
        let mut editor = LineEditor::from("line input");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        let mut view = ViewBuilder::new().build();

        // Backspace
        let mut expected_editor = editor.clone();
        expected_editor.unicode = "030".to_owned();
        expected_editor.unicode_cursor = Some(3);
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);

        // Delete
        editor.unicode_cursor = Some(0);
        expected_editor = editor.clone();
        expected_editor.unicode = "30".to_owned();
        view = ViewBuilder::new().build();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn indent_dedent_nop_during_unicode_input() {
        let mut editor = LineEditor::from("    line input");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        let mut view = ViewBuilder::new().build();
        let expected_editor = editor.clone();

        // Indent
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(editor, expected_editor);

        // Dedent
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(editor, expected_editor);
    }

    #[test]
    fn unicode_input_limited_to_six_hex_digits() {
        let mut editor = LineEditor::from("ae");
        editor.unicode.clear();
        editor.unicode_cursor = Some(0);
        let mut view = ViewBuilder::new().build();

        // non-hex digit chars ignored
        let mut expected_editor = editor.clone();
        assert!(view.is_valid());
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('z'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(editor, expected_editor);

        // up to six hex digits accepted
        for ch in "01F3b8".chars() {
            expected_editor.unicode.push(ch);
            *expected_editor.unicode_cursor.get_or_insert(0) += 1;
            view = ViewBuilder::new().build();
            let res = editor
                .handle_event(
                    &mut view,
                    &Event::Key(KeyEvent::new(
                        KeyCode::Char(ch),
                        KeyModifiers::NONE,
                    )),
                )
                .unwrap();
            assert!(res.is_continue());
            assert!(!view.is_valid());
            assert_eq!(editor, expected_editor);
        }

        // hex digits beyond 6 ignored
        view = ViewBuilder::new().build();
        let res = editor
            .handle_event(
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('a'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(editor, expected_editor);
    }
}
