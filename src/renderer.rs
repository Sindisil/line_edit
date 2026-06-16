use std::cmp;
use std::io;
use std::io::Write;
use std::ops::Range;

use crossterm::ExecutableCommand;
use crossterm::QueueableCommand;
use crossterm::cursor::Hide;
use crossterm::cursor::MoveTo;
use crossterm::cursor::Show;
use crossterm::style::Attribute;
use crossterm::terminal;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use crossterm::terminal::ScrollUp;

use crate::LineEditor;

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    size: DimWH,
    first_display_line: u16,
    cursor_position: Coord2D,
    visible_chars: Range<usize>,
    prompt: Option<char>,
    unicode_input_position: Option<Coord2D>,
    state: ViewState,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ViewState {
    Valid,
    Invalid,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Coord2D(pub u16, pub u16);

impl From<(u16, u16)> for Coord2D {
    fn from(v: (u16, u16)) -> Self {
        Coord2D(v.0, v.1)
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq)]
pub struct DimWH(pub u16, pub u16);

impl From<(u16, u16)> for DimWH {
    fn from(v: (u16, u16)) -> Self {
        DimWH(v.0, v.1)
    }
}

#[must_use]
fn char_width(ch: char, width_before: u16) -> u16 {
    if ch == '\t' {
        8 - (width_before % 8)
    } else {
        use unicode_width::UnicodeWidthChar;
        ch.width().unwrap_or(0).try_into().expect("width is at most 2 columns")
    }
}

#[must_use]
fn str_width(s: &str, width_before: u16) -> u16 {
    s.chars().fold(0, |width, ch| width + char_width(ch, width + width_before))
}

impl View {
    pub fn new(
        size: DimWH,
        first_display_line: u16,
        prompt: Option<char>,
    ) -> View {
        assert!(size.0 >= 9, "Min 9 col. display width");
        View {
            size,
            first_display_line,
            cursor_position: Coord2D(0, first_display_line),
            visible_chars: 0..0,
            prompt,
            unicode_input_position: None,
            state: ViewState::Invalid,
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub fn size(&self) -> DimWH {
        self.size
    }

    #[cfg(not(tarpaulin_include))]
    pub fn invalidate(&mut self) {
        self.state = ViewState::Invalid;
    }

    pub fn is_valid(&self) -> bool {
        self.state == ViewState::Valid
    }

    pub fn resize(
        &mut self,
        size: DimWH,
        cursor_position: Coord2D,
        editor: &LineEditor,
    ) {
        // If nothing has changed, nothing to do
        if self.size == size && self.cursor_position == cursor_position {
            return;
        }

        assert!(size.0 >= 9, "Min 9 col. display width");

        self.size = size;
        self.cursor_position = cursor_position;

        let buf_lines = self.wrap_lines(editor);

        let line_cursor_buf_line = buf_lines
            .iter()
            .position(|l| l.contains(&editor.line_cursor))
            .expect("line_cursor is in or just after buffer");

        let first_visible_buf_line = line_cursor_buf_line
            .saturating_sub(usize::from(self.cursor_position.1));

        let last_visible_buf_line = first_visible_buf_line
            + cmp::min(usize::from(self.size.1), buf_lines.len())
            - 1;

        self.first_display_line = u16::try_from(
            usize::from(self.cursor_position.1)
                .saturating_sub(line_cursor_buf_line),
        )
        .expect("first_display_line fits u16");
        self.visible_chars.start = buf_lines[first_visible_buf_line].start;
        self.visible_chars.end = buf_lines[last_visible_buf_line].end
            - usize::from(
                last_visible_buf_line == line_cursor_buf_line
                    && editor.line_cursor == editor.line.len(),
            );

        if let Some(unicode_cursor) = editor.unicode_cursor {
            let unicode_cursor_offset =
                2 + u16::try_from(unicode_cursor).expect("unicode_cursor <= 6");
            if self.cursor_position.0 >= unicode_cursor_offset {
                self.unicode_input_position = Some(Coord2D(
                    self.cursor_position.0 - unicode_cursor_offset,
                    self.cursor_position.1,
                ));
            } else {
                let prompt_width = if line_cursor_buf_line == 0 {
                    self.prompt.map_or(0, |p| char_width(p, 0))
                } else {
                    0
                };
                let cols_before_unicode = str_width(
                    &editor.line[buf_lines[line_cursor_buf_line].start
                        ..editor.line_cursor],
                    0,
                ) + prompt_width;
                self.unicode_input_position = Some(Coord2D(
                    cols_before_unicode,
                    self.cursor_position.1 - 1,
                ));
                self.first_display_line -= 1;
            }
        } else {
            self.unicode_input_position = None;
        }
        self.state = ViewState::Valid;
    }

    /// If View isn't in a Valid state, update and return wrapped
    /// buffer lines and amount view needs to scroll, otherwise return None.
    fn update(&mut self, editor: &LineEditor) -> Option<u16> {
        if self.is_valid() {
            return None;
        }

        let buf_lines = self.wrap_lines(editor);

        let mut scroll_lines = 0;

        let line_cursor_buf_line = buf_lines
            .iter()
            .position(|s| s.contains(&editor.line_cursor))
            .expect("line_cursor is in or just after buffer");

        let prompt_width = if line_cursor_buf_line == 0 {
            self.prompt.map_or(0, |p| char_width(p, 0))
        } else {
            0
        };

        let current_lines_to_bottom =
            usize::from(self.size.1 - 1 - self.first_display_line);

        let mut first_visible_buf_line = buf_lines
            .iter()
            .position(|l| l.contains(&self.visible_chars.start))
            .expect("visible_chars are in the buffer");

        let new_line_cursor_x = prompt_width
            + str_width(
                &editor.line
                    [buf_lines[line_cursor_buf_line].start..editor.line_cursor],
                prompt_width,
            );

        let new_line_cursor_y = if first_visible_buf_line
            + current_lines_to_bottom
            < line_cursor_buf_line
        {
            // line_cursor below display
            let delta = line_cursor_buf_line
                - (first_visible_buf_line + current_lines_to_bottom);
            scroll_lines = u16::try_from(cmp::min(
                usize::from(self.first_display_line),
                delta,
            ))
            .expect("scroll_lines fits u16");
            self.first_display_line -= scroll_lines;
            self.size.1 - 1
        } else if line_cursor_buf_line < first_visible_buf_line {
            // Only possible if first_display_line was 0
            first_visible_buf_line = line_cursor_buf_line;
            0
        } else {
            self.first_display_line
                + u16::try_from(line_cursor_buf_line - first_visible_buf_line)
                    .expect("new cursor y fits u16")
        };
        self.cursor_position = Coord2D(new_line_cursor_x, new_line_cursor_y);

        if let Some(unicode_cursor_idx) = editor.unicode_cursor {
            // Unicode input mode active
            let mut unicode_input_position = self.cursor_position;
            self.cursor_position.0 += 2 + u16::try_from(unicode_cursor_idx)
                .expect("unicode_cursor <= 6");
            if self.cursor_position.0 >= self.size.0 {
                // Unicode wrapped to next line
                self.cursor_position.0 -= self.size.0;
                if self.cursor_position.1 == self.size.1 - 1 {
                    // Cursor now below display
                    if self.first_display_line > 0 {
                        self.first_display_line -= 1;
                        scroll_lines += 1;
                    }
                    unicode_input_position.1 -= 1;
                } else {
                    self.cursor_position.1 += 1;
                }
            }
            self.unicode_input_position = Some(unicode_input_position);
        } else {
            // Unicode input mode not active
            self.unicode_input_position = None;
        }

        let last_visible_buf_line = cmp::min(
            buf_lines.len() - 1,
            first_visible_buf_line
                + usize::from(self.size.1 - 1 - self.first_display_line),
        );
        self.visible_chars.start = buf_lines[first_visible_buf_line].start;
        self.visible_chars.end =
            cmp::min(buf_lines[last_visible_buf_line].end, editor.line.len());

        self.state = ViewState::Valid;
        Some(scroll_lines)
    }

    /// render current buffer to display
    #[cfg(not(tarpaulin_include))]
    pub fn repaint(&mut self, editor: &LineEditor) -> io::Result<()> {
        let Some(scroll_lines) = self.update(editor) else {
            return Ok(());
        };

        // redraw display
        let mut stdout = io::stdout().lock();

        stdout.queue(Hide)?;

        if scroll_lines > 0 {
            stdout.queue(ScrollUp(scroll_lines))?;
        }

        stdout
            .queue(MoveTo(0, self.first_display_line))?
            .queue(Clear(ClearType::FromCursorDown))?;

        write!(
            stdout,
            "{}{}",
            self.prompt.unwrap_or_default(),
            &editor.line[self.visible_chars.clone()],
        )?;

        if let Some(unicode_position) = self.unicode_input_position {
            // Render Unicode input field
            stdout.queue(MoveTo(unicode_position.0, unicode_position.1))?;
            print!(
                "{}\\u{} {}",
                Attribute::Reverse,
                editor.unicode,
                Attribute::NoReverse
            );
        }
        stdout
            .queue(MoveTo(self.cursor_position.0, self.cursor_position.1))?
            .queue(Show)?
            .flush()
    }

    /// Generate list of spans representing
    /// the chars that would be displayed, wrapped
    /// to display width, leaving room for cursor
    /// at end if necessary.
    #[must_use]
    fn wrap_lines(&self, editor: &LineEditor) -> Vec<Range<usize>> {
        let mut lines = Vec::new();
        let mut cols = self.prompt.map_or(0, |ch| char_width(ch, 0));
        let mut begin = 0;
        let mut end;
        for (i, ch) in editor.line.char_indices() {
            let w = char_width(ch, cols);
            end = i;
            if self.size.0 - cols < w {
                lines.push(begin..end);
                cols = 0;
                begin = i;
            }
            cols += w;
        }

        // leave room for cursor at end, if necessary
        end = editor.line.len();
        if editor.line_cursor == end {
            if cols == self.size.0 {
                lines.push(begin..end);
                begin = end;
            }
            end = editor.line.len() + 1;
        }
        lines.push(begin..end);

        lines
    }
}

impl Drop for View {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = io::stdout().execute(Show);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use similar_asserts::assert_eq;

    pub struct ViewBuilder {
        size: DimWH,
        first_display_line: u16,
        cursor_position: Coord2D,
        visible_chars: Range<usize>,
        prompt: Option<char>,
        unicode_input_position: Option<Coord2D>,
        state: ViewState,
    }

    impl ViewBuilder {
        pub fn new() -> Self {
            ViewBuilder {
                size: DimWH(10, 5),
                first_display_line: 0,
                cursor_position: Coord2D(0, 0),
                visible_chars: 0..0,
                prompt: Some(':'),
                unicode_input_position: None,
                state: ViewState::Valid,
            }
        }

        pub fn build(&self) -> View {
            let mut v =
                View::new(self.size, self.first_display_line, self.prompt);
            v.cursor_position = self.cursor_position;
            v.visible_chars.start = self.visible_chars.start;
            v.visible_chars.end = self.visible_chars.end;
            v.unicode_input_position = self.unicode_input_position;
            v.state = self.state;
            v
        }

        pub fn with_size(&mut self, size: DimWH) -> &mut Self {
            assert!(size.0 >= 9, "Min display width is 9 columns");
            self.size = size;
            self
        }

        pub fn with_first_display_line(&mut self, fdl: u16) -> &mut Self {
            self.first_display_line = fdl;
            self
        }

        pub fn with_cursor_position(&mut self, pos: Coord2D) -> &mut Self {
            self.cursor_position = pos;
            self
        }

        pub fn with_visible_chars(&mut self, cs: Range<usize>) -> &mut Self {
            self.visible_chars = cs;
            self
        }

        pub fn with_prompt(&mut self, p: Option<char>) -> &mut Self {
            self.prompt = p;
            self
        }

        pub fn with_unicode_input_position(
            &mut self,
            uip: Option<Coord2D>,
        ) -> &mut Self {
            self.unicode_input_position = uip;
            self
        }

        pub fn with_state(&mut self, s: ViewState) -> &mut Self {
            self.state = s;
            self
        }
    }

    #[test]
    fn coord2d_from_u16_u16() {
        let f = (169u16, 13u16);
        let t = Coord2D(169, 13);

        assert_eq!(t, Coord2D::from(f));
    }

    #[test]
    fn dimwh_from_u16_u16() {
        let f = (169u16, 13u16);
        let t = DimWH(169, 13);

        assert_eq!(t, DimWH::from(f));
    }

    #[test]
    fn update_empty_buffer_with_prompt() {
        let editor = LineEditor::new();
        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_prompt(Some(':'))
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(0, 23))
            .with_first_display_line(23)
            .with_state(ViewState::Invalid)
            .build();

        let expected_view = vb
            .with_cursor_position(Coord2D(1, 23))
            .with_first_display_line(23)
            .with_state(ViewState::Valid)
            .build();

        let scroll_lines = view.update(&editor);

        assert_eq!(view, expected_view);
        assert_eq!(scroll_lines, Some(0));
    }

    #[test]
    fn update_one_char_added() {
        let editor = LineEditor::from("\u{1f3b8}");
        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_prompt(Some(':'))
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(1, 23))
            .with_first_display_line(23)
            .with_state(ViewState::Invalid)
            .with_visible_chars(0..0)
            .build();

        let expected_view = vb
            .with_cursor_position(Coord2D(3, 23))
            .with_visible_chars(0..editor.line.len())
            .with_state(ViewState::Valid)
            .build();

        let scroll_lines = view.update(&editor);

        assert_eq!(scroll_lines, Some(0));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn line_cursor_moved_above_display() {
        let mut editor = LineEditor::from(
            "012345678\
             9012345678\
             9012345678\
             9012345678\
             9012345678\
             9012345678",
        );
        editor.line_cursor = 0;

        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_size(DimWH(10, 5))
            .with_prompt(Some(':'))
            .with_cursor_position(Coord2D(0, 4)) // ip was at end
            .with_first_display_line(0)
            .with_visible_chars(19..editor.line.len())
            .with_state(ViewState::Invalid)
            .build();

        let expected_view = vb
            .with_cursor_position(Coord2D(1, 0)) // cursor at start of input
            .with_visible_chars(0..editor.line.len() - 10) // view moved up one line
            .with_state(ViewState::Valid)
            .build();

        let scroll_lines = view.update(&editor);

        assert_eq!(scroll_lines, Some(0));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_on_valid_view_is_nop() {
        let editor = LineEditor::from("buffer text");
        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_prompt(Some(':'))
            .with_cursor_position(Coord2D(
                u16::try_from(editor.line.len()).unwrap() + 1,
                23,
            ))
            .with_first_display_line(23)
            .with_visible_chars(0..editor.line.len())
            .with_state(ViewState::Valid)
            .build();

        let expected_view = view.clone();

        let scroll_lines = view.update(&editor);

        assert_eq!(scroll_lines, None);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_backspace_past_column_0() {
        let editor = LineEditor::from("12345678");
        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_size(DimWH(10, 5))
            .with_prompt(Some(':'))
            .with_cursor_position(Coord2D(0, 4))
            .with_first_display_line(3)
            .with_visible_chars(0..editor.line.len())
            .with_state(ViewState::Invalid)
            .build();

        let expected_view = vb
            .with_cursor_position(Coord2D(9, 3))
            .with_state(ViewState::Valid)
            .build();

        let scroll_lines = view.update(&editor);

        assert_eq!(scroll_lines, Some(0));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_added_char_at_display_end() {
        let editor = LineEditor::from("012345678");
        let mut vb = ViewBuilder::new();

        let mut view = vb
            .with_prompt(Some(':'))
            .with_size(DimWH(10, 5))
            .with_cursor_position(Coord2D(9, 4))
            .with_first_display_line(4)
            .with_visible_chars(0..9)
            .with_state(ViewState::Invalid)
            .build();

        let expected_view = vb
            .with_cursor_position(Coord2D(0, 4))
            .with_first_display_line(3)
            .with_visible_chars(0..9)
            .with_state(ViewState::Valid)
            .build();

        let scroll_lines = view.update(&editor);

        assert_eq!(view, expected_view);
        assert_eq!(scroll_lines, Some(1));
    }

    #[test]
    fn resize_with_no_change_does_nothing() {
        let size = DimWH(10, 5);
        let cursor_pos = Coord2D(11, 0);

        let editor = LineEditor::from("buffer text");

        let mut view = ViewBuilder::new()
            .with_size(size)
            .with_cursor_position(cursor_pos)
            .build();
        let expected_view = view.clone();

        view.resize(size, cursor_pos, &editor);

        assert_eq!(view, expected_view);
    }

    #[test]
    fn resize_saves_values_and_revalidates() {
        let editor = LineEditor::from(
            "0123456789012345678901234567890123456789012345678",
        );

        let mut vb = ViewBuilder::new();
        let mut view = vb
            .with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(
                editor.line.len().try_into().unwrap(),
                23,
            ))
            .with_first_display_line(23)
            .build();

        let expected_view = vb
            .with_size(DimWH(10, 5))
            .with_first_display_line(0)
            .with_cursor_position(Coord2D(0, 4))
            .with_visible_chars(9..editor.line.len())
            .build();
        view.resize(DimWH(10, 5), Coord2D(0, 4), &editor);

        assert!(view.is_valid());
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_unicode_field_fits_after_cursor() {
        let mut editor = LineEditor::from("abcde");
        let mut vb = ViewBuilder::new();
        vb.with_size(DimWH(80, 24))
            .with_cursor_position(Coord2D(6, 23))
            .with_visible_chars(0..5)
            .with_first_display_line(23);
        let mut view = vb.build();

        let mut expected_view = view.clone();
        let unicode_pos = expected_view.cursor_position;
        expected_view.unicode_input_position = Some(unicode_pos);
        expected_view.cursor_position = Coord2D(12, 23);

        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);
        view.unicode_input_position = Some(unicode_pos);
        view.invalidate();

        let scroll = view.update(&editor);
        assert_eq!(scroll, Some(0));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_unicode_field_too_wide_after_cursor() {
        let mut editor = LineEditor::from("abcde");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);

        let mut vb = ViewBuilder::new();
        vb.with_size(DimWH(10, 5));
        let mut view = vb
            .with_cursor_position(Coord2D(6, 3))
            .with_first_display_line(3)
            .with_visible_chars(0..5)
            .build();

        let expected_view = vb
            .with_unicode_input_position(Some(Coord2D(6, 3)))
            .with_cursor_position(Coord2D(2, 4))
            .with_first_display_line(3)
            .build();
        view.invalidate();

        let scroll = view.update(&editor);
        assert_eq!(scroll, Some(0));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn update_unicode_field_too_wide_after_cursor_past_bottom() {
        let mut editor = LineEditor::from("abcde");
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);

        let mut vb = ViewBuilder::new();
        vb.with_size(DimWH(10, 5));
        let mut view = vb
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .with_visible_chars(0..5)
            .build();

        let expected_view = vb
            .with_unicode_input_position(Some(Coord2D(6, 3)))
            .with_cursor_position(Coord2D(2, 4))
            .with_first_display_line(3)
            .build();
        view.invalidate();

        let scroll = view.update(&editor);
        assert_eq!(scroll, Some(1));
        assert_eq!(view, expected_view);
    }

    #[test]
    fn resize_handles_unicode_input() {
        // Set up view with active unicode input field
        // such that resize will cause both cursor and
        // unicode_input_position to change.
        let mut editor = LineEditor::from("012345678abcde");
        editor.line_cursor = 0;
        editor.unicode = "0308".to_owned();
        editor.unicode_cursor = Some(4);

        let mut vb = ViewBuilder::new();
        vb.with_size(DimWH(20, 5)).with_visible_chars(0..14);
        let mut view = vb
            .with_unicode_input_position(Some(Coord2D(0, 4)))
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .build();

        let expected_view = vb
            .with_size(DimWH(25, 5))
            .with_unicode_input_position(Some(Coord2D(0, 4)))
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .build();
        view.resize(DimWH(25, 5), Coord2D(6, 4), &editor);
        assert_eq!(view, expected_view);

        let expected_view = vb
            .with_size(DimWH(10, 5))
            .with_unicode_input_position(Some(Coord2D(1, 3)))
            .with_cursor_position(Coord2D(1, 4))
            .with_first_display_line(3)
            .build();

        view.resize(DimWH(10, 5), Coord2D(1, 4), &editor);
        assert_eq!(view, expected_view);

        let expected_view = vb
            .with_size(DimWH(20, 5))
            .with_unicode_input_position(Some(Coord2D(0, 4)))
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .build();
        view.resize(DimWH(20, 5), Coord2D(6, 4), &editor);
        assert_eq!(view, expected_view);

        editor.line_cursor = editor.line.len();
        let expected_view = vb
            .with_size(DimWH(25, 5))
            .with_unicode_input_position(Some(Coord2D(0, 4)))
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .build();
        view.resize(DimWH(25, 5), Coord2D(6, 4), &editor);
        assert_eq!(view, expected_view);

        let expected_view = vb
            .with_size(DimWH(10, 5))
            .with_unicode_input_position(Some(Coord2D(5, 3)))
            .with_cursor_position(Coord2D(1, 4))
            .with_first_display_line(2)
            .build();

        view.resize(DimWH(10, 5), Coord2D(1, 4), &editor);
        assert_eq!(view, expected_view);

        let expected_view = vb
            .with_size(DimWH(20, 5))
            .with_unicode_input_position(Some(Coord2D(0, 4)))
            .with_cursor_position(Coord2D(6, 4))
            .with_first_display_line(4)
            .build();
        view.resize(DimWH(20, 5), Coord2D(6, 4), &editor);
        assert_eq!(view, expected_view);
    }

    #[test]
    fn char_width_of_tab() {
        assert_eq!(char_width('\t', 0), 8);
        assert_eq!(char_width('\t', 1), 7);
        assert_eq!(char_width('\t', 2), 6);
        assert_eq!(char_width('\t', 7), 1);
        assert_eq!(char_width('\t', 8), 8);
        assert_eq!(char_width('\t', 20), 4);
    }
}
