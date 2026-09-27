//! Generates the management API TypeScript bindings.
//!
//! Takes the output target directory as its only argument — relative or absolute, created when
//! absent. Every version's bindings are rendered into one `<version>.ts` file directly in that
//! target — `/management-api/v0` into `v0.ts` — so one invocation covers the whole management API.
//! Run with:
//!
//! ```text
//! cargo run \
//!     --bin generate-management-api-typescript-bindings \
//!     --features management-api-typescript-bindings \
//!     -- <output-directory>
//! ```

use janusgate::generate_typescript_bindings;
use lexopt::{Arg as Argument, Parser as ArgumentsParser};
use std::path::PathBuf;

fn main() {
    let output_directory = parse_output_directory();

    generate_typescript_bindings(&output_directory);
}

/// Parses the single output-directory argument.
///
/// # Panics
///
/// Panics when the argument is missing, duplicated, or an option is passed; the invocation is
/// first-party, so a malformed one is fatal.
fn parse_output_directory() -> PathBuf {
    let mut parser = ArgumentsParser::from_env();
    let mut output_directory = None;

    #[expect(
        clippy::unwrap_used,
        reason = "command-line argument syntax errors are fatal"
    )]
    while let Some(argument) = parser.next().unwrap() {
        match argument {
            Argument::Value(value) if output_directory.is_none() => {
                output_directory = Some(PathBuf::from(value));
            }

            #[expect(clippy::panic, reason = "a malformed invocation is fatal")]
            Argument::Value(_) => {
                panic!("expected exactly one output-directory argument");
            }

            #[expect(clippy::panic, reason = "a malformed invocation is fatal")]
            unexpected => {
                panic!("unexpected argument {unexpected:?}");
            }
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "the output directory names the generator's only output"
    )]
    output_directory.expect("missing output-directory argument")
}
