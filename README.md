# line_input
Rust library providing line input with simple editing and optional
prompt character and history.

# Standard default bindings

## Navigation
| Key Binding  | Action              |
|--------------|---------------------|
| Left         | Cursor back         | 
| Right        | Cursor forward      |
| Home         | Cursor to start     | 
| End          | Cursor to end       |
| Ctrl + Left  | Cursor back word    |
| Ctrl + right | Cursor forward word |

## Deletion
| Key Binding      | Action                                     |
|------------------|--------------------------------------------|
| Delete           | Delete up to next non zero width character |
| Backspace        | Delete character before cursor             |
| Ctrl + Home      | Delete before cursor                       |
| Ctrl + End       | Delete from cursor to end                  |
| Ctrl + Backspace | Delete to previous word start              |
| Ctrl + Delete    | Delete up to next word start               |

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
| Key Binding | Action                                  |
|-------------|-----------------------------------------|
| Up          | Display next older history              |
| Down        | Display next newer history              | 
| Esc         | Display draft input                     |
| F8          | Find next older history matching buffer |
| Shift + F8  | Find next newer history matching buffer |

Note: line_input began life as a sub-crate of [lned](https://github.com/Sindisil/lned). References to issue numbers previous to Oct. 1, 2025 refer to issues within that project. I've chosen not to move the closed issues here because they this library (then called line_read) was most often changed in lockstep with lned.
