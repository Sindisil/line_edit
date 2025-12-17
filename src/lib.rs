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
use std::io::{self, Write};
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
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LineEditor {
    history: Option<HistoryStack>,
    use_history: bool,
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

impl LineEditor {
    /// Creates a new `LineEditor`.
    ///
    /// The new instance will not allocate space to store
    /// history until a read is done with history enabled.
    #[must_use]
    pub fn new() -> LineEditor {
        LineEditor { ..Default::default() }
    }

    #[cfg(test)]
    fn with_history(history: Option<HistoryStack>) -> LineEditor {
        LineEditor { history, use_history: true, ..Default::default() }
    }

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

        self.use_history = options.is_some_and(|o| o.history);
        if self.use_history && self.history.is_none() {
            self.history = Some(HistoryStack::new());
        }

        let mut input_buffer = String::with_capacity(80);

        if let Some(prefill) = options.and_then(|o| o.prefill.as_ref()) {
            input_buffer.push_str(prefill);
            view.set_insertion_point(input_buffer.len());
        }

        view.repaint(&input_buffer)?;
        while self.pump_event(&mut input_buffer, &mut view)?.is_continue() {
            view.repaint(&input_buffer)?;
        }

        let _ = do_cursor_to_end(&input_buffer, &mut view);
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\r\n")?;
        stdout.flush()?;

        let prev_bytes = output_buffer.len();
        output_buffer.push_str(&input_buffer);
        output_buffer.push_str(native_eol());
        Ok(output_buffer.len() - prev_bytes)
    }

    fn pump_event(
        &mut self,
        buffer: &mut String,
        view: &mut View,
    ) -> io::Result<ControlFlow<()>> {
        let event = event::read()?;
        self.handle_event(buffer, view, &event)
    }

    fn handle_event(
        &mut self,
        buffer: &mut String,
        view: &mut View,
        event: &Event,
    ) -> io::Result<ControlFlow<()>> {
        match event {
            Event::Key(event) if event.is_press() => Ok(self
                .handle_key_pressed(
                    (event.code, event.modifiers),
                    buffer,
                    view,
                )),
            &Event::Resize(mut w, mut h) => {
                while let Ok(true) = event::poll(Duration::from_millis(50)) {
                    if let Event::Resize(w1, h1) = event::read()? {
                        (w, h) = (w1, h1);
                    }
                }
                let cursor_position: Coord2D = cursor::position()?.into();
                view.resize(DimWH(w, h), cursor_position, buffer);
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
        buffer: &mut String,
        view: &mut View,
    ) -> ControlFlow<()> {
        let Some(command) = self.key_bindings.get(key) else {
            return ControlFlow::Continue(());
        };

        let history =
            if self.use_history { self.history.as_mut() } else { None };
        match command {
            EditCommand::CharInput(ch) => do_char_input(buffer, view, ch),
            EditCommand::Backspace => do_backspace(buffer, view),
            EditCommand::Delete => do_delete(buffer, view),
            EditCommand::HistoryNextBack => {
                do_history_next_back(buffer, view, history)
            }
            EditCommand::HistoryNext => do_history_next(buffer, view, history),
            EditCommand::RestoreDraft => {
                do_restore_draft(buffer, view, history)
            }
            EditCommand::CursorBack => do_cursor_back(buffer, view),
            EditCommand::CursorForward => do_cursor_forward(buffer, view),
            EditCommand::CursorToStart => do_cursor_to_start(view),
            EditCommand::CursorToEnd => do_cursor_to_end(buffer, view),
            EditCommand::DeleteToStart => do_delete_to_start(buffer, view),
            EditCommand::DeleteToEnd => do_delete_to_end(buffer, view),
            EditCommand::AcceptLine => do_accept_line(buffer, history),
            EditCommand::HistoryRFind => {
                do_history_rfind(buffer, view, history)
            }
            EditCommand::HistoryFind => do_history_find(buffer, view, history),
            EditCommand::Indent => do_indent(buffer, view),
            EditCommand::Dedent => do_dedent(buffer, view),
            EditCommand::CursorSpanBack => do_cursor_span_back(buffer, view),
            EditCommand::CursorSpanForward => {
                do_cursor_span_forward(buffer, view)
            }
            EditCommand::DeleteSpanBack => do_delete_span_back(buffer, view),
            EditCommand::DeleteSpanForward => {
                do_delete_span_forward(buffer, view)
            }
        }
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

fn do_accept_line(
    buffer: &str,
    mut history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(ref mut history) = history {
        history.rewind();
        if !buffer.is_empty()
            && history.last().is_none_or(|last| last != buffer)
        {
            history.push(buffer.to_string());
        }
    }
    ControlFlow::Break(())
}

fn do_restore_draft(
    buffer: &mut String,
    view: &mut View,
    history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(draft) = history.and_then(HistoryStack::rewind) {
        buffer.replace_range(.., &draft);
        view.set_insertion_point(buffer.len());
    }
    ControlFlow::Continue(())
}

fn do_history_next(
    buffer: &mut String,
    view: &mut View,
    history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(history_line) = history.and_then(|h| h.next_newer()) {
        buffer.replace_range(.., history_line);
        view.set_insertion_point(buffer.len());
    }

    ControlFlow::Continue(())
}

fn do_history_next_back(
    buffer: &mut String,
    view: &mut View,
    history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(line) = history.and_then(|h| h.next_older(buffer)) {
        buffer.replace_range(.., line);
        view.set_insertion_point(buffer.len());
    }

    ControlFlow::Continue(())
}

fn do_char_input(
    buffer: &mut String,
    view: &mut View,
    c: char,
) -> ControlFlow<()> {
    // if char is zero width, but no previous chars exist to
    //  which it can  be combined, do nothing (i.e., don't accept
    // the input)
    if c != '\t'
        && c.width().unwrap_or(0) == 0
        && !buffer[..view.insertion_point()]
            .chars()
            .rev()
            .take_while(|c| *c != '\t')
            .any(|c| c.width().unwrap_or(0) > 0)
    {
        return ControlFlow::Continue(());
    }

    buffer.insert(view.insertion_point(), c);
    view.set_insertion_point(view.insertion_point() + c.len_utf8());

    ControlFlow::Continue(())
}

fn do_backspace(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    if view.insertion_point() != 0
        && let Some((i, _)) =
            buffer[..view.insertion_point()].char_indices().next_back()
    {
        buffer.remove(i);
        view.set_insertion_point(i);
    }

    ControlFlow::Continue(())
}

fn do_cursor_back(buffer: &str, view: &mut View) -> ControlFlow<()> {
    if view.insertion_point() != 0
        && let Some((prev_idx, _)) = buffer[..view.insertion_point()]
            .char_indices()
            .rfind(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
    {
        view.set_insertion_point(prev_idx);
    }

    ControlFlow::Continue(())
}

fn do_cursor_forward(buffer: &str, view: &mut View) -> ControlFlow<()> {
    // If aleady at end, nothing to do
    if view.insertion_point() != buffer.len() {
        let next_idx = buffer[view.insertion_point()..]
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
            .map_or_else(|| buffer.len(), |(i, _)| i + view.insertion_point());
        view.set_insertion_point(next_idx);
    }

    ControlFlow::Continue(())
}

fn do_delete(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    // if at end of buffer, nothing to do
    let cur_idx = view.insertion_point();
    if cur_idx != buffer.len() {
        let next_idx = buffer[view.insertion_point()..]
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '\t' || c.width().unwrap_or(0) > 0)
            .map_or_else(|| buffer.len(), |(i, _)| i + view.insertion_point());
        buffer.replace_range(view.insertion_point()..next_idx, "");
        view.invalidate();
    }

    ControlFlow::Continue(())
}

fn do_delete_to_start(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    // if at start of buffer, nothing to do
    if view.insertion_point() != 0 {
        buffer.replace_range(..view.insertion_point(), "");
        view.set_insertion_point(0);
    }

    ControlFlow::Continue(())
}

fn do_delete_to_end(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    // if at end of buffer, nothing to do
    if view.insertion_point() != buffer.len() {
        buffer.replace_range(view.insertion_point().., "");
        view.set_insertion_point(buffer.len());
    }

    ControlFlow::Continue(())
}

fn do_cursor_to_start(view: &mut View) -> ControlFlow<()> {
    if view.insertion_point() != 0 {
        view.set_insertion_point(0);
    }

    ControlFlow::Continue(())
}

fn do_cursor_to_end(buffer: &str, view: &mut View) -> ControlFlow<()> {
    if view.insertion_point() != buffer.len() {
        view.set_insertion_point(buffer.len());
    }

    ControlFlow::Continue(())
}

fn do_history_find(
    buffer: &mut String,
    view: &mut View,
    history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(line) =
        history.and_then(|h| h.find(&buffer[..view.insertion_point()]))
    {
        buffer.replace_range(.., line);
        view.invalidate();
    }

    ControlFlow::Continue(())
}

fn do_history_rfind(
    buffer: &mut String,
    view: &mut View,
    history: Option<&mut HistoryStack>,
) -> ControlFlow<()> {
    if let Some(line) =
        history.and_then(|h| h.rfind(&buffer[..view.insertion_point()]))
    {
        buffer.replace_range(.., line);
        view.invalidate();
    }

    ControlFlow::Continue(())
}

fn do_indent(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    // If the first buffer char is tab ('\t'), insert one additional
    // tab at start of line. If not, insert up to 4 space (' ') chars
    // at start of line, so that leading spaces are the next multiple
    // of four.
    if buffer.starts_with('\t') {
        buffer.insert(0, '\t');
        view.set_insertion_point(view.insertion_point() + 1);
    } else {
        let leading_spaces = buffer.chars().take_while(|c| *c == ' ').count();
        let next_stop = (leading_spaces + 1).next_multiple_of(4);
        let to_add = next_stop - leading_spaces;
        buffer.insert_str(0, &"    "[..to_add]);
        view.set_insertion_point(view.insertion_point() + to_add);
    }
    ControlFlow::Continue(())
}

fn do_dedent(buffer: &mut String, view: &mut View) -> ControlFlow<()> {
    // If the first buffer char is tab ('\t'), delete it.
    // If not, delete up to 4 leading spaces so that the
    // number of remaining leading spaces is a multple of four.
    if buffer.starts_with('\t') {
        buffer.remove(1);
        view.set_insertion_point(view.insertion_point().saturating_sub(1));
    } else if buffer.starts_with(' ') {
        let leading_spaces = buffer.chars().take_while(|c| *c == ' ').count();
        let previous_stop = (leading_spaces / 4).saturating_sub(1) * 4;
        let to_remove = leading_spaces.saturating_sub(previous_stop);
        buffer.replace_range(..to_remove, "");
        view.set_insertion_point(
            view.insertion_point().saturating_sub(to_remove),
        );
    }
    ControlFlow::Continue(())
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

fn do_cursor_span_back(buffer: &str, view: &mut View) -> ControlFlow<()> {
    if view.insertion_point() == 0 {
        return ControlFlow::Continue(());
    }

    let mut gr_idxs = buffer[..view.insertion_point()]
        .grapheme_indices(true)
        .rev()
        .skip_while(|(_, gr)| span_type(gr) == SpanType::Space);
    if let Some((idx, target_span_type)) =
        gr_idxs.next().map(|(idx, gr)| (idx, span_type(gr)))
    {
        view.set_insertion_point(
            gr_idxs
                .take_while(|(_, gr)| span_type(gr) == target_span_type)
                .last()
                .map_or(idx, |(i, _)| i),
        );
    } else {
        view.set_insertion_point(0);
    }

    ControlFlow::Continue(())
}

fn do_cursor_span_forward(buffer: &str, view: &mut View) -> ControlFlow<()> {
    let mut gr_idxs = buffer
        .grapheme_indices(true)
        .skip_while(|(i, _)| *i < view.insertion_point());
    let mut current_span_type =
        gr_idxs.next().map_or(SpanType::Empty, |(_, gr)| span_type(gr));
    if current_span_type != SpanType::Empty {
        view.set_insertion_point(
            gr_idxs
                .find(|(_, gr)| match span_type(gr) {
                    SpanType::Space => {
                        current_span_type = SpanType::Space;
                        false
                    }
                    st => st != current_span_type,
                })
                .map_or(buffer.len(), |(i, _)| i),
        );
    }
    ControlFlow::Continue(())
}

fn do_delete_span_back(
    buffer: &mut String,
    view: &mut View,
) -> ControlFlow<()> {
    if view.insertion_point() == 0 {
        return ControlFlow::Continue(());
    }

    let mut gr_idxs = buffer[..view.insertion_point()]
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
    buffer.replace_range(span_start..view.insertion_point(), "");
    view.set_insertion_point(span_start);
    ControlFlow::Continue(())
}

fn do_delete_span_forward(
    buffer: &mut String,
    view: &mut View,
) -> ControlFlow<()> {
    let mut gr_idxs = buffer
        .grapheme_indices(true)
        .skip_while(|(i, _)| *i < view.insertion_point());
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
            .map_or(buffer.len(), |(i, _)| i);
        buffer.replace_range(view.insertion_point()..span_end, "");
        view.invalidate();
    }
    ControlFlow::Continue(())
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum EditCommand {
    CharInput(char),
    Backspace,
    Delete,
    HistoryNextBack,
    HistoryNext,
    RestoreDraft,
    CursorBack,
    CursorForward,
    CursorToStart,
    CursorToEnd,
    DeleteToStart,
    DeleteToEnd,
    AcceptLine,
    HistoryRFind,
    HistoryFind,
    Indent,
    Dedent,
    CursorSpanBack,
    CursorSpanForward,
    DeleteSpanBack,
    DeleteSpanForward,
}

#[derive(Debug, Clone, PartialEq)]
struct KeyMap {
    bindings: HashMap<(KeyCode, KeyModifiers), EditCommand>,
}

impl KeyMap {
    fn get(&self, key: (KeyCode, KeyModifiers)) -> Option<EditCommand> {
        let cmd = self.bindings.get(&key).cloned();

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
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // Common
        bindings.insert(
            (KeyCode::Enter, KeyModifiers::NONE),
            EditCommand::AcceptLine,
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
        bindings.insert(
            (KeyCode::Esc, KeyModifiers::NONE),
            EditCommand::RestoreDraft,
        );
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
            EditCommand::RestoreDraft,
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
    fn unimplemented_event_ignored() {
        let mut buf = String::new();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(&mut buf, &mut view, &Event::FocusLost)
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn unimplemented_key_event_ignored() {
        let mut buf = String::new();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn enter_breaks_input_loop() {
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut String::new(),
                &mut ViewBuilder::new().build(),
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_break());
    }

    #[test]
    fn char_input_non_0w_inserts() {
        let mut buf = String::new();
        let expected_buf = "🎸!";

        let mut view = ViewBuilder::new().with_insertion_point(0).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('🎸'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue(),);
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('!'),
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();

        assert!(res.is_continue(),);

        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
    }

    #[test]
    fn char_input_0w_requires_base_char() {
        let mut buf = String::with_capacity(80);

        let mut vb = ViewBuilder::new();
        let mut view = vb.build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('\u{0308}'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert!(buf.is_empty());
        assert_eq!(view, expected_view);

        buf.push('a');
        let expected_buf = "ä";

        let mut view = vb.with_insertion_point(buf.len()).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('\u{0308}'),
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
    }

    #[test]
    fn backspace_0w() {
        let mut buf = "AëZ".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len() - 1).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, "AeZ");
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn backspace_1w() {
        let mut buf = "AeZ".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len() - 1).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, "AZ");
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);
    }

    #[test]
    fn backspace_2w() {
        let mut buf = "a🎸z".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len() - 1).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, "az");
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);
    }

    #[test]
    fn backspace_at_input_start_does_nothing() {
        let mut buf = "input text".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                )),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn left_from_input_start_does_nothing() {
        let mut buf = "12345".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn left_moves_cursor_to_preceding_base_char() {
        let mut buf = "aë🎸iou".to_owned();
        let expected_buf = buf.clone();

        let mut vb = ViewBuilder::new();

        let mut view = vb.with_insertion_point(8).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 4);

        let mut view = vb.with_insertion_point(4).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);

        let mut view = vb.with_insertion_point(1).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 0);
    }

    #[test]
    fn home_from_input_start_does_nothing() {
        let mut buf = "input text".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn home_moves_cursor_to_input_start() {
        let mut buf = "input text".to_owned();
        let expected_buf = buf.clone();

        let mut vb = ViewBuilder::new();
        let mut view = vb.with_insertion_point(5).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 0);
    }

    #[test]
    fn right_at_buffer_end_does_nothing() {
        let mut buf = "input text".to_owned();
        let expected_buf = buf.clone();

        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn right_moves_cursor_to_next_base_char_until_end() {
        let mut buf = "aë🎸o".to_owned();
        let expected_buf = buf.clone();

        let mut vb = ViewBuilder::new();
        let mut view = vb.build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);

        let mut view = vb.with_insertion_point(1).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 4);

        let mut view = vb.with_insertion_point(4).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 8);

        let mut view = vb.with_insertion_point(8).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 9);
    }

    #[test]
    fn end_at_buffer_end_does_nothing() {
        let mut buf = "buffer text".to_owned();
        let expected_buf = buf.clone();

        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn end_moves_cursor_to_buffer_end() {
        let mut buf = "buffer text".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().with_insertion_point(3).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
    }

    #[test]
    fn delete_at_buffer_end_does_nothing() {
        let mut buf = "aë🎸io".to_owned();
        let expected_buf = buf.clone();

        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_removes_chars_from_cursor_to_next_base_char() {
        let mut buf = "aë🎸io".to_owned();

        let mut view = ViewBuilder::new().with_insertion_point(1).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "a🎸io");
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, "aio");

        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "ao");
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), 1);
    }

    #[test]
    fn delete_to_start_at_start_is_nop() {
        let mut buf = "aë🎸io".to_owned();
        let mut view = ViewBuilder::new()
            .with_insertion_point(0)
            .with_state(ViewState::Valid)
            .build();
        let expected_buf = buf.clone();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Home,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_to_start_removes_chars_before_cursor() {
        let mut buf = "aë🎸io".to_owned();
        let expected_buf = "io";
        let mut view = ViewBuilder::new()
            .with_insertion_point(8)
            .with_state(ViewState::Valid)
            .build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Home,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, expected_buf);
        assert_eq!(view.insertion_point(), 0);
    }

    #[test]
    fn delete_to_end_at_end_is_nop() {
        let mut buf = "aë🎸io".to_owned();
        let mut view = ViewBuilder::new()
            .with_insertion_point(buf.len())
            .with_state(ViewState::Valid)
            .build();
        let expected_buf = buf.clone();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_to_end_removes_chars_from_cursor_to_end() {
        let mut buf = "aë🎸io".to_owned();
        let mut view = ViewBuilder::new()
            .with_insertion_point(4)
            .with_state(ViewState::Valid)
            .build();
        let expected_buf = "aë".to_owned();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
    }

    #[test]
    fn up_nop_if_no_history() {
        let mut buf = "abcdëf🎸".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue(),);
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn down_nop_if_no_history() {
        let mut buf = "abcdëf🎸".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn esc_nop_if_no_history() {
        let mut buf = "abcdëf🎸".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn down_nop_when_not_viewing_history() {
        let mut buf = "abcdëf🎸".to_owned();
        let expected_buf = buf.clone();

        let mut view = ViewBuilder::new().build();
        let expected_view = view.clone();

        let hs = Some(HistoryStack::new());
        let expected_hs = hs.clone();

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
        assert_eq!(editor.history, expected_hs);
    }

    #[test]
    fn enter_adds_non_empty_input_to_history() {
        let mut buf = "123456789abc".to_owned();
        let mut view = ViewBuilder::new().build();

        let hs = Some(HistoryStack::new());
        let expected_hs =
            HistoryStackBuilder::new().with_entries(&["123456789abc"]).build();
        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_break());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn up_editing_input_saves_input_and_views_most_recent_history() {
        let mut buf = "123456789abc".to_owned();
        let expected_buf = "baz";

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_draft(Some("123456789abc"));
        let hs = Some(hs_builder.with_entries(&["foo", "bar", "baz"]).build());
        let expected_hs = hs_builder.with_index(Some(2)).build();

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert_eq!(editor.history.unwrap(), expected_hs);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
    }

    #[test]
    fn accepting_history_item_resets_history_stack() {
        let mut buf = "ba".to_owned();
        let expected_buf = "foo";

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let hs = Some(hs_builder.with_index(Some(1)).build());
        let expected_hs = hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_index(Some(0))
            .build();

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert_eq!(editor.history.as_ref(), Some(&expected_hs));

        let expected_hs = hs_builder
            .with_entries(&["foo", "bar", "baz", "foo"])
            .with_index(Some(4))
            .with_draft(None)
            .build();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_break());
        assert_eq!(&buf, expected_buf);
        assert_eq!(editor.history.as_ref(), Some(&expected_hs));
    }

    #[test]
    fn up_viewing_history_views_next_oldest_history() {
        let mut buf = "baz".to_owned();
        let expected_buf = "bar";

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(1)).build();
        let hs = Some(hs_builder.with_index(Some(2)).build());

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn up_viewing_history_nop_after_oldest_history() {
        let mut buf = "foo".to_owned();

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(0)).build();
        let hs = Some(hs_builder.build());

        let expected_buf = buf.clone();
        let expected_view = view.clone();

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn down_viewing_history_views_next_newest_history() {
        let mut buf = "foo".to_owned();
        let expected_buf = "bar";

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder
            .with_entries(&["foo", "bar", "baz"])
            .with_draft(Some("123456789abc"));
        let expected_hs = hs_builder.with_index(Some(1)).build();
        let hs = Some(hs_builder.with_index(Some(0)).build());

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn down_from_newest_history_returns_to_editing_draft() {
        let draft = "123456789abc";

        let mut buf = "baz".to_owned();
        let expected_buf = draft;

        let mut view = ViewBuilder::new().build();

        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_entries(&["foo", "bar", "baz"]);
        let expected_hs = hs_builder.with_draft(Some(draft)).build();
        let hs = Some(hs_builder.with_index(Some(2)).build());

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn esc_editing_history_edits_draft() {
        let mut hs_builder = HistoryStackBuilder::new();
        let expected_hs =
            hs_builder.with_entries(&["foo", "bar", "baz"]).build();
        let hs = Some(
            hs_builder
                .with_draft(Some("123456789abc"))
                .with_index(Some(0))
                .build(),
        );

        let expected_buf = "123456789abc";
        let mut buf = "fo".to_owned();

        let mut view = ViewBuilder::new().build();

        let mut editor = LineEditor::with_history(hs);
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert!(!view.is_valid());
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn esc_nop_when_editing_input() {
        let mut buf = "some text".to_owned();
        let mut view = ViewBuilder::new().build();

        let expected_buf = buf.clone();
        let expected_view = view.clone();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn esc_viewing_history_after_editing_input_edits_input() {
        let mut buf = "foo".to_owned();
        let mut view = ViewBuilder::new().build();
        let mut hs_builder = HistoryStackBuilder::new();
        hs_builder.with_entries(&["foo", "bar", "baz"]);
        let hs = hs_builder
            .with_draft(Some("123456789abc"))
            .with_index(Some(0))
            .build();
        let mut editor = LineEditor::with_history(Some(hs));

        let expected_buf = "123456789abc";
        let expected_hs =
            hs_builder.with_index(Some(3)).with_draft(None).build();

        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            )
            .unwrap();

        assert!(res.is_continue());
        assert_eq!(&buf, expected_buf);
        assert_eq!(view.insertion_point(), expected_buf.len());
        assert!(!view.is_valid());
        assert_eq!(editor.history.unwrap(), expected_hs);
    }

    #[test]
    fn rfind_shows_match() {
        let mut buf = "ol".to_owned();
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_insertion_point(buf.len())
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(buf.len().try_into().unwrap(), 23))
            .with_first_display_line(23)
            .build();

        let hs = HistoryStackBuilder::new()
            .with_entries(&["oldest", "older", "old", "newest"])
            .build();
        let mut editor = LineEditor::with_history(Some(hs));
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "old");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn rfind_uses_new_prefix() {
        let mut buf = "ol".to_owned();
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_insertion_point(buf.len())
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(buf.len().try_into().unwrap(), 23))
            .with_first_display_line(23)
            .build();

        let hs = HistoryStackBuilder::new()
            .with_entries(&["oldest", "older", "old", "newest"])
            .build();
        let mut editor = LineEditor::with_history(Some(hs));
        let _ = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            )
            .unwrap();
        buf.replace_range(.., "ne");
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('r'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "newest");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn find_shows_match() {
        let mut buf = "ol".to_owned();
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_insertion_point(buf.len())
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(buf.len().try_into().unwrap(), 23))
            .with_first_display_line(23)
            .build();

        let hs = HistoryStackBuilder::new()
            .with_entries(&["oldest", "older", "old", "newest"])
            .build();
        let mut editor = LineEditor::with_history(Some(hs));
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "oldest");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn find_uses_new_prefix() {
        let mut buf = "ol".to_owned();
        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_insertion_point(buf.len())
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(buf.len().try_into().unwrap(), 23))
            .with_first_display_line(23)
            .build();

        let hs = HistoryStackBuilder::new()
            .with_entries(&["oldest", "older", "old", "newest"])
            .build();
        let mut editor = LineEditor::with_history(Some(hs));
        let _ = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::F(8), KeyModifiers::SHIFT)),
            )
            .unwrap();
        buf.replace_range(.., "ne");
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('s'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "newest");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn ctrl_i_inputs_tab() {
        let mut buf = "text".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(1).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Char('i'),
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "t\text");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn tab_indents_with_tab() {
        let mut buf = "\tline".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "\t\tline");
        assert_eq!(view.insertion_point(), buf.len());
    }

    #[test]
    fn tab_indents_with_spaces() {
        let mut buf = "line".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(2).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "    line");
        assert_eq!(view.insertion_point(), 6);
        let mut buf = "     line".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(6).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "        line");
        assert_eq!(view.insertion_point(), 9);
    }

    #[test]
    fn tab_indents_correctly_with_mixed_leading_blanks() {
        let mut buf = "     \tline".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "        \tline");
        assert_eq!(view.insertion_point(), buf.len());

        let mut buf = "\t\t  line".to_owned();
        view.set_insertion_point(buf.len());
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "\t\t\t  line");
        assert_eq!(view.insertion_point(), buf.len());
    }

    #[test]
    fn backtab_dedents_with_tab() {
        let mut buf = "\t\tline".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(5).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buf, "\tline");
        assert_eq!(view.insertion_point(), 4);
    }

    #[test]
    fn backtab_dedents_with_spaces() {
        let mut buf = "        line".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(10).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(!view.is_valid());
        assert_eq!(&buf, "    line");
        assert_eq!(view.insertion_point(), 6);
    }

    #[test]
    fn backtab_nop_with_no_indent() {
        let mut buf = "line".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(2).build();
        let expected_buf = buf.clone();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::BackTab,
                    KeyModifiers::SHIFT,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert!(view.is_valid());
        assert_eq!(buf, expected_buf);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn cursor_span_forward_jumps_to_next_word() {
        let mut buf = "word \t  (())".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(2).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(8, view.insertion_point());
        assert!(!view.is_valid());

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(12, view.insertion_point());
    }

    #[test]
    fn cursor_span_forward_nop_at_end() {
        let mut buf = "chars".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
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
    }

    #[test]
    fn cursor_span_forward_nop_on_empty_buffer() {
        let mut buf = String::new();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len()).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
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
    }

    #[test]
    fn cursor_span_back_jumps_to_start_of_previous_word() {
        let mut buf = "    word \t  (())".to_owned();
        let mut view =
            ViewBuilder::new().with_insertion_point(buf.len() - 2).build();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(12, view.insertion_point());
        assert!(!view.is_valid());

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(4, view.insertion_point());

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(0, view.insertion_point());
    }

    #[test]
    fn cursor_span_back_nop_at_start() {
        let mut buf = "chars".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
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
        let mut buf = String::new();
        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buf,
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
        let mut buffer = "    word    \t  (())".to_owned();
        let expected_buffer = buffer.clone();
        let mut view =
            ViewBuilder::new().with_insertion_point(buffer.len()).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buffer, expected_buffer);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn delete_span_forward_deletes_to_next_span_end() {
        let mut buffer = "    word    \t  (())".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(2).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "  word    \t  (())");
        assert_eq!(view.insertion_point(), 2);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "  (())");
        assert_eq!(view.insertion_point(), 2);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Delete,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "  ");
        assert_eq!(view.insertion_point(), 2);
    }

    #[test]
    fn delete_span_back_deletes_to_previous_span_start() {
        let mut buffer = "    word    \t  (())".to_owned();
        let mut view = ViewBuilder::new().with_insertion_point(17).build();

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "    word    \t  ))");
        assert_eq!(view.insertion_point(), 15);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "    ))");
        assert_eq!(view.insertion_point(), 4);

        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(&buffer, "))");
        assert_eq!(view.insertion_point(), 0);
    }

    #[test]
    fn delete_span_back_at_start_is_nop() {
        let mut buffer = "    word    \t  (())".to_owned();
        let expected_buffer = buffer.clone();
        let mut view = ViewBuilder::new().with_insertion_point(0).build();
        let expected_view = view.clone();
        let mut editor = LineEditor::new();
        let res = editor
            .handle_event(
                &mut buffer,
                &mut view,
                &Event::Key(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::CONTROL,
                )),
            )
            .unwrap();
        assert!(res.is_continue());
        assert_eq!(buffer, expected_buffer);
        assert_eq!(view, expected_view);
    }
}
