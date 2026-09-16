//! The management API TypeScript bindings generator, active only under the
//! `management-api-typescript-bindings` feature. It renders one `.ts` file
//! per wire payload type — ts-rs owns the Rust-to-TypeScript conversion and
//! the transitive dependency exports — and composes the `routes.ts` route
//! table from the [`MANAGEMENT_API_ROUTES`](super::route::MANAGEMENT_API_ROUTES)
//! registry.

use super::route::MANAGEMENT_API_ROUTES;
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
use ts_rs::Config;

/// One route as the generator sees it, after collection from the registry.
struct CollectedRoute {
    /// HTTP method name, such as `POST`.
    method: String,

    /// Path within the `/management-api` sub-router.
    path: &'static str,

    /// TypeScript identifier of the request payload type.
    request_ident: String,

    /// TypeScript identifier of the response payload type.
    response_ident: String,
}

/// Generates the management API TypeScript bindings into `out_dir`, which
/// may be relative or absolute and is created when absent: one `.ts` file
/// per wire type, rendered by ts-rs, plus the `routes.ts` route table. The
/// output is deterministic — routes are ordered by path and method, types by
/// name.
///
/// # Panics
///
/// Panics when two routes share one method and path, when two distinct
/// payload types share one TypeScript name, or when ts-rs or the filesystem
/// fail; the generator is a first-party build tool and every failure is a
/// fixable declaration or environment bug.
pub fn generate_typescript_bindings(out_dir: &Path) {
    #[expect(
        clippy::expect_used,
        reason = "an uncreatable bindings directory is a fatal generator failure"
    )]
    fs::create_dir_all(out_dir).expect("creating the bindings directory must succeed");

    // Management payloads ride JSON, whose numbers are IEEE-754 doubles, and
    // every management identifier stays far below the 2^53 precision
    // boundary — so 64-bit identifiers render as `number`, not `bigint`.
    let config = Config::new().with_large_int("number").with_out_dir(out_dir);

    let mut routes = Vec::new();
    let mut exported = HashSet::new();
    let mut names: HashMap<String, TypeId> = HashMap::new();

    for binding in MANAGEMENT_API_ROUTES.iter() {
        let request = (binding.request)(&config);
        let response = (binding.response)(&config);

        for payload in [&request, &response] {
            register_type_names(&mut names, &payload.closure);

            if exported.insert(payload.root.0) {
                #[expect(
                    clippy::expect_used,
                    reason = "a bindings export failure is a fatal generator failure"
                )]
                (payload.export_all)(&config)
                    .expect("exporting a management payload type must succeed");
            }
        }

        routes.push(CollectedRoute {
            method: (binding.method)().as_str().to_owned(),
            path: binding.path,
            request_ident: request.root.1,
            response_ident: response.root.1,
        });
    }

    routes.sort_by(|previous, next| {
        (previous.path, &previous.method).cmp(&(next.path, &next.method))
    });

    reject_duplicate_routes(&routes);

    let rendered = render_routes_file(&routes);

    #[expect(
        clippy::expect_used,
        reason = "an unwritable routes file is a fatal generator failure"
    )]
    fs::write(out_dir.join("routes.ts"), rendered).expect("writing the routes file must succeed");
}

/// Records every closure type's TypeScript name, rejecting a name claimed by
/// two distinct Rust types.
///
/// # Panics
///
/// Panics naming the collision when it occurs; management payload type names
/// are unique by contract, so a collision is a declaration bug.
#[expect(
    clippy::panic,
    reason = "a type-name collision is a fatal route-declaration bug"
)]
fn register_type_names(names: &mut HashMap<String, TypeId>, closure: &[(TypeId, String)]) {
    for (type_id, ident) in closure {
        if let Some(existing) = names.get(ident)
            && *existing != *type_id
        {
            panic!("two distinct management payload types share the TypeScript name `{ident}`");
        }

        names.insert(ident.clone(), *type_id);
    }
}

/// Rejects two routes sharing one method and path.
///
/// # Panics
///
/// Panics naming the collision when it occurs; the `route!` declarations are
/// first-party, so a duplicate is a declaration bug.
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

/// Renders the `routes.ts` route table: sorted imports, the route object
/// grouped by path, and the lookup helpers over it.
fn render_routes_file(routes: &[CollectedRoute]) -> String {
    let mut imports: Vec<&str> = routes
        .iter()
        .flat_map(|route| [route.request_ident.as_str(), route.response_ident.as_str()])
        .collect();
    imports.sort_unstable();
    imports.dedup();

    let mut rendered = String::from(concat!(
        "// Generated by `cargo run --bin generate-management-api-typescript-bindings \\\n",
        "//     --features management-api-typescript-bindings -- <output-directory>`.\\\n",
        "// Do not edit.\n",
        "\n",
    ));

    for ident in imports {
        rendered.push_str(&format!("import type {{ {ident} }} from \"./{ident}\";\n"));
    }

    rendered.push_str("\nexport type Routes = {\n");

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

    rendered.push_str(concat!(
        "\n",
        "export type Path = keyof Routes;\n",
        "\n",
        "export type Method<P extends Path> = keyof Routes[P];\n",
        "\n",
        "export type Request<P extends Path, M extends Method<P>> =\n",
        "  Routes[P][M][\"request\"];\n",
        "\n",
        "export type Response<P extends Path, M extends Method<P>> =\n",
        "  Routes[P][M][\"response\"];\n",
    ));

    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::management_api::route;
    use std::path::PathBuf;

    route! {
        impl Route for BindingsTestAlpha {
            const METHOD: http::Method = http::Method::POST;
            const PATH: &'static str = "/bindings-test-alpha";
            type Request = BindingsTestAlphaRequest;
            type Response = BindingsTestAlphaResponse;
        }
    }

    route! {
        impl Route for BindingsTestBeta {
            const METHOD: http::Method = http::Method::GET;
            const PATH: &'static str = "/bindings-test-beta";
            type Request = BindingsTestBetaRequest;
            type Response = BindingsTestBetaResponse;
        }
    }

    #[derive(ts_rs::TS)]
    struct BindingsTestAlphaRequest {
        /// Name.
        #[expect(dead_code, reason = "the field shapes the generated TypeScript")]
        name: String,
    }

    #[derive(ts_rs::TS)]
    struct BindingsTestAlphaResponse {
        /// Identifier.
        #[expect(dead_code, reason = "the field shapes the generated TypeScript")]
        id: i64,
    }

    #[derive(ts_rs::TS)]
    struct BindingsTestBetaRequest {}

    #[derive(ts_rs::TS)]
    struct BindingsTestBetaResponse {
        /// Levels.
        #[expect(dead_code, reason = "the field shapes the generated TypeScript")]
        levels: Vec<crate::ir::IrReasoningEffort>,
    }

    /// A per-run output directory, removed first so every run starts empty.
    fn fresh_output_directory() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "janusgate-management-api-bindings-{}",
            std::process::id()
        ));

        let _ = fs::remove_dir_all(&directory);

        directory
    }

    /// Verifies that generation is deterministic across runs, ordered by
    /// path, renders both payloads for every route — the empty request type
    /// included — and exports one file per wire type, the nested
    /// reasoning-effort enum included.
    ///
    /// # Panics
    ///
    /// Panics if either run fails or the two runs render different bytes.
    #[test]
    fn generates_deterministic_ordered_bindings() {
        let directory = fresh_output_directory();

        generate_typescript_bindings(&directory);

        #[expect(clippy::expect_used, reason = "the generator just wrote the file")]
        let first = fs::read_to_string(directory.join("routes.ts"))
            .expect("routes.ts exists after generation");

        generate_typescript_bindings(&directory);

        #[expect(clippy::expect_used, reason = "the generator just wrote the file")]
        let second = fs::read_to_string(directory.join("routes.ts"))
            .expect("routes.ts exists after regeneration");

        assert_eq!(first, second, "generation must be deterministic");

        #[expect(
            clippy::expect_used,
            reason = "the test asserts the entry's presence next"
        )]
        let alpha = first
            .find("\"/bindings-test-alpha\"")
            .expect("the alpha route entry is rendered");

        #[expect(
            clippy::expect_used,
            reason = "the test asserts the entry's presence next"
        )]
        let beta = first
            .find("\"/bindings-test-beta\"")
            .expect("the beta route entry is rendered");

        assert!(alpha < beta);

        assert!(first.contains("request: BindingsTestAlphaRequest;"));
        assert!(first.contains("response: BindingsTestAlphaResponse;"));
        assert!(first.contains("request: BindingsTestBetaRequest;"));

        assert!(directory.join("BindingsTestAlphaRequest.ts").exists());
        assert!(directory.join("BindingsTestBetaRequest.ts").exists());
        assert!(directory.join("IrReasoningEffort.ts").exists());

        let _ = fs::remove_dir_all(&directory);
    }

    /// Verifies that a method-and-path collision between two routes is
    /// rejected when the sorted route list is scanned.
    ///
    /// # Panics
    ///
    /// Panics if the collision is not rejected.
    #[test]
    #[should_panic(expected = "duplicate management route POST /clash")]
    fn rejects_duplicate_routes() {
        reject_duplicate_routes(&[
            collected("GET", "/alpha"),
            collected("POST", "/clash"),
            collected("POST", "/clash"),
        ]);
    }

    /// Verifies that one TypeScript name claimed by two distinct Rust types
    /// is rejected, while one type claiming its own name twice is not.
    ///
    /// # Panics
    ///
    /// Panics if the collision is not rejected.
    #[test]
    #[should_panic(expected = "share the TypeScript name `Clashing`")]
    fn rejects_conflicting_type_names() {
        let mut names = HashMap::new();

        register_type_names(&mut names, &[(TypeId::of::<u8>(), "Distinct".to_string())]);

        register_type_names(&mut names, &[(TypeId::of::<u8>(), "Stable".to_string())]);
        register_type_names(&mut names, &[(TypeId::of::<u8>(), "Stable".to_string())]);

        register_type_names(&mut names, &[(TypeId::of::<u16>(), "Clashing".to_string())]);

        register_type_names(&mut names, &[(TypeId::of::<u32>(), "Clashing".to_string())]);
    }

    /// Builds a minimal collected route for the checker tests.
    fn collected(method: &str, path: &'static str) -> CollectedRoute {
        CollectedRoute {
            method: method.to_string(),
            path,
            request_ident: "Request".to_string(),
            response_ident: "Response".to_string(),
        }
    }
}
