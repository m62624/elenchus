//! `elenchus` — check an elenchus `.vrf` program (string or file) and report.
//!
//! Exit code mirrors the verdict: 0 = consistent, 1 = underdetermined/warnings,
//! 2 = conflicts (or a parse/compile error). This makes it usable as a CI gate.

use std::io::Read;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueEnum};
use elenchus_compiler::{FileResolver, read_data_bindings};
use elenchus_solver::{CompileError, PortBinding, Report, verify_source_with, verify_with};

#[derive(Parser)]
#[command(
    name = "elenchus",
    version,
    about = "Check an elenchus .vrf program for logical consistency.",
    long_about = "Reads a .vrf program (a file, inline --text, or explicit stdin \
with '-'), runs the engine, and prints the verdict. With a file, \
IMPORTs are resolved relative to it. Exit code: 0 consistent, 1 \
underdetermined/warnings, 2 conflicts.\n\n\
A program is line-oriented: one statement per line (newline-separated). \
Indentation and extra spaces are cosmetic, so the readable indented form and a \
flat no-indent form parse identically.",
    // Shown at the bottom of --help and of the bare-invocation usage. Aimed at an
    // AI agent that reached this binary without the companion skill — a fallback
    // hint, harness-agnostic, no product names.
    after_help = "FOR AI AGENTS: you'll get markedly better results with the matching \
`elenchus` skill loaded (it carries the workflow, the verdict loop, and examples this \
binary expects). Check that you have it and that its version matches `elenchus \
--version`. The skill is attached to every release; grab the one for your version from \
https://github.com/m62624/elenchus/releases"
)]
struct Cli {
    /// A `.vrf` file to check, or `-` to read from stdin.
    file: Option<String>,

    /// Inline program text instead of a file or stdin.
    #[arg(long, conflicts_with = "file")]
    text: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// On a syntax error, show at most this many error *classes* (one class per
    /// keyword). 0 = all. The rest are summarised as a `… and N more classes`
    /// footer.
    #[arg(long, default_value_t = 0)]
    max_classes: usize,

    /// On a syntax error, show at most this many *places* within each class.
    /// 0 = all. The rest are summarised as a `… and N more <keyword> problems`
    /// line, so a class with hundreds of hits does not flood the output.
    #[arg(long, default_value_t = 0)]
    max_per_class: usize,

    /// Supply external values for `VAR` ports, as a space-separated string of
    /// `name:true|false` pairs, e.g. `--set "db_ready:true deploy_ok:false"`.
    /// Repeatable; all pairs are merged (a key set twice to different values is an
    /// error).
    #[arg(long)]
    set: Vec<String>,

    /// A data file of `PROVIDE <name>: true|false` lines, supplying port values.
    /// Repeatable. A key set to different values by two sources (any `--data` or
    /// `--set`) is an error.
    #[arg(long)]
    data: Vec<String>,

    /// Hide the PLACEHOLDERS section from the human report (print only the verdict,
    /// as before ports existed). The JSON form always includes it.
    #[arg(long)]
    hide_params: bool,
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
    if cli.text.is_none() && cli.file.is_none() {
        eprintln!("elenchus: no input provided; pass a file, --text, or - for stdin\n");
        let mut cmd = Cli::command();
        let _ = cmd.print_help();
        eprintln!();
        return ExitCode::from(2);
    }

    let report = match build_report(&cli) {
        Ok(r) => r,
        Err(e) => {
            print_error(&e, cli.max_classes, cli.max_per_class);
            return ExitCode::from(2);
        }
    };
    match cli.format {
        Format::Human => println!("{}", report.render_human(!cli.hide_params)),
        Format::Json => println!("{}", report.to_json()),
    }
    ExitCode::from(report.exit_code() as u8)
}

/// A failure before a verdict could be produced: either a compile/parse error
/// (which we render specially) or plain I/O / usage text.
enum CliError {
    Compile(CompileError),
    Other(String),
}

/// Print a pre-verdict error to stderr. Syntax errors get the grouped
/// diagnostic blocks (capped by `--max-classes` / `--max-per-class`); everything
/// else stays a one-liner.
fn print_error(e: &CliError, max_classes: usize, max_per_class: usize) {
    match e {
        CliError::Compile(CompileError::Parse(diag)) => {
            let classes = (max_classes > 0).then_some(max_classes);
            let per_class = (max_per_class > 0).then_some(max_per_class);
            eprintln!("{}", diag.render(classes, per_class));
        }
        CliError::Compile(other) => eprintln!("elenchus: {other}"),
        CliError::Other(msg) => eprintln!("elenchus: {msg}"),
    }
}

fn build_report(cli: &Cli) -> Result<Report, CliError> {
    let mut inputs = parse_set(&cli.set)?;
    inputs.extend(load_data_files(&cli.data)?);
    if let Some(text) = &cli.text {
        return verify_source_with("<text>", text, &inputs).map_err(CliError::Compile);
    }
    match cli.file.as_deref() {
        Some(path) => {
            if path == "-" {
                // Explicit stdin (`-`): a single source; IMPORTs are not resolved.
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| CliError::Other(format!("reading stdin: {e}")))?;
                verify_source_with("<stdin>", &buf, &inputs).map_err(CliError::Compile)
            } else {
                // A real file: resolve IMPORTs relative to it.
                verify_with(path, &FileResolver, &inputs).map_err(CliError::Compile)
            }
        }
        None => Err(CliError::Other(
            "no input provided; pass a file, --text, or - for stdin".to_string(),
        )),
    }
}

/// Parse the `--set` strings into `(name, binding)` pairs. Each string is a
/// whitespace-separated list of `name:true|false` tokens (so one `--set
/// "a:true b:false"` and two `--set a:true --set b:false` are equivalent). A
/// malformed token is a usage error (exit 2). Duplicate/conflicting keys are
/// detected later, by the compiler, so both origins can be named.
fn parse_set(values: &[String]) -> Result<Vec<(String, PortBinding)>, CliError> {
    let mut out = Vec::new();
    for chunk in values {
        for tok in chunk.split_whitespace() {
            let (name, val) = tok.split_once(':').ok_or_else(|| {
                CliError::Other(format!(
                    "bad --set token `{tok}` — expected name:true|false"
                ))
            })?;
            let value = match val {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(CliError::Other(format!(
                        "bad --set value in `{tok}` — expected true or false"
                    )));
                }
            };
            out.push((
                name.to_string(),
                PortBinding {
                    value,
                    origin: "CLI".to_string(),
                },
            ));
        }
    }
    Ok(out)
}

/// Read each `--data <file>` of `PROVIDE` lines into `(name, binding)` pairs,
/// tagged `data:<file>`. A read error is a usage error; a non-`PROVIDE` statement
/// surfaces as a compile error (exit 2).
fn load_data_files(paths: &[String]) -> Result<Vec<(String, PortBinding)>, CliError> {
    let mut out = Vec::new();
    for path in paths {
        let src = std::fs::read_to_string(path)
            .map_err(|e| CliError::Other(format!("reading data file {path}: {e}")))?;
        // `read_data_bindings` is the shared bridge (origin `data:<path>`) used by
        // every surface, so a `--data` file resolves identically here, in wasm, and
        // in MCP.
        out.extend(read_data_bindings(path, &src).map_err(CliError::Compile)?);
    }
    Ok(out)
}
