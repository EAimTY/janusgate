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

#[cfg(test)]
mod tests {
    use super::{ClosureType, CollectedRoute};
    use crate::management_api::{route as route_contract, v0, v0::route};
    use http::Method;
    use std::{any::TypeId, collections::HashMap, env, fs, path::PathBuf, process};
    use ts_rs::Config;

    route! {
        BindingsTestAlpha: {
            path: "/bindings-test-alpha",
            method: Method::POST,
            request: BindingsTestAlphaRequest,
            response: BindingsTestAlphaResponse,
        }
    }

    /// Path of the beta bindings-test route, held in a constant so the suite covers
    /// constant-phrased `path` declarations alongside inline literals.
    const BINDINGS_TEST_BETA_PATH: &str = "/bindings-test-beta";

    route! {
        BindingsTestBeta: {
            path: BINDINGS_TEST_BETA_PATH,
            method: http::Method::GET,
            request: BindingsTestBetaRequest,
            response: BindingsTestBetaResponse,
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
        /// Discovery state.
        #[expect(dead_code, reason = "the field shapes the generated TypeScript")]
        state: crate::provider::DiscoveryState,
    }

    /// A per-run output target, removed first so every run starts empty.
    fn fresh_output_target() -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "janusgate-management-api-bindings-{}",
            process::id()
        ));

        let _ = fs::remove_dir_all(&directory);

        directory
    }

    /// Verifies that generation is deterministic across runs, that it renders exactly one file —
    /// `v0.ts`, named after the version segment, directly in the output target — holding
    /// everything: every wire type's declaration inline (the discovery-state union, the error body
    /// with its errno taxonomy included, the empty request type's too) plus the route table,
    /// ordered by path, referencing the types declared above it with no imports. The route table
    /// keys are the routes' paths within the version, version prefix excluded — the file name
    /// is what carries the version. The generated surface carries no ordered-vocabulary artifact:
    /// `ThinkingLevel` and `THINKING_LEVEL_ORDER` are gone, closed sets publish as plain unions,
    /// and `models:delete` is part of the route table (README §7.3).
    ///
    /// # Panics
    ///
    /// Panics if either run fails, the two runs render different bytes, or the output diverges from
    /// the single-file shape.
    #[test]
    fn generates_deterministic_single_file_bindings() {
        let target = fresh_output_target();

        super::generate(&target, v0::PREFIX, &v0::ROUTES, v0::EXPORTED_TYPES);

        #[expect(clippy::expect_used, reason = "the generator just wrote the file")]
        let first =
            fs::read_to_string(target.join("v0.ts")).expect("v0.ts exists after generation");

        super::generate(&target, v0::PREFIX, &v0::ROUTES, v0::EXPORTED_TYPES);

        #[expect(clippy::expect_used, reason = "the generator just wrote the file")]
        let second =
            fs::read_to_string(target.join("v0.ts")).expect("v0.ts exists after regeneration");

        assert_eq!(first, second, "generation must be deterministic");

        #[expect(
            clippy::expect_used,
            reason = "the generator just wrote into the target"
        )]
        let mut entries: Vec<String> = fs::read_dir(&target)
            .expect("the output target is readable")
            .map(|entry| {
                #[expect(clippy::expect_used, reason = "the directory was just written")]
                let entry = entry.expect("every directory entry is readable");

                entry.file_name().to_string_lossy().into_owned()
            })
            .collect();

        entries.sort_unstable();

        assert_eq!(entries, ["v0.ts"], "one version renders exactly one file");

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

        assert!(first.contains("export type BindingsTestAlphaRequest"));
        assert!(first.contains("export type BindingsTestBetaRequest"));

        // Closed sets publish as plain unions: the discovery-state union is declared, and no
        // ordered-vocabulary artifact — the vocabulary-ordered level type or its order constant
        // — renders anywhere in the file (README §7.3). The descriptor's level-extraction shapes
        // — `ThinkingLevelsExtraction` — are payload types, not vocabulary artifacts, and publish
        // legitimately.
        assert!(first.contains("export type DiscoveryState"));
        assert!(first.contains(r#""discovered""#));
        assert!(first.contains(r#""vanished""#));
        assert!(first.contains(r#""manual""#));

        assert!(!first.contains("THINKING_LEVEL_ORDER"));
        assert!(!first.contains("ThinkingLevel "));
        assert!(!first.contains("ThinkingLevel="));
        assert!(
            !first.contains("IrReasoningEffort"),
            "the Rust type name never reaches the bindings"
        );

        // The capability-override mirrors of the model rows publish as their two-state shapes
        // (rule 2): the required-nullable array and number overrides on create and update, the
        // always-present nullable columns on the row (README §7.1).
        assert!(first.contains("thinking_levels_override: Array<string> | null"));
        assert!(first.contains("max_input_tokens_override: number | null"));
        assert!(first.contains("thinking_levels: Array<string> | null"));

        // The `models:delete` route is part of the published route table.
        assert!(first.contains("\"/models:delete\""));
        assert!(first.contains("request: DeleteModelRequest;"));
        assert!(first.contains("response: EmptyResponse;"));

        // The usage read surface ships its two routes with both payload types declared in the
        // same file (README §7.3), and its row and bucket shapes alongside them.
        assert!(first.contains("\"/usage:list\""));
        assert!(first.contains("\"/usage:query\""));
        assert!(first.contains("request: ListUsageRequest;"));
        assert!(first.contains("response: ListUsageResponse;"));
        assert!(first.contains("request: QueryUsageRequest;"));
        assert!(first.contains("response: QueryUsageResponse;"));
        assert!(first.contains("export type ApiLogResponse"));
        assert!(
            first.contains("export type UsageBucket ")
                || first.contains("export type UsageBucket =")
        );

        // The two accounting vocabularies publish as plain unions carrying exactly their closed
        // member sets — no ordering or enumeration artifact (README §7.3).
        assert!(first.contains(
            "export type ApiLogEndpointType = \"openai_chat_completions\" | \"openai_responses\" | \
             \"openai_responses_compact\" | \"gemini_generate_content\" | \
             \"gemini_interactions\" | \"anthropic_messages\";"
        ));
        assert!(first.contains(
            "export type ApiLogOutcome = \"completed\" | \"client_aborted\" | \
             \"upstream_error\" | \"internal\";"
        ));
        assert!(first.contains(
            "export type UsageGroupBy = \"day\" | \"model\" | \"model_route\" | \"provider\" | \
             \"api_key\" | \"endpoint_type\" | \"outcome\" | \"is_stream\" | \"http_status\";"
        ));

        assert!(first.contains("export type ErrorResponse"));
        assert!(first.contains("export enum ErrorCode"));
        assert!(!first.contains("import "), "a single file needs no imports");

        // The provider union is discriminated by the row's credential kind:
        // one member per kind, each carrying the shared definition fields.
        assert!(first.contains(r#"export type ProviderResponse = { "type": "oauth","#));
        assert!(first.contains(r#""type": "api_key","#));

        // The complete-provider-union route payloads carry the shared fields
        // through the flattened helper, so no member restates them.
        assert!(first.contains("provider_definition_name"));
        assert!(first.contains("provider_definition_version"));
        assert!(first.contains("format: UpstreamFormat"));

        // The union previously carried one member per provider kind; none of
        // the old kinds survives as a ProviderResponse discriminant.
        for kind in [
            "openai_response",
            "openai_chat_completion",
            "gemini_generate_content",
            "gemini_interaction",
            "anthropic_messages",
        ] {
            let restated_member = format!("{{ \"type\": \"{kind}\", ");
            assert!(
                !first.contains(&restated_member),
                "the `{kind}` variant restates its field list per kind",
            );
        }

        let _ = fs::remove_dir_all(&target);
    }

    /// Verifies that a method-and-path collision between two routes is rejected when the sorted
    /// route list is scanned.
    ///
    /// # Panics
    ///
    /// Panics if the collision is not rejected.
    #[test]
    #[should_panic(expected = "duplicate management route POST /clash")]
    fn rejects_duplicate_routes() {
        super::reject_duplicate_routes(&[
            collected("GET", "/alpha"),
            collected("POST", "/clash"),
            collected("POST", "/clash"),
        ]);
    }

    /// Verifies that one TypeScript name claimed by two distinct Rust types is rejected, while one
    /// type claiming its own name twice is not.
    ///
    /// # Panics
    ///
    /// Panics if the collision is not rejected.
    #[test]
    #[should_panic(expected = "share the TypeScript name `Clashing`")]
    fn rejects_conflicting_type_names() {
        let mut names = HashMap::new();

        super::register_type_names(&mut names, &[closure_type(TypeId::of::<u8>(), "Distinct")]);

        super::register_type_names(&mut names, &[closure_type(TypeId::of::<u8>(), "Stable")]);
        super::register_type_names(&mut names, &[closure_type(TypeId::of::<u8>(), "Stable")]);

        super::register_type_names(&mut names, &[closure_type(TypeId::of::<u16>(), "Clashing")]);

        super::register_type_names(&mut names, &[closure_type(TypeId::of::<u32>(), "Clashing")]);
    }

    /// Verifies that a payload type ts-rs cannot declare is rejected before it reaches the route
    /// table, so an inline-only type — `()` above all — can never stand in for an explicitly
    /// declared request or response payload type; an empty concrete type is declarable and
    /// therefore allowed.
    ///
    /// # Panics
    ///
    /// Panics as designed, on the type that carries no declaration.
    #[test]
    #[should_panic(expected = "has no TypeScript declaration")]
    fn rejects_undeclarable_payload_types() {
        let _ = route_contract::payload_binding::<()>(&Config::new());
    }

    /// Builds a minimal closure type for the checker tests; the declaration renderer is never
    /// invoked there.
    fn closure_type(type_id: TypeId, ident: &str) -> ClosureType {
        fn declaration(_config: &Config) -> String {
            String::new()
        }

        ClosureType {
            type_id,
            ident: ident.to_string(),
            declaration,
        }
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
