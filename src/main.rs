mod app;
mod cli;
mod fmt;
mod http;
mod meta;
mod net;
mod ping;
mod speedtest;
mod traffic;
mod ui;
mod util;
mod verdict;

fn main() {
    let opts = match cli::parse(std::env::args_os().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if opts.tcp {
        ping::force_tcp();
    }
    match opts.mode {
        cli::Mode::Help => println!("{}", cli::help()),
        cli::Mode::Version => println!("netekg {}", env!("CARGO_PKG_VERSION")),
        cli::Mode::Speed { json } => std::process::exit(cli::run_speed(json)),
        cli::Mode::Tui => {
            if let Err(e) = app::run() {
                eprintln!("netekg: {e}");
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("target") && !p.ends_with(".git") {
                    files(&p, out);
                }
            } else {
                out.push(p);
            }
        }
    }

    // Every text file in the project is English and uses plain hyphens.
    #[test]
    fn texts_are_english_with_plain_hyphens() {
        let mut all = Vec::new();
        files(Path::new(env!("CARGO_MANIFEST_DIR")), &mut all);
        for p in all {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for (i, line) in text.lines().enumerate() {
                let bad = line.chars().any(|c| matches!(c, '\u{2013}' | '\u{2014}' | '\u{0400}'..='\u{04ff}'));
                assert!(!bad, "{}:{}: {line}", p.display(), i + 1);
            }
        }
    }
}
