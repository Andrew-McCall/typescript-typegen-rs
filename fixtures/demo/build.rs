use ts_typegen_build::{Config, Preset};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Config::new()
        .scan("src")
        .output("bindings")
        .preset(Preset::Common)
        .index(true)
        .run()
}
