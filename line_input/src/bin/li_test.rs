use line_input::LineInput;
use line_input::LineRead;

fn main() {
    let mut line = String::new();
    let mut input = LineInput::new();
    let res = input.read_line(&mut line, ":");
    match res {
        Err(e) => eprintln!("{e}"),
        Ok(bytes_read) => {
            eprintln!("read {bytes_read} bytes\n{line}");
        }
    }
}
