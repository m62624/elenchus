//! Every model- and user-facing string the server emits, gathered in one place
//! so the wording can be read and tuned in isolation from the protocol plumbing.
//! `crate::rpc` and `crate::tools` pull their text from here — nothing
//! user-visible is written inline next to the JSON.

/// The MCP protocol version this server speaks (reported at `initialize`).
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// `serverInfo.name` reported at `initialize`.
pub const SERVER_NAME: &str = "elenchus";

/// `elenchus_check` tool description — what the tool does and how to read it.
pub const CHECK_TOOL: &str = "Check an elenchus `.vrf` program (facts, premises, rules, checks) for \
logical consistency. Returns one of CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT with \
details and an exit code. Treat WARNING, UNDERDETERMINED and CONFLICT as NOT done: add the \
missing facts or rethink the premises, then call again — iterate until the result is CONSISTENT.";

/// `elenchus_check` — description of the `program` argument (entry mode 1).
pub const CHECK_ARG_PROGRAM: &str = "Inline .vrf program text: a leading DOMAIN, FACT/NOT assertions, \
PREMISE/RULE first principles, and a CHECK. This is the entry source; for IMPORTs, send the imported \
sources inline via `files`. Give either `program` or `path`, not both.";

/// `elenchus_check` — description of the `path` argument (entry mode 2: read from
/// the filesystem, like the CLI).
pub const CHECK_ARG_PATH: &str = "Filesystem path to a .vrf file to check. The server reads it — and \
resolves its `IMPORT`s — directly from disk (no `files` needed), exactly like `elenchus-cli <file>`. \
Use this only when the server runs locally with filesystem access; a remote server cannot see your \
files, so prefer inline `program` (+ `files`) for portability. Give either `program` or `path`, not both.";

/// `elenchus_check` — description of the optional `format` argument.
pub const CHECK_ARG_FORMAT: &str = "Output format. Default \"json\".";

/// `elenchus_check` — description of the optional `max_classes` argument.
pub const CHECK_ARG_MAX_CLASSES: &str = "On a syntax error, show at most this many error classes \
(one class per keyword; 0 or omitted = all). Only affects parse-error output.";

/// `elenchus_check` — description of the optional `max_per_class` argument.
pub const CHECK_ARG_MAX_PER_CLASS: &str = "On a syntax error, show at most this many places within \
each class (0 or omitted = all). Only affects parse-error output.";

/// `elenchus_check` — description of the optional `values` argument.
pub const CHECK_ARG_VALUES: &str = "External values for VAR ports, as an object of \
{ \"portName\": true|false }. Each named port must be declared with `VAR <name>` in the program; \
a port set to two different values is an error. Qualify a key with a `domain.` prefix \
(\"self.has_vision\") to pick one of several imported domains that declare the same port name, \
or name a multi-word atom (\"engine has_fuel\") to assert it directly.";

/// `elenchus_check` — description of the optional `files` argument (the in-memory
/// import set, which is how the resolver-less MCP server resolves IMPORT).
pub const CHECK_ARG_FILES: &str = "Extra sources for IMPORT resolution, as an object of \
{ \"path\": \"<.vrf text>\" }. `program` is the entry file; its `IMPORT \"path\"` statements load \
the matching keys here. This is how multi-domain templates work over MCP (which has no filesystem) \
— send every imported file's text inline. Paths are matched after normalizing `/`, `\\`, `.` and \
`..`, so a Windows- or Unix-style import resolves the same way.";

/// `elenchus_check` — description of the optional `data` argument.
pub const CHECK_ARG_DATA: &str = "VAR port values supplied from data files, as an object of \
{ \"name\": \"<PROVIDE-only .vrf text>\" }. Each value is read like a CLI `--data` file: it may \
contain only `PROVIDE <port>: true|false` lines. Equivalent to `values`, just carried as file text; \
a port set to two different values (across `values`/`data`/the program) is an error.";

/// `elenchus_check` — description of the optional `data_paths` argument (the
/// filesystem counterpart of `data`, mirroring how `path` relates to `program`).
pub const CHECK_ARG_DATA_PATHS: &str = "VAR port values from PROVIDE-only .vrf files on disk, as an \
array of filesystem paths. The server reads each — like a CLI `--data <file>` — so each file may \
contain only `PROVIDE <port>: true|false` lines. Use this only on a locally-run server with \
filesystem access; a remote server cannot see your files, so prefer inline `data` for portability. \
Merged with `values`/`data`; a port set to two different values is an error.";

/// `elenchus_version` tool description — the MCP analog of `elenchus --version`.
pub const VERSION_TOOL: &str = "Return the running elenchus engine version (e.g. \"elenchus 0.3.0\"). \
Call this once up front and compare it to the version your skill targets; if they differ, \
warn the user about the version skew before relying on the other tools.";

/// Shared by the `elenchus_about` tool's *description* and its *returned text*,
/// so the two never drift. Deliberately version-free (`elenchus_version` owns the
/// number) and harness-agnostic (no product names).
pub const ABOUT_TOOL: &str = "elenchus checks whether a set of facts and first principles is \
logically consistent (a small three-valued SAT checker). You are calling it over MCP, so \
you are an AI agent: you'll get markedly better results with the matching `elenchus` skill \
loaded — it carries the verdict loop (iterate until CONSISTENT), the DSL, and worked \
examples this server expects. The skill is attached to every release; grab the one matching \
the engine version (call `elenchus_version`) from the releases page, or ask the user to \
install it: https://github.com/m62624/elenchus/releases";
