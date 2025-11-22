# line_input
Rust library providing line input with simple editing and optional
prompt character and history.

# Standard default bindings

## Navigation
Left                 Cursor back
Right                Cursor forward
Home                 Cursor to start
End                  Cursor to end
Control + Left       Cursor back word
Control + right      Cursor forward word

## Deletion
Delete               Delete from cursor up to next non zero width
                     character
Backspace            Delete character before cursor
Control + Home       Delete before cursor
Control + End        Delete from cursor to end
Control + Backspace  Delete to previous word start
Control + Delete     Delete up to next word start

## Indentation & Tabs
Tab                  Indent buffer one stop
Shift + Tab          Dedent buffer one stop
Control + i          Insert one tab character ('\t') at cursor

## History
Up                   Display next older history
Down                 Display next newer history
Esc                  Display draft input
F8                   Find next older history matching buffer
Shift + F8           Find next newer history matching buffer

Note: line_input began life as a sub-crate of [lned](https://github.com/Sindisil/lned). References to issue numbers previous to Oct. 1, 2025 refer to issues within that project. I've chosen not to move the closed issues here because they this library (then called line_read) was most often changed in lockstep with lned.
