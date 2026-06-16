use line_edit::EditorOptions;
use line_edit::LineEdit;
use line_edit::LineEditor;

#[cfg(not(tarpaulin_include))]
fn main() {
    let mut line = String::new();
    let mut reader = LineEditor::new();
    loop {
        let res = reader.accept_line(
            &mut line,
            Some(&EditorOptions { prompt: Some(':'), ..Default::default() }),
        );
        match res {
            Err(e) => println!("{e}"),
            Ok(bytes_read) => {
                println!("read {bytes_read} bytes\n{line:?}");
                if line.trim() == "q" {
                    println!("exiting");
                    break;
                }
            }
        }
        line.clear();
    }
}
