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

/// `elenchus_check` — description of the required `program` argument.
pub const CHECK_ARG_PROGRAM: &str = "The .vrf program text: a leading DOMAIN, FACT/NOT assertions, PREMISE/RULE first principles, and a CHECK.";

/// `elenchus_check` — description of the optional `format` argument.
pub const CHECK_ARG_FORMAT: &str = "Output format. Default \"json\".";

/// `elenchus_check` — description of the optional `max_classes` argument.
pub const CHECK_ARG_MAX_CLASSES: &str = "On a syntax error, show at most this many error classes \
(one class per keyword; 0 or omitted = all). Only affects parse-error output.";

/// `elenchus_check` — description of the optional `max_per_class` argument.
pub const CHECK_ARG_MAX_PER_CLASS: &str = "On a syntax error, show at most this many places within \
each class (0 or omitted = all). Only affects parse-error output.";

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
examples this server expects. The skill is attached to every release; pull the one matching \
the engine version (call `elenchus_version`) from \
https://github.com/m62624/elenchus/releases/download/v<version>/SKILL.md (latest: \
https://github.com/m62624/elenchus/releases/latest/download/SKILL.md), or ask the user to \
install it from https://github.com/m62624/elenchus.";
