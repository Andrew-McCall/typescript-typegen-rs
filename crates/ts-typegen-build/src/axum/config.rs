//! Configuration for axum route discovery and TypeScript client generation.

use std::collections::HashMap;

/// How route discovery finds handlers to generate clients for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Discover {
    /// Only handlers carrying a registered route attribute (e.g. `ts_route`)
    /// are discovered.
    #[default]
    Annotated,
    /// Every handler reachable from a configured router is discovered, with
    /// no attribute required.
    All,
}

/// The role an extractor argument plays in a handler's TypeScript signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extractor {
    /// Contributes nothing to the generated client call.
    Ignore,
    /// Supplies the request body.
    Body,
    /// Supplies query-string parameters.
    Query,
    /// Supplies path parameters.
    Path,
}

/// How a handler parameter typed as a collection renders in a query string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryArrays {
    /// Emits a diagnostic warning instead of guessing an encoding.
    #[default]
    Warn,
    /// Comma-joins values into a single query parameter.
    Csv,
    /// Repeats the key once per value, e.g. `?id=1&id=2`.
    Repeated,
}

/// Whether and how a TypeScript client module is generated alongside the
/// type bindings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Client {
    /// No client is generated.
    #[default]
    None,
    /// Every route function is emitted into a single flat module.
    Flat,
    /// Routes are split into one module per router.
    Modules {
        /// Prefix removed from each router's name before using it as a
        /// module name.
        strip: Option<String>,
    },
}

/// A route-declaring attribute that route discovery recognizes, and the key
/// names it uses for the path, method, request, and response type.
#[derive(Debug, Clone)]
pub struct RouteAttr {
    name: String,
    path_key: String,
    method_key: String,
    request_key: String,
    response_key: String,
}

impl RouteAttr {
    /// A route attribute named `name`, with the default lowercase key names.
    pub fn new(name: impl Into<String>) -> Self {
        RouteAttr {
            name: name.into(),
            path_key: "path".to_string(),
            method_key: "method".to_string(),
            request_key: "request".to_string(),
            response_key: "response".to_string(),
        }
    }

    /// The built-in attribute registered on every [`Axum`] by default.
    pub fn ts_route() -> Self {
        Self::new("ts_route")
    }

    /// Overrides the key that carries the route path.
    pub fn path_key(mut self, key: impl Into<String>) -> Self {
        self.path_key = key.into();
        self
    }

    /// Overrides the key that carries the HTTP method.
    pub fn method_key(mut self, key: impl Into<String>) -> Self {
        self.method_key = key.into();
        self
    }

    /// Overrides the key that carries the request body type.
    pub fn request_key(mut self, key: impl Into<String>) -> Self {
        self.request_key = key.into();
        self
    }

    /// Overrides the key that carries the response type.
    pub fn response_key(mut self, key: impl Into<String>) -> Self {
        self.response_key = key.into();
        self
    }

    /// The attribute's name, e.g. `ts_route`.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Configured key name for the route path.
    pub fn path_key_name(&self) -> &str {
        &self.path_key
    }
    /// Configured key name for the HTTP method.
    pub fn method_key_name(&self) -> &str {
        &self.method_key
    }
    /// Configured key name for the request body type.
    pub fn request_key_name(&self) -> &str {
        &self.request_key
    }
    /// Configured key name for the response type.
    pub fn response_key_name(&self) -> &str {
        &self.response_key
    }
}

/// Builder for the axum section of a [`crate::Config`]. Construct with
/// [`Axum::new`], chain setters, then pass to [`crate::Config::axum`].
#[derive(Debug, Clone)]
pub struct Axum {
    route_attrs: Vec<RouteAttr>,
    discover: Discover,
    routers: bool,
    strict_routes: bool,
    prefix: Option<String>,
    prefix_exempt: Vec<String>,
    extractors: HashMap<String, Extractor>,
    response_map: HashMap<String, String>,
    error_type: Option<String>,
    query_arrays: QueryArrays,
    client: Client,
}

impl Default for Axum {
    fn default() -> Self {
        Axum {
            route_attrs: vec![RouteAttr::ts_route()],
            discover: Discover::default(),
            routers: false,
            strict_routes: false,
            prefix: None,
            prefix_exempt: Vec::new(),
            extractors: HashMap::new(),
            response_map: HashMap::new(),
            error_type: None,
            query_arrays: QueryArrays::default(),
            client: Client::default(),
        }
    }
}

const BUILT_IN: &[(&str, Extractor)] = &[
    ("Path", Extractor::Path),
    ("Query", Extractor::Query),
    ("Json", Extractor::Body),
    ("Form", Extractor::Body),
    ("Multipart", Extractor::Body),
    ("Bytes", Extractor::Body),
    ("Body", Extractor::Body),
    ("String", Extractor::Body),
    ("State", Extractor::Ignore),
    ("Extension", Extractor::Ignore),
    ("HeaderMap", Extractor::Ignore),
    ("ConnectInfo", Extractor::Ignore),
    ("Host", Extractor::Ignore),
    ("Method", Extractor::Ignore),
    ("Uri", Extractor::Ignore),
    ("Request", Extractor::Ignore),
];

impl Axum {
    /// Starts a new axum section with every option at its default: annotated
    /// discovery, no router scanning, and `ts_route` as the only registered
    /// route attribute.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an additional attribute route discovery recognizes,
    /// alongside the built-in `ts_route`.
    pub fn route_attr(mut self, attr: RouteAttr) -> Self {
        self.route_attrs.push(attr);
        self
    }

    /// How route discovery finds handlers.
    pub fn discover(mut self, d: Discover) -> Self {
        self.discover = d;
        self
    }

    /// Whether to additionally scan configured routers for handlers not
    /// reachable through a registered route attribute.
    pub fn routers(mut self, on: bool) -> Self {
        self.routers = on;
        self
    }

    /// Whether every discovered route's path must resolve unambiguously.
    pub fn strict_routes(mut self, on: bool) -> Self {
        self.strict_routes = on;
        self
    }

    /// Prefix prepended to every discovered route's path, except paths
    /// matching [`Axum::prefix_exempt`].
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Path patterns exempt from [`Axum::prefix`]: either an exact path, or
    /// ending in a single trailing `*` to match a prefix. Extends, rather
    /// than replaces, any patterns already configured.
    pub fn prefix_exempt(mut self, patterns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.prefix_exempt.extend(patterns.into_iter().map(Into::into));
        self
    }

    /// Overrides the extractor role inferred for a type's final path
    /// segment, taking precedence over the built-in table.
    pub fn extractor(mut self, last_segment: impl Into<String>, e: Extractor) -> Self {
        self.extractors.insert(last_segment.into(), e);
        self
    }

    /// Overrides the TypeScript rendering of a handler's response type,
    /// matched on its final path segment.
    pub fn map_response(mut self, last_segment: impl Into<String>, ts: impl Into<String>) -> Self {
        self.response_map.insert(last_segment.into(), ts.into());
        self
    }

    /// The Rust type a handler's fallible response error variant is mapped
    /// to in the generated client.
    pub fn error_type(mut self, name: impl Into<String>) -> Self {
        self.error_type = Some(name.into());
        self
    }

    /// How array-valued query parameters render.
    pub fn query_arrays(mut self, q: QueryArrays) -> Self {
        self.query_arrays = q;
        self
    }

    /// Whether and how a TypeScript client module is generated.
    pub fn client(mut self, c: Client) -> Self {
        self.client = c;
        self
    }

    /// The extractor role for a type's final path segment: a configured entry
    /// first, then the built-in table.
    pub fn extractor_for(&self, last_segment: &str) -> Option<Extractor> {
        if let Some(e) = self.extractors.get(last_segment) {
            return Some(*e);
        }
        BUILT_IN.iter().find(|(n, _)| *n == last_segment).map(|(_, e)| *e)
    }

    /// The prefix to prepend to `declared_path`, or `None` when it is exempt or
    /// no prefix is configured.
    pub fn prefix_for(&self, declared_path: &str) -> Option<&str> {
        let prefix = self.prefix.as_deref()?;
        if self.prefix_exempt.iter().any(|p| crate::axum::path::matches_exempt(declared_path, p)) {
            return None;
        }
        Some(prefix)
    }

    // Readers. Named apart from the setters above, which already own the
    // obvious names.

    /// Configured route discovery mode.
    pub fn discovery(&self) -> Discover {
        self.discover
    }
    /// Whether configured routers are additionally scanned for handlers.
    pub fn routers_enabled(&self) -> bool {
        self.routers
    }
    /// Whether every discovered route's path must resolve unambiguously.
    pub fn strict_routes_enabled(&self) -> bool {
        self.strict_routes
    }
    /// Configured style for array-valued query parameters.
    pub fn query_arrays_style(&self) -> QueryArrays {
        self.query_arrays
    }
    /// Whether and how a TypeScript client module is generated.
    pub fn client_style(&self) -> &Client {
        &self.client
    }
    /// The registered route attributes, `ts_route` included.
    pub fn route_attrs(&self) -> &[RouteAttr] {
        &self.route_attrs
    }
    /// The configured error type name, if one was set.
    pub fn error_type_name(&self) -> Option<&str> {
        self.error_type.as_deref()
    }
    /// The TypeScript override for a response type's final path segment, if
    /// one was configured.
    pub fn response_map_for(&self, last_segment: &str) -> Option<&str> {
        self.response_map.get(last_segment).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_annotated_discovery_with_no_router_scan() {
        let a = Axum::new();
        assert_eq!(a.discovery(), Discover::Annotated);
        assert!(!a.routers_enabled());
        assert!(!a.strict_routes_enabled());
        assert_eq!(a.query_arrays_style(), QueryArrays::Warn);
    }

    #[test]
    fn ts_route_is_the_built_in_attribute_with_lowercase_keys() {
        let a = RouteAttr::ts_route();
        assert_eq!(a.name(), "ts_route");
        assert_eq!(a.path_key_name(), "path");
        assert_eq!(a.method_key_name(), "method");
        assert_eq!(a.response_key_name(), "response");
    }

    #[test]
    fn a_foreign_attribute_can_rename_every_key() {
        let a = RouteAttr::new("api")
            .path_key("Route")
            .method_key("Method")
            .request_key("Request")
            .response_key("Response");
        assert_eq!(a.name(), "api");
        assert_eq!(a.path_key_name(), "Route");
        assert_eq!(a.request_key_name(), "Request");
    }

    #[test]
    fn a_prefix_applies_except_to_exempt_paths() {
        let a = Axum::new().prefix("v2").prefix_exempt(["/auth/*", "/monitor"]);
        assert_eq!(a.prefix_for("/patient/view/{id}"), Some("v2"));
        assert_eq!(a.prefix_for("/auth/refresh"), None);
        assert_eq!(a.prefix_for("/monitor"), None);
    }

    #[test]
    fn built_in_extractors_resolve_and_config_extends_them() {
        let a = Axum::new().extractor("AuthSession", Extractor::Ignore);
        assert_eq!(a.extractor_for("Json"), Some(Extractor::Body));
        assert_eq!(a.extractor_for("State"), Some(Extractor::Ignore));
        assert_eq!(a.extractor_for("AuthSession"), Some(Extractor::Ignore));
        assert_eq!(a.extractor_for("MysteryWrapper"), None);
    }

    #[test]
    fn config_carries_an_axum_section_only_when_set() {
        assert!(crate::Config::new().axum_config().is_none());
        assert!(crate::Config::new().axum(Axum::new()).axum_config().is_some());
    }
}
