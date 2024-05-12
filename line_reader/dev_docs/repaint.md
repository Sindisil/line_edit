# Requirements
    1. Cursor needs to alwasy be in viewport to provide editing context.
       Viewport is defined as the full terminal window if terminal
       height is < 3, or one line short of the terminal top or bottom
       if the full buffer doesn't fit in that direcction (i.e., there
       will be lines "off screen" in that direction).
    2. Old term content should be scrolled out of window as needed,
       to preserve scrollback buffer history.
    3. Cursor shouldn't move around needlessly, to avoid disorienting
       the user.
    4. Modulo terminl bugs, line_reader should support terminal window
       resizing.


# Rendering methods

## I. Cursor centric

### A. Top level procedure

	1.	Compute new cursor position given current display start
		and buffer content.
	2.	If cursor would be outside viewport, compute new display start
	   	and cursor position so that cursor will be on last line of
	   	viewport in that direction.
	3.	If new display start is above current display start, compute
		the appropriate number of lines to scroll to preserve any
		scrollback buffer history.
    4.  Render buffer to display
    5.  Save new cursor position and display start

### B. Subtasks

#### 1. Compute new cursor position

    todo

#### 2. Compute new display start

    todo

#### 3. Compute scroll distance

    todo

#### 4. Render buffer to display
	a.	Hide cursor.
	b.  ScrollUp if needed.
	c.	Move cursor to new display start
	d.	Clear to end of terminal
	e.	Write before_gap to display, starting from display start
	f.  If after_gap isn't empty, write as much of it to display as
		will fit.
    g.  Move cursor to new cursor position
    h.  Show cursor
    	   
