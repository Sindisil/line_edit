# line_input
Rust library providing line input with simple editing and optional
prompt character and history.

# Key bindings

line_input supports both Windows and Bash/emacs style key bindings
for editing the input line.


## Navigation
| Key Binding              | Action              |
|--------------------------|---------------------|
| Left         or Ctrl + b | Cursor back         | 
| Right        or Ctrl + f | Cursor forward      |
| Home         or Ctrl + a | Cursor to start     | 
| End          or Ctrl + e | Cursor to end       |
| Ctrl + Left  or Alt + b  | Cursor back word    |
| Ctrl + Right or Alt + f  | Cursor forward word |

## Editing
| Key Binding                      | Action                              |
|----------------------------------|-------------------------------------|
| Backspace                        | Delete char before cursor           |
| Ctrl + Home                      | Delete before cursor to start       |
| Delete           or Ctrl + d     | Delete to next non zero width char  |
| Ctrl + End       or Ctrl + k     | Delete from cursor to end           |
| Ctrl + Backspace or Alt + Delete | Delete to previous word start       |
| Ctrl + Delete    or Alt + d      | Delete to next word start           |
| Ctrl + i                         | Insert one tab ('\t') at cursor     |
| Ctrl + u                         | Insert Unicode code point at cursor |
| Tab                              | Indent line one tab stop            |
| Shift + Tab                      | Dedent line one tab stop            |

### Unicode input
The Unicode input feature accepts a Unicode code point as up to six
hexidecimal digits. Pressing Enter inserts the character specified by the
code point a the cursor position. If the code point is invalid, or input
is canceled with Exc or Ctrl + g, no character is inserted and line_edit
returns to normal editing. Most edit commands are usable during Unicode
input, with the exception of history commands, word oriented commands, and
indent/dedent. As mentioned, Unicode input may be canceled with Esc or
Ctrl + g.

### Indent/Dedent
Indent means insert enough spaces to move the first printable character on
the line to the next tab stop. Dedent means delete enough spaces to move
the first printable character to the previous tab stop.

When indenting or dedenting, if the line begins with a tab ('\t'), a single
tab will be inserted or deleted from the start of line, rather than some
number of spaces.

Tab stops are currently defined as 4 spaces or one tab character ('\t').

## History
| Key Binding                  | Action                                  |
|------------------------------|-----------------------------------------|
| Up         or Ctrl + p       | Display next older history              |
| Down       or Ctrl + n       | Display next newer history              | 
| Esc        or Ctrl + g       | Cancel history editing, restoring draft |
| F8         or Ctrl + r       | Find next older history matching buffer |
| Shift + F8 or Ctrl + p       | Find next newer history matching buffer |


# Contributing
Policy may change in the future, but for now line_input is not open for pull
requests. I'm open to bug reports and/or requests
(keeping in mind that it is intended to be *simple* and primarily to fit
my particular needs).

**Note:**
> line_input began life as a sub-crate of [lned](https://github.com/Sindisil/lned).
References to issue numbers previous to Oct. 1, 2025 refer to issues
within that project. I've chosen not to move the closed issues here
because at the time this library (then called line_read) was most often
changed in lockstep with lned.

# Generative AI Policy
No Generative AI has been used in the development of line_edit.

Issues generated with AI agents will be rejected.
If line_edit eventually opens up to external pull requests, any pull requests
generated with AI agents will also be rejected.

# License
SPDX-License-Identifier: MIT OR Apache-2.0

Licensed under either of:

* [Apache License, Version 2.0](https://apache.org/licenses/LICENSE-2.0)
* [MIT License](https://mit-license.org/)

You may use it under either license, at your option.

Copyright © 2023 Greg A. Jandl
