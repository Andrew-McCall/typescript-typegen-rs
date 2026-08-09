//! The build-script-facing configuration builder.

use std::path::{Path, PathBuf};

use crate::lower::TypeMap;

/// How 64-bit and wider integers are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BigInt {
    /// Matches what `serde_json` puts on the wire. Lossy above 2^53.
    #[default]
    Number,
    Bigint,
    Str,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StructStyle {
    /// Named-field structs emit `export interface X { ... }`.
    #[default]
    Interface,
    /// Named-field structs emit `export type X = { ... };`.
    TypeAlias,
}

#[derive(Debug, Clone)]
pub struct Config {
    scans: Vec<PathBuf>,
    scans_customised: bool,
    output: PathBuf,
    index: bool,
    index_name: String,
    bigint: BigInt,
    bytes: Bytes,
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
            bigint: BigInt::default(),
            bytes: Bytes::default(),
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

    pub fn output(mut self, path: impl Into<PathBuf>) -> Self {
        self.output = path.into();
        self
    }

    pub fn index(mut self, on: bool) -> Self {
        self.index = on;
        self
    }

    pub fn index_name(mut self, name: impl Into<String>) -> Self {
        self.index_name = name.into();
        self
    }

    pub fn bigint(mut self, b: BigInt) -> Self {
        self.bigint = b;
        self
    }

    /// How byte arrays render. The default matches serde_json; `Str` is for
    /// projects whose codec text-encodes them.
    pub fn bytes(mut self, b: Bytes) -> Self {
        self.bytes = b;
        self
    }

    pub fn optional_style(mut self, s: OptionalStyle) -> Self {
        self.optional_style = s;
        self
    }

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
    pub fn scans(&self) -> &[PathBuf] {
        &self.scans
    }
    pub fn output_path(&self) -> &Path {
        &self.output
    }
    pub fn index_enabled(&self) -> bool {
        self.index
    }
    pub fn index_file_name(&self) -> &str {
        &self.index_name
    }
    pub fn bigint_style(&self) -> BigInt {
        self.bigint
    }
    pub fn bytes_style(&self) -> Bytes {
        self.bytes
    }
    pub fn optional(&self) -> OptionalStyle {
        self.optional_style
    }
    pub fn structs(&self) -> StructStyle {
        self.struct_style
    }
    pub fn extension(&self) -> &str {
        &self.import_extension
    }
    pub fn type_map(&self) -> &TypeMap {
        &self.type_map
    }
    pub fn is_check(&self) -> bool {
        self.check
    }
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
        assert_eq!(c.bigint_style(), BigInt::Number);
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
