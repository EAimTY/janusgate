//! The management API TypeScript bindings generator, active only under the
//! `management-api-typescript-bindings` feature. It renders one version's whole wire surface —
//! every payload type declaration ts-rs produces for the version's routes and extra exported types,
//! then the route table composed from the version's registry — into one `<version>.ts` file
//! directly in the output target, so a client imports a version from a single module and never
//! chases per-type files. ts-rs owns every type declaration; the generator collects and orders
//! them without parsing or rewriting their TypeScript.
//!
//! The generator is version-agnostic: the caller names the version prefix and hands over that
//! version's route registry, so every version's bindings are produced by the same code while their
//! contents stay entirely separate.

use super::route::{ClosureType, PayloadBinding, PayloadSource, RouteBinding};
use std::{any::TypeId, collections::HashMap, fs, path::Path};
use ts_rs::Config;

/// One route as the generator sees it, after collection from the registry.
struct CollectedRoute {
    /// HTTP method name, such as `POST`.
    method: String,

    /// Path within the version's sub-router.
    path: &'static str,

    /// TypeScript identifier of the request payload type.
    request_ident: String,

    /// TypeScript identifier of the response payload type.
    response_ident: String,
}

/// Generates one version's TypeScript bindings as one file directly in `output_target`, which may
/// be relative or absolute and is created when absent. The file is named after the version segment
/// of `version_prefix` — `/v0` renders `v0.ts` — so every version's bindings land side by side
/// under one shared target. The output is deterministic — routes are ordered by path and method,
/// type declarations by name.
///
/// `routes` supplies the version's route payloads and `exported_types` the payload types it
/// publishes besides them such as its error body. Both are this version's alone: name collisions
/// and duplicate routes are rejected per version.
///
/// # Panics
///
/// Panics when `routes` holds two routes sharing one method and path, when two distinct payload
/// types of the version share one TypeScript name, or when the filesystem fails; the generator is
/// a first-party build tool and every failure is a fixable declaration or environment bug.
pub(super) fn generate(
    output_target: &Path,
    version_prefix: &str,
    routes: &[RouteBinding],
    exported_types: &[PayloadSource],
) {
    let output_file = output_target.join(format!("{}.ts", version_prefix.trim_start_matches('/')));

    #[expect(
        clippy::expect_used,
        reason = "an uncreatable output directory is a fatal generator failure"
    )]
    fs::create_dir_all(output_target).expect("creating the output directory must succeed");

    // Management payloads ride JSON, whose numbers are IEEE-754 doubles, and every management
    // identifier stays far below the 2^53 precision boundary — so 64-bit identifiers render as
    // `number`, not `bigint`.
    let config = Config::new().with_large_int("number");

    let mut collected = Vec::new();
    let mut declarations: HashMap<TypeId, ClosureType> = HashMap::new();
    let mut names: HashMap<String, TypeId> = HashMap::new();

    for source in exported_types {
        collect_payload(&(source)(&config), &mut names, &mut declarations);
    }

    for binding in routes {
        let request = (binding.request)(&config);
        let response = (binding.response)(&config);

        collect_payload(&request, &mut names, &mut declarations);
        collect_payload(&response, &mut names, &mut declarations);

        collected.push(CollectedRoute {
            method: (binding.method)().as_str().to_owned(),
            path: binding.path,
            request_ident: request.root.1,
            response_ident: response.root.1,
        });
    }

    collected.sort_by(|previous, next| {
        (previous.path, &previous.method).cmp(&(next.path, &next.method))
    });

    reject_duplicate_routes(&collected);

    // Registry iteration order is unspecified, so the declarations are deduplicated into a map and
    // ordered by name before rendering; the same input always renders the same bytes.
    let mut ordered: Vec<ClosureType> = declarations.into_values().collect();
    ordered.sort_by(|previous, next| previous.ident.cmp(&next.ident));

    let rendered = render_version_file(&config, &ordered, &collected);

    #[expect(
        clippy::expect_used,
        reason = "an unwritable bindings file is a fatal generator failure"
    )]
    fs::write(output_file, rendered).expect("writing the bindings file must succeed");
}

/// Registers one payload type's closure — the type plus every type it transitively references — in
/// the version's declaration set, exactly once per Rust type, rejecting TypeScript name collisions
/// along the way.
fn collect_payload(
    payload: &PayloadBinding,
    names: &mut HashMap<String, TypeId>,
    declarations: &mut HashMap<TypeId, ClosureType>,
) {
    register_type_names(names, &payload.closure);

    for closure_type in &payload.closure {
        declarations
            .entry(closure_type.type_id)
            .or_insert_with(|| closure_type.clone());
    }
}

/// Records every closure type's TypeScript name, rejecting a name claimed by two distinct Rust
/// types.
///
/// # Panics
///
/// Panics naming the collision when it occurs; management payload type names are unique by
/// contract, so a collision is a declaration bug.
#[expect(
    clippy::panic,
    reason = "a type-name collision is a fatal route-declaration bug"
)]
fn register_type_names(names: &mut HashMap<String, TypeId>, closure: &[ClosureType]) {
    for closure_type in closure {
        if let Some(existing) = names.get(&closure_type.ident)
            && *existing != closure_type.type_id
        {
            panic!(
                "two distinct management payload types share the TypeScript name `{}`",
                closure_type.ident
            );
        }

        names.insert(closure_type.ident.clone(), closure_type.type_id);
    }
}

/// Rejects two routes sharing one method and path.
///
/// # Panics
///
/// Panics naming the collision when it occurs; the `route!` declarations are first-party, so a
/// duplicate is a declaration bug.
#[expect(
    clippy::panic,
    reason = "a duplicate route is a fatal route-declaration bug"
)]
fn reject_duplicate_routes(routes: &[CollectedRoute]) {
    for window in routes.windows(2) {
        let [previous, current] = window else {
            continue;
        };

        if previous.path == current.path && previous.method == current.method {
            panic!(
                "duplicate management route {} {}",
                current.method, current.path
            );
        }
    }
}

/// Renders one version's whole bindings file: the generated-by header, the payload type
/// declarations ordered by name, and the route table over them.
fn render_version_file(
    config: &Config,
    declarations: &[ClosureType],
    routes: &[CollectedRoute],
) -> String {
    let mut rendered = String::from(concat!(
        "// Generated by `cargo run --bin generate-management-api-typescript-bindings \\\n",
        "//     --features management-api-typescript-bindings -- <output-directory>`.\\\n",
        "// Do not edit.\n",
    ));

    for closure_type in declarations {
        rendered.push('\n');
        rendered.push_str(&(closure_type.declaration)(config));
        rendered.push('\n');
    }

    rendered.push('\n');
    rendered.push_str(&render_route_table(routes));
    rendered.push('\n');

    rendered
}

/// Renders the route table: the `Routes` object grouped by path, and the lookup helpers over it.
/// Every payload type the table references is declared earlier in the same file, so no imports are
/// needed.
fn render_route_table(routes: &[CollectedRoute]) -> String {
    let mut rendered = String::from("export type Routes = {\n");

    let mut previous_path: Option<&str> = None;

    for route in routes {
        if previous_path != Some(route.path) {
            if previous_path.is_some() {
                rendered.push_str("  };\n");
            }

            rendered.push_str(&format!("  \"{}\": {{\n", route.path));
            previous_path = Some(route.path);
        }

        rendered.push_str(&format!("    {}: {{\n", route.method));
        rendered.push_str(&format!("      request: {};\n", route.request_ident));
        rendered.push_str(&format!("      response: {};\n", route.response_ident));
        rendered.push_str("    };\n");
    }

    rendered.push_str("  };\n};\n");

    // Through generic indexed accesses TypeScript cannot resolve `Routes[P][M]["request"]` (TS2536
    // under strict), so the lookup helpers extract the fields through conditional `infer` instead.
    rendered.push_str(concat!(
        "\n",
        "export type Path = keyof Routes;\n",
        "\n",
        "export type Method<P extends Path> = keyof Routes[P];\n",
        "\n",
        "export type Request<P extends Path, M extends Method<P>> =\n",
        "  Routes[P][M] extends { request: infer Request } ? Request : never;\n",
        "\n",
        "export type Response<P extends Path, M extends Method<P>> =\n",
        "  Routes[P][M] extends { response: infer Response } ? Response : never;",
    ));

    rendered
}
