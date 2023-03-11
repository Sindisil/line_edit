mod cli;
mod main_loop;
mod edit_buffer;

fn main() {
    let stdout = std::io::stdout();
    let args = match cli::parse_args(&mut stdout.lock(), wild::args_os()) {
        Ok(args) => args,
        Err(cli::Error::WroteMessage) => std::process::exit(0),
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    };
    println!("{args:#?}");
}
