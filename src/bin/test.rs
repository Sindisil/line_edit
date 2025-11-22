use line_edit::EditorOptions;
use line_edit::LineEdit;
use line_edit::LineEditor;

#[cfg(not(tarpaulin_include))]
fn main() {
    let mut line = String::new();
    let mut reader = LineEditor::new();
    let res = reader.read_line(
        &mut line,
        Some(&EditorOptions { prompt: Some(':'), ..Default::default() }),
    );
    match res {
        Err(e) => eprintln!("{e}"),
        Ok(bytes_read) => {
            eprintln!("read {bytes_read} bytes\n{line}");
        }
    }
}
