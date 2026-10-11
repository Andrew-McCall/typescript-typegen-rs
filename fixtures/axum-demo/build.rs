use ts_typegen_build::axum::config::{Axum, Client, Discover};
use ts_typegen_build::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Config::new()
        .scan("src")
        .output("bindings")
        .index(false)
        .axum(Axum::new().discover(Discover::All).routers(true).client(Client::Flat))
        .run()
}
