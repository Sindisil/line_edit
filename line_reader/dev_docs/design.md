# Overall Design

Main data structure is a gap buffer. The gap is always at the current
cursor position, since typing in text is assumed to be the most frequent
action. 

The gap is moved by navigation actions (Left, Right, Home, etc.).

## Repaint

Repaintng is done when:

* Buffer content (not just cursor position) has changed
* Buffer content logically above or below the current screen
  needs to be revealed. This could be for several reasons:
    1. Cursor moved left (by BS or a cursor movement command)
       off top line of terminal.
    2. Cursor moved right off bottom line of terminal. Possibly only
       necessary if that movement was due to cursor movement, since
       outputting a character might line wrap and scroll. Will need
       to test in practice to see if repaint is actually needed in
       that case. If this behavior differs for different terminals
       I am beginning to see why some other read_line type utilites
       essentially repaint the whole visible portion of the buffer
       after each event.
    3. Terminal window expanded by resize to allow more lines
       of the input buffer to be shown. Only actually necessary
       if the terminal has less scroll back buffer than nedded
       to hold the input lines, but since there's no portable
       way to detect scrollback size or whether specific lines
       are still in the scrollback buffer ready to be revealed
       on resize, I need to repaint revealed lines to ensure
       they are displayed correctly.

One UX design question is whether or not LineInput should attempt
to keep a one line "buffer zone" ("buffer" in text editor development
seems to be as overloaded as "level" in RPGs) visible (i.e., the
cursor should only be on the top or bottom line of the terminal
if it is in the first or last line of buffer text, respectively).

It would be simpler to not worry about it, but being able to see
what text would be affected by a delete in the last position, or
a BS in the first position, would be desirable, I think. Also,
seeing more text would allow for more efficient navigation. Of
course, in the common case, the input would be a handfull of
lines at most, and often only a single line, so the whole bufer
would most often be on screen at all times. Still an edge case
possibly worth handling.

Repaint is done by reprinting the text on the cursor line onward to
either the end of the buffer or the end of the screen, whichever comes
first. This is another place where the repainting could be optimized
by repainting less when possible. However, given how few characters are
displayed on even a large termial window, and how small a portion the
input editor would usually be displaying (most often only one line), it
probably doesn't make sense to bother.

## Edit states

LineReader has several ReaderStates:

    EditInput
    ViewHistory
    EditHistory
    Accept

When history search is added, it will probably result in another
ReaderState (e.g., SearchHistory).

### EditInput

LineReader starts in this state. The input_buffer is displayed and
manipulated in this state. Transitions out of this state are:

#### EditInput

In this state, the input_buffer is displayed and manipulated.

[Enter] => Accept
[Up] => ViewHistory
[Esc] if previous state was EditHistory => EditHistory
_ => EditInput

#### ViewHistory

In this state, a line from the line_history list is displayed. When
transitioning into this state, the most recent (i.e., last) line of
line_history is displayed.

[Up] => ViewHistory (moves to next older line of history)
[Down] if at end of line_history => EditInput
[Down] => ViewHistory (move to next most recent line of history)
[Esc] if previous state was EditInput => EditInput
[Esc] if previous state was EditHistory => EditHistory
[Enter] => Accept
_ => EditHistory

#### EditHistory

When entering this state from ViewHistory, history_buffer is initialized
from the currently displayed line in line_history. The contents and state
persist until accepted or replaced by a subsequent transition from
ViewHistory.

[Up] => ViewHistory
[Esc] => EditInput
[Enter] => Accept
_ => EditHistory

#### Accept

If non-empty, the currently displayed line (whether input_buffer,
history_buffer, or an item in line_history) is pushed onto end of
line_history and copied into output buffer. In future, might shrink
one or both of the buffers if they're beyond some limit, as well, to
optimize memory consumption.

