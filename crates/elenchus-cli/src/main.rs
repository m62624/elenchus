//! `elenchus` — check an elenchus `.vrf` program (string or file) and report.
//!
//! Exit code mirrors the verdict: 0 = consistent, 1 = underdetermined/warnings,
//! 2 = conflicts (or a parse/compile error). This makes it usable as a CI gate.

use std::io::Read;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use elenchus_compiler::FileResolver;
use elenchus_solver::{Report, verify, verify_source};

#[derive(Parser)]
#[command(
    name = "elenchus",
    version,
    about = "Check an elenchus .vrf program for logical consistency.",
    long_about = "Reads a .vrf program (a file, inline --text, or stdin), runs the \
reasoning engine, and prints the verdict. With a file, IMPORTs are resolved \
relative to it. Exit code: 0 consistent, 1 underdetermined/warnings, 2 conflicts."
)]
struct Cli {
    /// A `.vrf` file to check. Omit (or pass `-`) to read from stdin.
    file: Option<String>,

    /// Inline program text instead of a file or stdin.
    #[arg(long, conflicts_with = "file")]
    text: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// Human-readable report.
    Human,
    /// Single-line JSON (for tooling / agents).
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let report = match build_report(&cli) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("elenchus: {e}");
            return ExitCode::from(2);
        }
    };
    match cli.format {
        Format::Human => println!("{report}"),
        Format::Json => println!("{}", report.to_json()),
    }
    ExitCode::from(report.exit_code() as u8)
}

fn build_report(cli: &Cli) -> Result<Report, String> {
    if let Some(text) = &cli.text {
        return verify_source("<text>", text).map_err(|e| e.to_string());
    }
    match cli.file.as_deref() {
        // A real file: resolve IMPORTs relative to it.
        Some(path) if path != "-" => verify(path, &FileResolver).map_err(|e| e.to_string()),
        // stdin (no file, or `-`): a single source; IMPORTs are not resolved.
        _ => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("reading stdin: {e}"))?;
            verify_source("<stdin>", &buf).map_err(|e| e.to_string())
        }
    }
}
