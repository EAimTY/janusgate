//! The management API route contract, the `route!` macro that declares every route in one place,
//! and — under the `management-api-typescript-bindings` feature — the binding machinery the
//! TypeScript bindings generator consumes.
//!
//! Nothing here belongs to a version: a version module owns its route registry and wraps `route!`
//! to bind that registry, while this module owns the contract and the expansion both share. The
//! version routers read the same [`Route`] constants, so a route's method, path, and payload types
//! are written exactly once.

use http::Method;
#[cfg(feature = "management-api-typescript-bindings")]
use std::{any, any::TypeId, collections::HashSet};
#[cfg(feature = "management-api-typescript-bindings")]
use ts_rs::{Config, Dependency, TS, TypeVisitor};

/// The contract of one management API route: the HTTP method and path it is served at, and the
/// concrete wire payload types it exchanges. Every request and response payload is a named type
/// unique across its version; a route that carries no request body declares an empty request
/// struct. Routes are declared through their version's `route!` macro, never by hand.
pub(super) trait Route {
    /// HTTP method the route answers.
    const METHOD: Method;

    /// Path within the version's sub-router, such as `/providers:list`; the version prefix is not
    /// part of it.
    const PATH: &'static str;

    /// Request body type; an empty struct when the route carries no request body.
    type Request;

    /// Success response body type.
    type Response;
}

/// One type of a payload's transitive closure as the bindings generator sees it: identity,
/// TypeScript identifier, and the renderer of its declaration.
#[cfg(feature = "management-api-typescript-bindings")]
#[derive(Clone)]
pub(super) struct ClosureType {
    /// Identity of the Rust type.
    pub(crate) type_id: TypeId,

    /// TypeScript identifier of the type.
    pub(crate) ident: String,

    /// Renders the type's declaration — doc comment included — as it appears in the version's
    /// single bindings file.
    pub(crate) declaration: fn(&Config) -> String,
}

/// ts-rs-backed description of one request or response payload type, produced lazily because
/// `TypeId` and TypeScript identifiers are not computable in a static initializer.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) struct PayloadBinding {
    /// Identity and TypeScript identifier of the payload type itself.
    pub(crate) root: (TypeId, String),

    /// The payload type plus every type it transitively references; the walk mirrors the one
    /// ts-rs's exporter performs, so every entry's declaration is rendered into the version's
    /// bindings file.
    pub(crate) closure: Vec<ClosureType>,
}

/// One route's registration in its version's registry, holding function pointers because a
/// distributed-slice entry must be a static initializer and none of the ts-rs values are
/// const-computable.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) struct RouteBinding {
    /// HTTP method the route answers.
    pub(crate) method: fn() -> Method,

    /// Path within its version's sub-router.
    pub(crate) path: &'static str,

    /// Request payload binding.
    pub(crate) request: fn(&Config) -> PayloadBinding,

    /// Response payload binding.
    pub(crate) response: fn(&Config) -> PayloadBinding,
}

/// One payload type a version's bindings export besides its route payloads, such as its error body;
/// instantiated as `payload_binding::<T>`.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) type PayloadSource = fn(&Config) -> PayloadBinding;

/// Provides the route's method as an owned value.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn route_method<R>() -> Method
where
    R: Route,
{
    R::METHOD.clone()
}

/// Builds the request payload binding for one route.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn request_binding<R>(config: &Config) -> PayloadBinding
where
    R: Route,
    R::Request: TS + 'static,
{
    payload_binding::<R::Request>(config)
}

/// Builds the response payload binding for one route.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn response_binding<R>(config: &Config) -> PayloadBinding
where
    R: Route,
    R::Response: TS + 'static,
{
    payload_binding::<R::Response>(config)
}

/// Builds the ts-rs-backed description of one payload type.
///
/// # Panics
///
/// Panics when the type carries no TypeScript declaration of its own, such as `()`; every route
/// payload is a declared concrete type, so such a type is a fatal route-declaration bug.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn payload_binding<T>(config: &Config) -> PayloadBinding
where
    T: TS + 'static,
{
    require_declared_payload::<T>(config);

    PayloadBinding {
        root: (TypeId::of::<T>(), T::ident(config)),
        closure: type_closure::<T>(config),
    }
}

/// Requires a payload type to be declarable: a named concrete type ts-rs renders a declaration for,
/// an empty struct included. An inline-only type — `()` above all, which every route contract
/// forbids as a payload — has no declaration and could only be referenced by the route table as an
/// inline TypeScript spelling, so it is rejected here instead of rendering broken bindings.
///
/// # Panics
///
/// Panics naming the Rust type when ts-rs reports no declaration for it.
#[cfg(feature = "management-api-typescript-bindings")]
#[expect(
    clippy::panic,
    reason = "an undeclarable payload type is a fatal route-declaration bug"
)]
fn require_declared_payload<T>(config: &Config)
where
    T: TS + 'static,
{
    if Dependency::from_ty::<T>(config).is_none() {
        panic!(
            "management payload type `{}` has no TypeScript declaration; \
             every route declares a concrete request and response type",
            any::type_name::<T>(),
        );
    }
}

/// Collects `T` and every transitively referenced exportable type, as identities paired with
/// TypeScript identifiers and declaration renderers.
#[cfg(feature = "management-api-typescript-bindings")]
fn type_closure<T>(config: &Config) -> Vec<ClosureType>
where
    T: TS + 'static,
{
    struct Walk<'a> {
        config: &'a Config,
        seen: HashSet<TypeId>,
        visited: Vec<ClosureType>,
    }

    impl TypeVisitor for Walk<'_> {
        fn visit<T>(&mut self)
        where
            T: TS + 'static + ?Sized,
        {
            // Inline-only types such as primitives carry no declaration of their own, so they
            // cannot participate in a name collision.
            if Dependency::from_ty::<T>(self.config).is_none()
                || !self.seen.insert(TypeId::of::<T>())
            {
                return;
            }

            // ts-rs's serde-json feature implements `serde_json::Value` through a private shadow
            // type whose TypeScript name is the same `JsonValue` the value type itself carries;
            // visiting the shadow would collide with the value it shadows, so it is skipped — the
            // value type's entry renders the shared declaration.
            if T::ident(self.config) == "JsonValue"
                && TypeId::of::<T>() != TypeId::of::<serde_json::Value>()
            {
                return;
            }

            self.visited.push(ClosureType {
                type_id: TypeId::of::<T>(),
                ident: T::ident(self.config),
                declaration: declaration::<T>(),
            });

            T::visit_dependencies(self);
        }
    }

    let mut walk = Walk {
        config,
        seen: HashSet::new(),
        visited: Vec::new(),
    };

    walk.visit::<T>();

    walk.visited
}

/// Builds the renderer of one closure type's declaration: the doc comment followed by the
/// `export`-prefixed declaration, exactly as ts-rs writes it into a per-type file, minus the
/// per-file scaffolding — note header and imports — that the version's single bindings file
/// replaces.
#[cfg(feature = "management-api-typescript-bindings")]
fn declaration<T>() -> fn(&Config) -> String
where
    T: TS + 'static + ?Sized,
{
    fn render<T>(config: &Config) -> String
    where
        T: TS + 'static + ?Sized,
    {
        let mut rendered = T::docs().unwrap_or_default();

        rendered.push_str("export ");
        rendered.push_str(&T::decl(config));

        rendered
    }

    render::<T>
}

/// Declares one management API route: a marker type carrying its [`Route`] implementation and —
/// under the `management-api-typescript-bindings` feature — its registration in `$registry`, the
/// declaring version's route slice. The marker type is generated, exists only at the type level,
/// and is named by `$route`.
///
/// A version wraps this macro to bind its own registry, so a route declaration names neither the
/// registry nor the version. The declaration names `path`, `method`, `request`, and `response`
/// in that order, each exactly once, with a trailing comma; `method` accepts any method
/// expression, so both `http::Method::POST` and an imported `Method::POST` spell the same route,
/// while `path` accepts a string literal or a constant. Anything else fails with
/// `compile_error!` showing the expected shape.
macro_rules! route {
    // Declares the route whose `path` is a literal, so the documentation embeds its value.
    (
        $registry:path ;
        $vis:vis $route:ident : {
            path : $path:literal ,
            method : $method:expr ,
            request : $request:ty ,
            response : $response:ty ,
        }
    ) => {
        #[doc = concat!(
            "Route marker for `",
            $path,
            "`; the type exists only at the type level, carrying the route's",
            " `Route` implementation, and is never constructed.",
        )]
        $vis struct $route {}

        #[doc = concat!("Registers the `", $path, "` route in the version's registry.")]
        #[expect(
            non_upper_case_globals,
            reason = "the binding static is named after its route marker type"
        )]
        #[cfg(feature = "management-api-typescript-bindings")]
        #[linkme::distributed_slice($registry)]
        static $route: $crate::management_api::route::RouteBinding =
            $crate::management_api::route::RouteBinding {
                method: $crate::management_api::route::route_method::<$route>,
                path: <$route as $crate::management_api::route::Route>::PATH,
                request: $crate::management_api::route::request_binding::<$route>,
                response: $crate::management_api::route::response_binding::<$route>,
            };

        impl $crate::management_api::route::Route for $route {
            #[doc = "HTTP method the route answers."]
            const METHOD: ::http::Method = $method;

            #[doc = concat!("Path `", $path, "` within the version's sub-router.")]
            const PATH: &'static str = $path;

            #[doc = "Request body type; empty when the route carries no body."]
            type Request = $request;

            #[doc = "Success response body type."]
            type Response = $response;
        }
    };

    // Declares the route whose `path` is a constant or other non-literal expression; the
    // documentation names the constant, whose value is not computable at expansion time.
    (
        $registry:path ;
        $vis:vis $route:ident : {
            path : $path:expr ,
            method : $method:expr ,
            request : $request:ty ,
            response : $response:ty ,
        }
    ) => {
        #[doc = concat!(
            "Route marker for `",
            stringify!($path),
            "`; the type exists only at the type level, carrying the route's",
            " `Route` implementation, and is never constructed.",
        )]
        $vis struct $route {}

        #[doc = concat!(
            "Registers the `",
            stringify!($path),
            "` route in the version's registry."
        )]
        #[expect(
            non_upper_case_globals,
            reason = "the binding static is named after its route marker type"
        )]
        #[cfg(feature = "management-api-typescript-bindings")]
        #[linkme::distributed_slice($registry)]
        static $route: $crate::management_api::route::RouteBinding =
            $crate::management_api::route::RouteBinding {
                method: $crate::management_api::route::route_method::<$route>,
                path: <$route as $crate::management_api::route::Route>::PATH,
                request: $crate::management_api::route::request_binding::<$route>,
                response: $crate::management_api::route::response_binding::<$route>,
            };

        impl $crate::management_api::route::Route for $route {
            #[doc = "HTTP method the route answers."]
            const METHOD: ::http::Method = $method;

            #[doc = concat!("Path `", stringify!($path), "` within the version's sub-router.")]
            const PATH: &'static str = $path;

            #[doc = "Request body type; empty when the route carries no body."]
            type Request = $request;

            #[doc = "Success response body type."]
            type Response = $response;
        }
    };

    // Anything else, including a misordered declaration, with the expected shape.
    ($($junk:tt)*) => {
        compile_error!(concat!(
            "invalid route! declaration; expected `Name: { path, method, ",
            "request, response }` with a trailing comma",
        ));
    };
}

pub(super) use route;
