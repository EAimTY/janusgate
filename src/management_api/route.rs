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
pub trait Route {
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

/// One ordered vocabulary a version's bindings publish besides their route payloads: a closed
/// unit-only enum whose variant order is itself a published fact, such as the canonical thinking
/// levels' order of increasing reasoning strength. Instantiated as `ordered_vocabulary::<T>`.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) type OrderedVocabulary = fn(&Config) -> ClosureType;

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

/// Builds one ordered vocabulary's replacement declaration: the enum as a [`ClosureType`] like any
/// payload's, whose declaration renders the variant-ordered const tuple plus the union derived from
/// the tuple instead of ts-rs's union, so the Rust enum stays the single source of truth for both
/// membership and order.
///
/// # Panics
///
/// Panics naming the type when its ts-rs declaration is not a unit-only enum's union; see
/// [`union_members`].
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) fn ordered_vocabulary<T>(config: &Config) -> ClosureType
where
    T: TS + 'static,
{
    ClosureType {
        type_id: TypeId::of::<T>(),
        ident: T::ident(config),
        declaration: ordered_declaration::<T>(),
    }
}

/// Builds the renderer of one ordered vocabulary's declaration: the doc comment on the ordered
/// const tuple carrying the vocabulary's canonical order, then the same doc comment again on the
/// union derived from the tuple, so hovering either artifact in a client shows the vocabulary's
/// documentation.
#[cfg(feature = "management-api-typescript-bindings")]
fn ordered_declaration<T>() -> fn(&Config) -> String
where
    T: TS + 'static + ?Sized,
{
    fn render<T>(config: &Config) -> String
    where
        T: TS + 'static + ?Sized,
    {
        let order_ident = order_const_ident(&T::ident(config));

        let order = union_members::<T>(config)
            .iter()
            .map(|member| format!("\"{member}\""))
            .collect::<Vec<_>>()
            .join(", ");

        let docs = T::docs().unwrap_or_default();

        format!(
            "{docs}export const {order_ident} = [{order}] as const;\n\n\
             {docs}export type {} = (typeof {order_ident})[number];",
            T::ident(config)
        )
    }

    render::<T>
}

/// Derives the TypeScript identifier of one vocabulary's ordered const tuple from its type
/// identifier: CamelCase becomes SCREAMING_SNAKE_CASE suffixed with `ORDER` — `ThinkingLevel`
/// renders `THINKING_LEVEL_ORDER`.
#[cfg(feature = "management-api-typescript-bindings")]
fn order_const_ident(ident: &str) -> String {
    let mut rendered = String::with_capacity(ident.len() + "_ORDER".len());

    for (index, character) in ident.chars().enumerate() {
        if index > 0 && character.is_ascii_uppercase() {
            rendered.push('_');
        }

        rendered.push(character.to_ascii_uppercase());
    }

    rendered.push_str("_ORDER");

    rendered
}

/// Extracts a unit-only enum's wire names, in variant order, from its ts-rs inline rendering — `"a"
/// | "b"`, the union bare of any declaration wrapper; the union's member order is the variant
/// order, so the tuple built from it carries the enum's canonical order.
///
/// # Panics
///
/// Panics naming the type when the inline rendering is not a union of quoted string literals — a
/// struct, a tagged union, or a `repr(enum)` taxonomy defines no variant-ordered vocabulary to
/// publish; an ordered vocabulary is a unit-only enum by contract.
#[cfg(feature = "management-api-typescript-bindings")]
#[expect(
    clippy::panic,
    reason = "a vocabulary type that is not a unit-only enum union is a fatal declaration bug"
)]
fn union_members<T>(config: &Config) -> Vec<String>
where
    T: TS + 'static + ?Sized,
{
    let inline = T::inline(config);
    let ident = T::ident(config);

    let members = inline
        .split(" | ")
        .map(|member| {
            member
                .strip_prefix('"')
                .and_then(|member| member.strip_suffix('"'))
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>();

    let Some(members) = members else {
        panic!(
            "ordered vocabulary `{ident}` is not a unit-only enum union; \
             its ts-rs inline rendering is `{inline}`"
        );
    };

    members
}

/// Declares one management API route: a marker type carrying its [`Route`] implementation and —
/// under the `management-api-typescript-bindings` feature — its registration in `$registry`, the
/// declaring version's route slice. The marker type is generated, exists only at the type level,
/// and is named by `$route`.
///
/// A version wraps this macro to bind its own registry, so a route declaration names neither the
/// registry nor the version.
macro_rules! route {
    ($registry:path; $vis:vis impl Route for $route:ident {
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
}

pub(super) use route;
