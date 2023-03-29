# Feature summary

This document describes lned's feature set. It is split into two main
sections: MVP (minimum viable product) and Enhancements.

# MVP Feature Set

This section describes the minimum features to allow lned to be used
(albeit painfully) for its own continued development. Once these
features are complete I could (and will try to) dogfood lned development.

* launch, with optional file path to read in
  - If no file is specified, create empty edit buffer reading file.
* Simple Prompt (":")

## Line Addressing

A line address specifies a line in a buffer. *lned* keeps track of the
_current line_, which commands typically use if no address is
specified. When a file is read into a buffer, the current line is
the last line read. After a command executes, the current line
is set to the last line affected by a command.

A line address is specified by one of:

.        The current line.
$        The last line in the buffer.
_n_      The _n_th line in the buffer, in the range [1,$]
0        Before the first line, where that makes sense (i.e., append or insert).

An line span is a closed range, represented by two line
addresses separated by a comma or semicolon. The value of the first
address must not exceed the value  of the second. If only one address
is given where a span is expected, it is treated as if the specified
line was given as both the beginning and end of the span.
If a line span is given where a command expects a single line
address, the last line specified by the span is used.

Two special shortcuts for common spans exist:

, or %   All the lines in the buffer; equivalent to the span 1,$.
;        The current through last lines in the buffer; equivalent to
         the span .,$.

When multiple buffer support is added, both addresses and spans may be prefixed
by a buffer specifier, which is separated from the line address or span by a colon (':').
The buffer specifier will initially be the buffer number.

## Commands

Commands are listed with the default address or address range
(in parentheses) used if none is given. Possible arguements are shown
as applicable.

There are two main types of commands understood by lned: edit commands,
which may be undone, and immediate commands, which may not. Immediate
commands which will have destructive side effects will not take effect
the first time they are given, and instead warning text explaining the
potential data loss will be written to stdout. Issuing the same
immediate command a second time with no other commands intervening will
actually execute the command. This will also be documented for each
such command.

### Edit Commands
(.)a            Appends text entered in input mode after the specified
                line, setting the current line to the last line entered.

(.,.)c          Change the specified lines by deleting them and appending
                text entered in input mode in their place. The buffer's current
                line is set to the last line enterd.

(.,.)m(.)       Move the specfied lines to the after the specified destination
                line. The current address is set to the last line moved.

//TODO - fill out descriptions of insert, transfer (copy), join, Justify
(.,.)i

(.,.)t(.)

(.,.+1)j <"join string">

(.,.+1)J_w_

(.,.)d          Delete the specified lines. The current line is set to the line
                after the deleted lines, if there is one, otherwise to the line
                before them.

### Immediate Commands

e _file_        Edit _file_ in a new buffer. If no _file_ is specified, create a
                new, empty buffer. Either way, the new buffer becomes the active
                buffer. The last line in the new buffer becomes the buffer's
                current line and, if specified, the buffer's default file is
                set to _file_.

f _file_        Set the current buffer's default filename to _file_.
                If no _file_ is given, prints the buffer's default filename.

(.,.)n          Write the specified lines to stdout, prefixed with line numbers.
                The buffer's current line is set to the last line written.

(.,.)p          Write the specified lines to stdout. The buffer's current line
                is set to the last line written.

q               Quits lned. If there are unwritten changes in any
                buffer, a warning to stdout. A second consecutive quit command
                will exit unconditionally, discarding unwritten changes.

(.+1)z_n_       Display _n_ lines starting at the specified line. If _n_ is not
                specified, then the current terminal window height is used. If
                it isn't possible to fetch the current terminal windown height,
                the configured scroll_window_size is used as the final fallback
                (defaulting to 25).

(1,$)w _file_   Write the specified lines to _file_, overwriting
                previous contents without warning. If there is no
                default filename, it is set to _file_, otherwise it
                is unchanged. If _file_ is not given, the default
                filename is used if it is set, otherwise an error is given.

## Input Mode

When edit commands that take user input (such as *a*) are given,
lned enters Input Mode. When in Input Mode, commands are not
available -- standard input is instead collected until input mode is
terminated by a single period alone on a line. Lines of input are
terminated by CR or CRLF.

The interrupt signal (usually CTRL-c) will also exit Input Mode, but
will discard the input text.

