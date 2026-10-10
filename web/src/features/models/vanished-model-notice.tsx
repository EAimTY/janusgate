import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { translation } from "/src/i18n";
import { vanishedModelMessage } from "/src/i18n/rich";
import { hrefs } from "/src/router";
import { Notice } from "/src/ui/feedback";

/**
 * The vanished model's suspension notice (`web/docs/architecture.md`): the warn notice the
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
                title={translation()(
                    ($) => $.screens.vanishedModelNotice.theUpstreamNoLongerReportsThisModelMessage,
                )}
                detail={vanishedModelMessage(
                    hrefs.providerDetail(props.providerId),
                    hrefs.modelNew(props.providerId, props.upstreamId),
                )}
            />
        </Show>
    );
}
