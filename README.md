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

## Deletion
| Key Binding                      | Action                             |
|----------------------------------|------------------------------------|
| Backspace                        | Delete char before cursor          |
| Ctrl + Home                      | Delete before cursor               |
| Delete           or Ctrl + d     | Delete to next non zero width char |
| Ctrl + End       or Ctrl + k     | Delete from cursor to end          |
| Ctrl + Backspace or Alt + Delete | Delete to previous word start      |
| Ctrl + Delete    or Alt + d      | Delete to next word start          |

## Indentation & Tabs
| Key Binding      | Action                                    |
|------------------|-------------------------------------------|
| Tab              | Indent line one tab stop                  |
| Shift + Tab      | Dedent line one tab stop                  |
| Ctrl + i         | Insert one tab character ('\t') at cursor |

Indent means insert enough spaces to move the first printable
character of the relevant region to the next tab stop. Dedent
means delete enough spaces to move the first printable character
to the previous tab stop.

When indenting or dedenting the whole line, if the line begins
with a tab ('\t') character, a single tab will be inserted or
deleted from the start of line, rather than some number of spaces.

Tab stops are currently defined as 4 spaces.

## History
| Key Binding            | Action                                  |
|------------------------|-----------------------------------------|
| Up         or Ctrl + p | Display next older history              |
| Down       or Ctrl + n | Display next newer history              | 
| Esc        or Ctrl + g | Display draft input                     |
| F8         or Ctrl + r | Find next older history matching buffer |
| Shift + F8 or Ctrl + p | Find next newer history matching buffer |

Note: line_input began life as a sub-crate of [lned](https://github.com/Sindisil/lned). References to issue numbers previous to Oct. 1, 2025 refer to issues within that project. I've chosen not to move the closed issues here because they this library (then called line_read) was most often changed in lockstep with lned.
