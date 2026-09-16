//! Process entry point: installs the process-wide fail-fast panic hook and
//! delegates all startup and serving to the library's `run`.

use std::{env, panic, process};
use tokio::main;

/// Application entry point.
///
/// # Panics
///
/// Panics if command-line arguments or configuration cannot be parsed or if
/// the global configuration or logger has already been initialized.
#[main]
async fn main() -> ! {
    set_panic_hook();

    janusgate::run(env::args_os()).await
}

/// Configures a custom panic hook that terminates the entire process
/// immediately.
///
/// By default, a panic on a background or worker thread in Rust only unwinds
/// that specific thread. In an asynchronous server, this leaves the remaining
/// runtime operational while services or connections may be permanently broken.
/// This hook prints diagnostic panic info to stderr via the standard hook and
/// calls `process::exit(1)` to ensure fail-fast semantics.
fn set_panic_hook() {
    let hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Print the panic location, payload, and backtrace to stderr.
        hook(panic_info);

        // Terminate the process immediately with a failure status code.
        process::exit(1);
    }));
}
