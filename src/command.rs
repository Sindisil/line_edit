use std::fmt;

#[derive(Debug, PartialEq)]
pub enum Cmd {
    //Quit,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Unknown(String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Unknown(s) => write!(f, "Unknown command '{s}'"),
        }
    }
}

impl Cmd {
    pub fn parse(input: &str) -> Result<Cmd, Error> {
        Err(Error::Unknown(input.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unknown_command_gives_error() {
        let input = "o";
        let res = Cmd::parse(&input);
        assert_eq!(res, Err(Error::Unknown(input.to_string())));
    }
}
