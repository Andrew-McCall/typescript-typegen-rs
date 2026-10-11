use ts_typegen_build::axum::config::{Axum, Client, RouteAttr};
use ts_typegen_build::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Config::new()
        .scan("src")
        .output("bindings")
        .index(false)
        .axum(
            Axum::new()
                .client(Client::Modules { strip: Some("handlers".to_string()) })
                .route_attr(
                    RouteAttr::new("api")
                        .path_key("Route")
                        .method_key("Method")
                        .request_key("Request")
                        .response_key("Response"),
                )
                .prefix("v2")
                .prefix_exempt(["/auth/*"])
                .map_response("FileResponse", "Blob")
                .error_type("ApiErrorBody"),
        )
        .run()
}
