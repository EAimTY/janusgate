//! The management API route contract, the `route!` macro that declares every
//! route in one place, and — under the `management-api-typescript-bindings`
//! feature — the distributed registry the TypeScript bindings generator
//! consumes. The server router reads the same [`Route`] constants, so a
//! route's method, path, and payload types are written exactly once.

/// The contract of one management API route: the HTTP method and sub-router
/// path it is served at, and the concrete wire payload types it exchanges.
/// Every request and response payload is a named type unique across the
/// management API; a route that carries no request body declares an empty
/// request struct. Routes are declared through the crate's `route!` macro,
/// never by hand.
pub trait Route {
    /// HTTP method the route answers.
    const METHOD: http::Method;

    /// Path within the `/management-api` sub-router.
    const PATH: &'static str;

    /// Request body type; an empty struct when the route carries no request
    /// body.
    type Request;

    /// Success response body type.
    type Response;
}

#[cfg(feature = "management-api-typescript-bindings")]
use std::{any::TypeId, collections::HashSet};
#[cfg(feature = "management-api-typescript-bindings")]
use ts_rs::{Config, Dependency, ExportError, TS, TypeVisitor};

/// ts-rs-backed description of one request or response payload type, produced
/// lazily because `TypeId` and TypeScript identifiers are not computable in a
/// static initializer.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) struct PayloadBinding {
    /// Identity and TypeScript identifier of the payload type itself.
    pub(crate) root: (TypeId, String),

    /// The payload type plus every type it transitively references, each as
    /// its identity and TypeScript identifier; the walk mirrors the one
    /// ts-rs's exporter performs, so a file is written for every entry.
    pub(crate) closure: Vec<(TypeId, String)>,

    /// Recursively exports the payload type and its dependencies into the
    /// generator's output directory.
    pub(crate) export_all: fn(&Config) -> Result<(), ExportError>,
}

/// One route's registration in [`MANAGEMENT_API_ROUTES`], holding function
/// pointers because a distributed-slice entry must be a static initializer
/// and none of the ts-rs values are const-computable.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) struct RouteBinding {
    /// HTTP method the route answers.
    pub(crate) method: fn() -> http::Method,

    /// Path within the `/management-api` sub-router.
    pub(crate) path: &'static str,

    /// Request payload binding.
    pub(crate) request: fn(&Config) -> PayloadBinding,

    /// Response payload binding.
    pub(crate) response: fn(&Config) -> PayloadBinding,
}

/// Every declared management API route, collected link-time; iteration order
/// is unspecified, so the generator sorts before rendering.
#[cfg(feature = "management-api-typescript-bindings")]
#[linkme::distributed_slice]
pub(super) static MANAGEMENT_API_ROUTES: [RouteBinding];

/// Provides the route's method as an owned value.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn route_method<R: Route>() -> http::Method {
    R::METHOD.clone()
}

/// Builds the request payload binding for one route.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn request_binding<R: Route>(config: &Config) -> PayloadBinding
where
    R::Request: TS + 'static,
{
    payload_binding::<R::Request>(config)
}

/// Builds the response payload binding for one route.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn response_binding<R: Route>(config: &Config) -> PayloadBinding
where
    R::Response: TS + 'static,
{
    payload_binding::<R::Response>(config)
}

/// Builds the ts-rs-backed description of one payload type.
#[cfg(feature = "management-api-typescript-bindings")]
fn payload_binding<T: TS + 'static>(config: &Config) -> PayloadBinding {
    PayloadBinding {
        root: (TypeId::of::<T>(), T::ident(config)),
        closure: type_closure::<T>(config),
        export_all: T::export_all,
    }
}

/// Collects `T` and every transitively referenced exportable type, as
/// identities paired with TypeScript identifiers.
#[cfg(feature = "management-api-typescript-bindings")]
fn type_closure<T: TS + 'static>(config: &Config) -> Vec<(TypeId, String)> {
    struct Walk<'a> {
        config: &'a Config,
        seen: HashSet<TypeId>,
        visited: Vec<(TypeId, String)>,
    }

    impl TypeVisitor for Walk<'_> {
        fn visit<T: TS + 'static + ?Sized>(&mut self) {
            // Inline-only types such as primitives carry no file or name of
            // their own, so they cannot participate in a name collision.
            if Dependency::from_ty::<T>(self.config).is_none()
                || !self.seen.insert(TypeId::of::<T>())
            {
                return;
            }

            self.visited
                .push((TypeId::of::<T>(), T::ident(self.config)));

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

/// Declares one management API route: a marker type carrying its [`Route`]
/// implementation and — under the `management-api-typescript-bindings`
/// feature — its registration in [`MANAGEMENT_API_ROUTES`]. The marker type
/// is generated, exists only at the type level, and is named by `$route`.
macro_rules! route {
    ($vis:vis impl Route for $route:ident {
        const METHOD: http::Method = $method:expr;
        const PATH: &'static str = $path:expr;
        type Request = $request:ty;
        type Response = $response:ty;
    }) => {
        #[doc = concat!(
            "Route marker for `",
            $path,
            "`; the type exists only at the type level, carrying the route's",
            " `Route` implementation, and is never constructed.",
        )]
        $vis struct $route {}

        #[doc = concat!("Registers the `", $path, "` route in the bindings registry.")]
        #[expect(
            non_upper_case_globals,
            reason = "the binding static is named after its route marker type"
        )]
        #[cfg(feature = "management-api-typescript-bindings")]
        #[linkme::distributed_slice($crate::management_api::route::MANAGEMENT_API_ROUTES)]
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

            #[doc = concat!("Path `", $path, "` within the `/management-api` sub-router.")]
            const PATH: &'static str = $path;

            #[doc = "Request body type; empty when the route carries no body."]
            type Request = $request;

            #[doc = "Success response body type."]
            type Response = $response;
        }
    };
}

pub(super) use route;
