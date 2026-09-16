import type { JSX } from "@solidjs/web";

/**
 * Placeholder application shell. Real screens (connection setup, providers,
 * models) are not implemented yet; this only proves the toolchain renders.
 */
export function App(): JSX.Element {
    return (
        <main class="grid min-h-screen place-content-center p-10 font-sans">
            <h1 class="m-0 text-3xl font-bold tracking-tight">janusgate-web</h1>
            <p class="mt-2 max-w-xl text-sm">
                Standalone web frontend for the JanusGate management API. The scaffold builds;
                business screens are not implemented yet.
            </p>
        </main>
    );
}
