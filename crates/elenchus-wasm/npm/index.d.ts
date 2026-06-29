// Public TypeScript surface for the assembled npm package. The wasm-pack output
// ships its own `elenchus.d.ts` (snake_case, engine-level); this is the curated
// Node-facing API the package exposes as `types`.

/** Output format for the verdict. Defaults to `"json"`. */
export type Format = "json" | "human";

/** External `VAR` port values: `{ portName: true | false }`. A key set to two
 * different values across sources is a hard error. */
export type Values = Record<string, boolean>;

/** `VAR` port values carried as data-file text: `{ name: "<PROVIDE-only .vrf>" }`.
 * Each value is parsed like a CLI `--data` file (only `PROVIDE` lines allowed). */
export type Data = Record<string, string>;

/**
 * Check an inline `.vrf` program. `IMPORT` is NOT resolved here — use
 * {@link checkWithResolver} or {@link checkFileWithImports} for multi-file
 * programs. `maxClasses` / `maxPerClass` cap the grouped output on a syntax
 * error (0 / omitted = no cap). `values` and `data` supply `VAR` port values.
 */
export function check(
  program: string,
  format?: Format,
  maxClasses?: number,
  maxPerClass?: number,
  values?: Values,
  data?: Data,
): string;

/**
 * Check a `.vrf` program, resolving every `IMPORT` through a synchronous
 * `read(path) => string` callback (throw to signal "not found"). `root` is the
 * entry path passed to `read` first.
 */
export function checkWithResolver(
  root: string,
  read: (path: string) => string,
  format?: Format,
  maxClasses?: number,
  maxPerClass?: number,
  values?: Values,
  data?: Data,
): string;

/** Read a single `.vrf` file (Node) and check it (no IMPORT resolution). */
export function checkFile(
  file: string,
  format?: Format,
  maxClasses?: number,
  maxPerClass?: number,
  values?: Values,
  data?: Data,
): string;

/** Check a `.vrf` file (Node), resolving IMPORTs through the filesystem. */
export function checkFileWithImports(
  entry: string,
  format?: Format,
  maxClasses?: number,
  maxPerClass?: number,
  values?: Values,
  data?: Data,
): string;

/** The running engine version, e.g. `"elenchus 0.9.1"` (engine, not package). */
export function version(): string;

/** A short pointer to the companion skill. */
export function about(): string;

/** The full companion skill text (`SKILL.md`). */
export function skill(): string;

/** The skill's version marker — the engine version the bundled skill targets. */
export function skillVersion(): string;
