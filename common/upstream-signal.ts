import { MILLISECONDS_PER_SECOND, OAUTH_UPSTREAM_TIMEOUT_SECONDS } from "/config";

/**
 * Signal for OAuth/IdP and discovery upstream calls: aborts after
 * `OAUTH_UPSTREAM_TIMEOUT_SECONDS`, composed with an optional caller signal
 * (typically the panel request or the gateway client disconnect).
 */
export function oauthUpstreamSignal(signal?: AbortSignal): AbortSignal {
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
