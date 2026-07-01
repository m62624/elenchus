//! Data-only sources: read a `.vrf` of `PROVIDE` values into port bindings.
use crate::error::CompileError;
use crate::ir::PortBinding;
use crate::resolver::parse_tagged;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use elenchus_parser::Statement;

/// Parse a data-only `.vrf` source and extract its `PROVIDE` bindings as
/// `(name, value)` pairs. A data file carries only values: any statement other
/// than `PROVIDE` (or a `DOMAIN`) is a [`CompileError::DataFileStatement`]. Used to
/// load a `--data <file>` of port values without compiling it as a program.
pub fn read_data_source(file: &str, src: &str) -> Result<Vec<(String, bool)>, CompileError> {
    let program = parse_tagged(file, src)?;
    let mut out = Vec::new();
    for stmt in &program.statements {
        match stmt {
            Statement::Provide { atom, value } => {
                // Serialize the target into a key string (`[domain.]subject[
                // predicate[ object]]`) that `parse_port_ref` re-parses uniformly,
                // so a data file and a `--set` key share one resolution path.
                let a = &atom.data;
                let mut key = String::new();
                if let Some(d) = a.domain {
                    key.push_str(d);
                    key.push('.');
                }
                key.push_str(a.subject);
                if let Some(p) = a.predicate {
                    key.push(' ');
                    key.push_str(p);
                }
                if let Some(o) = a.object {
                    key.push(' ');
                    key.push_str(o);
                }
                out.push((key, *value));
            }
            Statement::Domain(_) => {}
            other => {
                return Err(CompileError::DataFileStatement {
                    file: file.to_string(),
                    line: statement_line(other),
                });
            }
        }
    }
    Ok(out)
}

/// Parse a data-only `.vrf` source into ready-to-merge port [`PortBinding`]s, each
/// tagged with origin `data:<file>`. The shared bridge every surface uses to turn a
/// `--data` / data-map source into engine inputs, so a data file behaves identically
/// whether it arrives from the CLI, wasm, or MCP.
pub fn read_data_bindings(
    file: &str,
    src: &str,
) -> Result<Vec<(String, PortBinding)>, CompileError> {
    Ok(read_data_source(file, src)?
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                PortBinding {
                    value,
                    origin: alloc::format!("data:{file}"),
                },
            )
        })
        .collect())
}

/// The 1-based source line a statement begins on (for diagnostics).
fn statement_line(s: &Statement) -> u32 {
    match s {
        Statement::Domain(n) => n.span.location_line(),
        Statement::Import { path, .. } => path.span.location_line(),
        Statement::Fact { atom, .. } => atom.span.location_line(),
        Statement::Negation(a) => a.span.location_line(),
        Statement::Assume(l) => l.span.location_line(),
        Statement::Set { name, .. } => name.span.location_line(),
        Statement::Close { relation, .. } => relation.span.location_line(),
        Statement::Var { name, .. } => name.span.location_line(),
        Statement::Provide { atom, .. } => atom.span.location_line(),
        Statement::Premise { name, .. } | Statement::Rule { name, .. } => name.span.location_line(),
        Statement::Check { subject, .. } => subject
            .as_ref()
            .map(|s| s.span.location_line())
            .unwrap_or(0),
    }
}
