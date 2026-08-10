//! The build-script-facing configuration builder.

use std::path::{Path, PathBuf};

use crate::lower::TypeMap;

/// How 64-bit and wider integers are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BigInt {
    /// Matches what `serde_json` puts on the wire. Lossy above 2^53.
    #[default]
    Number,
    /// A native TypeScript `bigint`. Only correct if your decoder actually
    /// produces one — plain `JSON.parse` never does.
    Bigint,
    /// A string, for codecs that serialize wide integers as text.
    Str,
}

/// How opaque values such as `serde_json::Value` render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnknownStyle {
    /// Forces callers to narrow before use.
    #[default]
    Unknown,
    /// Opts out of checking entirely; useful when migrating an existing codebase.
    Any,
}

/// How `Vec<u8>` and `[u8; N]` render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bytes {
    /// What plain `serde_json` emits: a JSON array of numbers.
    #[default]
    NumberArray,
    /// For codecs that encode byte arrays as text, e.g. `serde_bytes` with a
    /// base64 helper. Changes only the binding, never what serde writes.
    Str,
}

/// How an `Option<T>` field renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionalStyle {
    /// `field: T | null` — serde serializes `None` as `null` with the key present.
    #[default]
    Nullable,
    /// `field?: T`
    Question,
    /// `field?: T | null`
    Both,
}

/// How named-field structs render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StructStyle {
    /// Named-field structs emit `export interface X { ... }`.
    #[default]
    Interface,
    /// Named-field structs emit `export type X = { ... };`.
    TypeAlias,
}

/// Builder for a generation run. Construct with [`Config::new`], chain
/// setters, then call [`Config::run`] from `build.rs`.
#[derive(Debug, Clone)]
pub struct Config {
    scans: Vec<PathBuf>,
    scans_customised: bool,
    output: PathBuf,
    index: bool,
    index_name: String,
    int64: BigInt,
    int128: BigInt,
    bytes: Bytes,
    unknown: UnknownStyle,
    header: String,
    optional_style: OptionalStyle,
    struct_style: StructStyle,
    import_extension: String,
    type_map: TypeMap,
    check: bool,
    always_run: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scans: vec![PathBuf::from("src")],
            scans_customised: false,
            output: PathBuf::from("bindings"),
            index: true,
            index_name: "index.ts".to_string(),
            int64: BigInt::default(),
            int128: BigInt::default(),
            bytes: Bytes::default(),
            unknown: UnknownStyle::default(),
            header: String::new(),
            optional_style: OptionalStyle::default(),
            struct_style: StructStyle::default(),
            import_extension: String::new(),
            type_map: TypeMap::default(),
            check: std::env::var_os("TS_TYPEGEN_CHECK").is_some(),
            always_run: false,
        }
    }
}

impl Config {
    /// Starts a new config with every option at its default.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a directory to scan. The first call replaces the `"src"` default so
    /// `.scan("../types/src")` alone does not silently also scan this crate.
    pub fn scan(mut self, path: impl Into<PathBuf>) -> Self {
        if !self.scans_customised {
            self.scans.clear();
            self.scans_customised = true;
        }
        self.scans.push(path.into());
        self
    }

    /// Directory `.ts` files are written to.
    pub fn output(mut self, path: impl Into<PathBuf>) -> Self {
        self.output = path.into();
        self
    }

    /// Whether to write a barrel file re-exporting every generated type.
    pub fn index(mut self, on: bool) -> Self {
        self.index = on;
        self
    }

    /// File name for the barrel file, when [`Config::index`] is enabled.
    pub fn index_name(mut self, name: impl Into<String>) -> Self {
        self.index_name = name.into();
        self
    }

    /// Sets the style for every integer wider than 32 bits. Shorthand for
    /// calling [`Config::int64`] and [`Config::int128`] with the same value.
    pub fn bigint(mut self, b: BigInt) -> Self {
        self.int64 = b;
        self.int128 = b;
        self
    }

    /// Style for `i64`, `u64`, `isize` and `usize`.
    ///
    /// These exceed JavaScript's safe integer range only above 2^53, so many
    /// projects leave them as `number` and set only [`Config::int128`].
    pub fn int64(mut self, b: BigInt) -> Self {
        self.int64 = b;
        self
    }

    /// Style for `i128` and `u128`, which no JavaScript number can represent.
    pub fn int128(mut self, b: BigInt) -> Self {
        self.int128 = b;
        self
    }

    /// How `serde_json::Value` and other opaque types render.
    pub fn unknown_type(mut self, u: UnknownStyle) -> Self {
        self.unknown = u;
        self
    }

    /// Lines inserted after the generated-file marker in every emitted file,
    /// e.g. `/* eslint-disable */`. No trailing newline needed.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = header.into();
        self
    }

    /// How byte arrays render. The default matches serde_json; `Str` is for
    /// projects whose codec text-encodes them.
    pub fn bytes(mut self, b: Bytes) -> Self {
        self.bytes = b;
        self
    }

    /// How `Option<T>` fields render.
    pub fn optional_style(mut self, s: OptionalStyle) -> Self {
        self.optional_style = s;
        self
    }

    /// How named-field structs render.
    pub fn struct_style(mut self, s: StructStyle) -> Self {
        self.struct_style = s;
        self
    }

    /// Extension to append in import specifiers, e.g. `".js"` for ESM under
    /// `moduleResolution: node16`.
    pub fn import_extension(mut self, ext: impl Into<String>) -> Self {
        self.import_extension = ext.into();
        self
    }

    /// Overrides the TypeScript rendering of a type by Rust path, matched on
    /// the final path segment — so `"uuid::Uuid"` matches a field written as
    /// `Uuid` after a `use` too.
    pub fn map_type(mut self, rust_path: &str, ts: &str) -> Self {
        self.type_map.insert(rust_path, ts);
        self
    }

    /// Apply a set of built-in type mappings for a common foreign crate.
    /// A later `map_type` call overrides anything a preset installed.
    pub fn preset(mut self, p: crate::presets::Preset) -> Self {
        for (rust, ts) in p.expand() {
            self.type_map.insert(rust, ts);
        }
        self
    }

    /// When enabled, generate in memory and diff against what's on disk
    /// instead of writing, failing [`Config::run`] if they differ. Defaults to
    /// on when the `TS_TYPEGEN_CHECK` environment variable is set.
    pub fn check(mut self, on: bool) -> Self {
        self.check = on;
        self
    }

    /// Generate even when this package is not a cargo primary package. Off by
    /// default so a downstream consumer building from the registry does not try
    /// to write into a read-only directory.
    pub fn always_run(mut self, on: bool) -> Self {
        self.always_run = on;
        self
    }

    /// Run generation. Call this at the end of your `build.rs`.
    pub fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        for root in &self.scans {
            println!("cargo::rerun-if-changed={}", root.display());
        }
        // Watching the output directory too means deleting bindings/ regenerates
        // it: cargo always reruns when a watched path is missing. Writes are
        // idempotent, so this settles after one extra run instead of looping.
        println!("cargo::rerun-if-changed={}", self.output.display());
        println!("cargo::rerun-if-env-changed=TS_TYPEGEN_CHECK");

        match crate::run::generate(&self) {
            Ok(outcome) => {
                for line in outcome.warnings.iter().flat_map(|w| w.lines()) {
                    println!("cargo::warning=ts-typegen: {line}");
                }
                for name in &outcome.report.removed {
                    println!("cargo::warning=ts-typegen: removed stale binding {name}");
                }
                Ok(())
            }
            Err(e) => {
                for line in format!("{e}").lines() {
                    println!("cargo::warning=ts-typegen: {line}");
                }
                Err(e)
            }
        }
    }

    // Readers. Named apart from the setters above, which already own the
    // obvious names.

    /// Directories that will be scanned for `#[derive(Ts)]` types.
    pub fn scans(&self) -> &[PathBuf] {
        &self.scans
    }
    /// Directory `.ts` files will be written to.
    pub fn output_path(&self) -> &Path {
        &self.output
    }
    /// Whether a barrel file will be written.
    pub fn index_enabled(&self) -> bool {
        self.index
    }
    /// File name for the barrel file.
    pub fn index_file_name(&self) -> &str {
        &self.index_name
    }
    /// Configured style for `i64`, `u64`, `isize` and `usize`.
    pub fn int64_style(&self) -> BigInt {
        self.int64
    }
    /// Configured style for `i128` and `u128`.
    pub fn int128_style(&self) -> BigInt {
        self.int128
    }
    /// Configured style for opaque values such as `serde_json::Value`.
    pub fn unknown_style(&self) -> UnknownStyle {
        self.unknown
    }
    /// Lines inserted after the generated-file marker in every emitted file.
    pub fn header_lines(&self) -> &str {
        &self.header
    }
    /// Configured style for `Vec<u8>` and `[u8; N]`.
    pub fn bytes_style(&self) -> Bytes {
        self.bytes
    }
    /// Configured style for `Option<T>` fields.
    pub fn optional(&self) -> OptionalStyle {
        self.optional_style
    }
    /// Configured style for named-field structs.
    pub fn structs(&self) -> StructStyle {
        self.struct_style
    }
    /// Extension appended to import specifiers.
    pub fn extension(&self) -> &str {
        &self.import_extension
    }
    /// The accumulated type overrides from [`Config::map_type`] and
    /// [`Config::preset`].
    pub fn type_map(&self) -> &TypeMap {
        &self.type_map
    }
    /// Whether [`Config::run`] will check for drift instead of writing.
    pub fn is_check(&self) -> bool {
        self.check
    }
    /// Whether generation runs even when this package is not cargo's primary package.
    pub fn always_run_enabled(&self) -> bool {
        self.always_run
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_are_usable_with_no_configuration() {
        let c = Config::new();
        assert_eq!(c.scans(), &[PathBuf::from("src")]);
        assert_eq!(c.output_path(), Path::new("bindings"));
        assert!(c.index_enabled());
        assert_eq!(c.index_file_name(), "index.ts");
        assert_eq!(c.int64_style(), BigInt::Number);
        assert_eq!(c.int128_style(), BigInt::Number);
        assert_eq!(c.optional(), OptionalStyle::Nullable);
        assert_eq!(c.structs(), StructStyle::Interface);
        assert_eq!(c.extension(), "");
    }

    #[test]
    fn first_scan_replaces_default_then_appends() {
        let c = Config::new().scan("a").scan("b");
        assert_eq!(c.scans(), &[PathBuf::from("a"), PathBuf::from("b")]);
    }

    #[test]
    fn type_map_entries_are_keyed_by_final_segment() {
        let c = Config::new().map_type("uuid::Uuid", "string");
        assert_eq!(c.type_map().get("Uuid"), Some("string"));
    }

    #[test]
    fn check_mode_reads_the_environment() {
        // Explicit config wins even when the env var is absent.
        assert!(Config::new().check(true).is_check());
    }
}
