//! glisa-ctl entry point (see the library for the commands).

use std::process::ExitCode;

fn main() -> ExitCode {
    glisa_i18n::set_language(None);
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut ask = glisa_ctl::ask_terminal;
    let mut io = glisa_ctl::Io {
        out: &mut out,
        err: &mut err,
        ask: &mut ask,
    };
    ExitCode::from(glisa_ctl::run(&argv, &mut io))
}
