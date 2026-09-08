import { MILLISECONDS_PER_SECOND, OAUTH_UPSTREAM_TIMEOUT_SECONDS } from "/config";

/**
 * Signal bounding any administrative upstream call with the shared timeout — discovery, OAuth
 * flows, and token refresh alike: aborts after `OAUTH_UPSTREAM_TIMEOUT_SECONDS`, composed with
 * an optional caller signal (typically the panel request or the gateway client disconnect).
 */
export function boundedUpstreamSignal(signal?: AbortSignal): AbortSignal {
    const timeoutSignal = AbortSignal.timeout(
        OAUTH_UPSTREAM_TIMEOUT_SECONDS * MILLISECONDS_PER_SECOND,
    );
    return signal === undefined
        ? timeoutSignal
        : AbortSignal.any([
              signal,
              timeoutSignal,
          ]);
}
