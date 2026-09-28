import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { hrefs } from "/src/router";
import { AppLink } from "/src/ui/app-link";
import { proseLinkClass } from "/src/ui/attributes";
import { Notice } from "/src/ui/feedback";

/**
 * The vanished model's suspension notice (README §6.3): the warn notice the
 * model detail and edit pages state under the header when the upstream no
 * longer reports the row, with the two recovery paths linked where the
 * sentence names them — discovery again on the owning provider's page, and
 * the claim-back onto the Create model page with the provider and upstream
 * ID carried along as the prefill. The wording and both link targets live
 * here alone, so the two pages cannot drift apart.
 */
export function VanishedModelNotice(props: {
    /** Whether the mounted row's discovery state is `vanished`. */
    readonly isVanished: boolean;
    /** The vanished row's owning provider, whose page runs discovery again. */
    readonly providerId: number;
    /** The vanished row's upstream ID, carried onto the claim-back prefill. */
    readonly upstreamId: string;
}): JSX.Element {
    return (
        <Show when={props.isVanished}>
            <Notice
                tone="warn"
                title="The upstream no longer reports this model."
                detail={
                    <>
                        It serves nothing meanwhile.{" "}
                        <AppLink
                            href={hrefs.providerDetail(props.providerId)}
                            class={proseLinkClass}
                        >
                            Discover again on the provider's page
                        </AppLink>
                        {" when the upstream lists it, or "}
                        <AppLink
                            href={hrefs.modelNew(props.providerId, props.upstreamId)}
                            class={proseLinkClass}
                        >
                            claim it back from the Create model page
                        </AppLink>
                        {"."}
                    </>
                }
            />
        </Show>
    );
}
