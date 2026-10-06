/** Canonical semantic message inventory; all other catalogs are checked against it. */
const english = {
    shell: {
        skipToContent: "Skip to content",
        primary: "Primary",
        refresh: "Refresh",
        disconnect: "Disconnect",
        keep: "Keep",
    },
    connection: {
        connectToJanusGate: "Connect to JanusGate",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "This app administers a JanusGate gateway: its upstream accounts, their models, and their credentials.",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "A running JanusGate instance, reachable from this browser.",
        theOriginBesideIt: "the origin beside it",
        copyThisAppsOrigin: "Copy this app's origin",
        gatewayAddress: "Gateway address",
        schemeHostAndPortOnly: "Scheme, host and port only.",
        managementToken: "Management token",
        sentOnlyToThatInstanceAndKeptOnly:
            "Sent only to that instance, and kept only in this browser.",
        cancel: "Cancel",
    },
    controls: {
        modelAccess: "Model access",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "Defaults affect existing inheriting entries. Pool access includes future eligible members. Global availability is separate from permission.",
        defaultModelAccessLabel: "Default model access:",
        defaultModelAccess: "Default model access",
        appliesToEntriesWithoutAProviderPoolOr:
            "Applies to entries without a provider, pool, or model override, including future entries.",
        searchModelAccess: "Search model access",
        publicIDOrOwnerName: "Public ID or owner name",
        overridesOnly: "Overrides only",
        removeOverride: "Remove override",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "Remove deleted references before saving, or refresh to resolve the inventory mismatch.",
        thisDefaultAppliesToFutureEntries: "This default applies to future entries.",
        noEntriesMatchThisViewHiddenOverridesAre:
            "No entries match this view. Hidden overrides are preserved.",
        name: "Name",
        key: "Key",
        skOrGenerateOne: "sk-… or Generate one",
        generate: "Generate",
        replaceIt: "Replace it",
        keepIt: "Keep it",
        theSecretHere: "the secret here",
        delete: "Delete…",
        revert: "Revert",
        unsavedChanges: "Unsaved changes",
        overrideReasoningLevels: "Override reasoning levels",
        reasoningLevels: "Reasoning levels",
        pickEveryLevelThisModelDeclaresUntickAll:
            "Pick every level this model declares. Untick all of them to declare none.",
        aPositiveIntegerClearTheSwitchToFollow:
            "A positive integer. Clear the switch to follow discovery again.",
        overrideCompactionFacts: "Override compaction facts",
        compactionMechanisms: "Compaction mechanisms",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            'State every mechanism whose facts this model replaces. Unknown states "no fact" loudly — it suppresses the discovered fact for that mechanism.',
        addAlias: "Add alias",
        modelPrefix: "Model prefix",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "Outside pools, models are callable as this prefix + upstream ID. Members use their pool prefixes. Leave empty to use upstream IDs directly. Changing the prefix changes existing automatic model names.",
        upstreamBaseURL: "Upstream base URL",
        enabled: "Enabled",
        publicRoute: "Public route",
        provider: "Provider",
        physicalModel: "Physical model",
        apiKey: "API key",
        endpoint: "Endpoint",
        outcome: "Outcome",
        streamMode: "Stream mode",
        httpStatuses: "HTTP statuses",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "Comma-separated, e.g. 200, 429. Never-answered requests match no status.",
    },
    navigation: {
        overview: "Overview",
        providers: "Providers",
        inventory: "Inventory",
        pools: "Pools",
        routes: "Public routes",
        apiKeys: "API keys",
        usage: "Usage",
        lightTheme: "Light theme — switch to dark",
        darkTheme: "Dark theme — switch to system",
        systemTheme: "System theme — switch to light",
    },
    rich: {
        connected: "Connected to [[baseUrl]] — the token lives only in this browser.",
        setupToken: "Its management token: [[setting]] in that instance’s configuration file.",
        setupCors:
            "Allow this app’s origin in that instance’s [[setting]] — unless this app is served from the gateway itself.",
    },
    titles: {
        bulkPublication: "Publish inventory",
        providerPools: "Pools",
        providerPoolNew: "Create pool",
        providerPoolEdit: "Edit pool #{{id}}",
        modelRoutes: "Public routes",
        modelRouteNew: "Create public route",
        modelRouteEdit: "Edit public route #{{id}}",
        home: "Overview",
        providers: "Providers",
        providerNew: "Create provider",
        providerDetail: "Provider #{{id}}",
        providerEdit: "Edit provider #{{id}}",
        models: "Models",
        modelNew: "Create model",
        modelDetail: "Model #{{id}}",
        modelEdit: "Edit model #{{id}}",
        apiKeys: "API keys",
        apiKeyNew: "Create API key",
        apiKeyDetail: "API key #{{id}}",
        apiKeyEdit: "Edit API key #{{id}}",
        usage: "Usage",
        usageQuery: "Usage aggregates",
        notFound: "Page not found",
    },
    format: {
        lessThanMinute: "less than one minute",
        minutes: "{{minutes}}m",
        hours: "{{hours}}h",
        hoursMinutes: "{{hours}}h {{minutes}}m",
        days: "{{days}}d",
        daysHours: "{{days}}d {{hours}}h",
        expired: "Expired {{distance}} ago — on {{instant}}.",
        valid: "Valid for {{distance}} — until {{instant}}.",
        durationSeconds: "{{seconds}}s",
        durationMinutesSeconds: "{{minutes}}m {{seconds}}s",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "API key",
            editAPIKey: "Edit API key",
            secret: "Secret",
            overview: "Overview",
            details: "Details",
            name: "Name",
            created: "Created",
            status: "Status",
            apiKeys: "API keys",
            createAPIKey: "Create API key",
            enabled: "Enabled",
            disabled: "Disabled",
        },
        apiKeyEditScreen: {
            editAPIKey: "Edit API key",
            settings: "Settings",
            rotatingMeansGenerateThenSaveMessage: "Rotating means Generate, then Save.",
            enabled: "Enabled",
            replacingTheSecretRevokesTheOldValueOnMessage:
                "Replacing the secret revokes the old value on save.",
            aKeyNeedsASecret: "A key needs a secret.",
            deleteThisKey: "Delete this key",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "Deleting revokes the key at once. Clients holding it start getting 401s. Its model access rules are removed; historical accounting remains.",
            apiKey: "API key",
            showTheCurrentKey: "Show the current key",
        },
        apiKeyNewScreen: {
            createAPIKey: "Create API key",
            oneSecretYourServingClientsShareMessage: "One secret your serving clients share.",
            apiKeysBackLabel: "API keys",
            key: "Key",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "The secret holds letters, numbers, and symbols only — no spaces — and is held by no other key.",
            enabled: "Enabled",
            loadModelAccessInventoryBeforeCreatingAKeyMessage:
                "Load model access inventory before creating a key.",
            creating: "Creating…",
        },
        apiKeysScreen: {
            apiKeys: "API keys",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "Serving keys. OpenAI clients send one as Authorization: Bearer <key>, Gemini as x-goog-api-key, and Anthropic as x-api-key; open one for its secret and usage.",
            createAPIKey: "Create API key",
            noAPIKeysYet: "No API keys yet",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "A fresh gateway serves nothing until the first key exists.",
            searchAPIKeys: "Search API keys",
            searchByNameOrKey: "Search by name or key",
            theSecretHere: "the secret here",
            on: "On",
            off: "Off",
        },
        bulkPublicationScreen: {
            createAliases: "Create aliases",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "Choose physical models and author their public IDs. Each selection creates one enabled direct route; name conflicts are reported without renaming or overwriting.",
            inventoryBackLabel: "Inventory",
            noPhysicalModelsToAlias: "No physical models to alias",
            discoverOrAddInventoryFirstMessage: "Discover or add inventory first.",
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "Another selected row uses this public ID. Choose distinct explicit names.",
            openRoute: "Open route",
            createSelectedAliases: "Create selected aliases",
            publicIDIsRequiredMessage: "Public ID is required.",
            noDirectRoutes: "No direct routes",
            modelVanishedItsNewRouteWillBeSuspended:
                "Model vanished; its new route will be suspended",
            modelEnabled: "Model enabled",
            modelDisabled: "Model disabled",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "Show the token while typing",
            probingTheGateway: "Probing the gateway…",
            connect: "Connect",
        },
        homeScreen: {
            overview: "Overview",
            theHealthOfThisGatewayAtAGlanceMessage: "The health of this gateway at a glance.",
            createProvider: "Create provider",
            providers: "Providers",
            publicRoutes: "Public routes",
            inventoryReady: "Inventory ready",
            apiKeys: "API keys",
            recentRequests24h: "Recent requests (24h)",
            recentErrors24h: "Recent errors (24h)",
            requestsPerDay: "Requests per day",
            theLast14DaysInUTCTheGatewaysMessage:
                "The last 14 days, in UTC — the gateway's own day boundary.",
            openUsage: "Open usage",
            storedSettingsReviewedMessage: "Stored settings reviewed.",
            needsAttention: "Needs attention",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "No models are available. Discover models on an enabled provider.",
            noAPIKeysYetMessage: "No API keys yet.",
            endpoints: "Endpoints",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "Every surface serves every model. Point any OpenAI-, Gemini-, or Anthropic-shaped client at this instance.",
            theURLBesideIt: "the URL beside it",
            noProvidersYet: "No providers yet",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "A provider is one upstream account. Its models arrive when you discover them.",
            createYourFirstProvider: "Create your first provider",
            connectedToTheGateway: "Connected to the gateway",
            discoverModels: "Discover models",
            createAPIKey: "Create API key",
            openAndDiscover: "Open and discover",
            reviewModels: "Review models",
            credentialNotFresh: "Credential not fresh",
            credentialExpired: "Credential expired",
            reconnect: "Reconnect",
            openProvider: "Open provider",
            createAProvider: "Create a provider",
            discoverItsModelsFromUpstream: "Discover its models from upstream",
            issueAnAPIKeyForServingClients: "Issue an API key for serving clients",
            pointAClientAtAnEndpointBelow: "Point a client at an endpoint below",
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "Inspect public routes for quota availability. These figures describe stored switches and inventory.",
            noRequestsRecordedMessage: "No requests recorded.",
        },
        modelDetailScreen: {
            model: "Model",
            upstreamOnProvider: "{{upstreamId}} on {{provider}}.",
            modelsBackLabel: "Models",
            capabilities: "Capabilities",
            whatTheModelListingsDeclareForThisModelMessage:
                "What the model listings declare for this model. Nothing here gates a request — the upstream answers what it answers.",
            reasoningLevels: "Reasoning levels",
            maxInputTokens: "Max input tokens",
            maxOutputTokens: "Max output tokens",
            compaction: "Compaction",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "The row's own compaction facts — the override, else the latest discovery. The definition's declaration decides whatever both leave unstated.",
            noLevels: "No levels",
            addAlias: "Add alias",
            editModel: "Edit model",
            details: "Details",
            publicModelNames: "Public model names",
            directRoutes: "Direct routes",
            upstreamID: "Upstream ID",
            provider: "Provider",
            discovery: "Discovery",
            status: "Status",
            models: "Models",
            overriddenByAdministratorMessage: "Overridden by administrator.",
            fromTheLatestDiscoveryMessage: "From the latest discovery.",
            unknownOmittedFromModelListingsMessage: "Unknown — omitted from model listings.",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "No fact on this row — the definition's declaration decides.",
            unknown: "Unknown",
            noAutomaticEntry: "No automatic entry",
            enabled: "Enabled",
            disabled: "Disabled",
        },
        modelEditScreen: {
            editModel: "Edit model",
            modelSettings: "Model settings",
            enabled: "Enabled",
            deleteThisModel: "Delete this model",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "Deleting removes this physical model and its direct routes. Pool routes remain stored.",
            model: "Model",
        },
        modelNewScreen: {
            createModel: "Create model",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "Something discovery would not produce — or a model the upstream no longer reports, to claim back.",
            noProvidersYet: "No providers yet",
            aModelBelongsToAProviderAndThereMessage:
                "A model belongs to a provider, and there is none to hold it. Add a provider first — discovery may already produce this model.",
            createProvider: "Create provider",
            aModelAddedByHandIsNeverRemovedMessage:
                "A model added by hand is never removed automatically — and it claims back what discovery lost.",
            models: "Models",
            identity: "Identity",
            provider: "Provider",
            upstreamID: "Upstream ID",
            modelSettings: "Model settings",
            enabled: "Enabled",
            aModelBelongsToOneProviderMessage: "Required. A model belongs to one provider.",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "Required. Sent upstream verbatim — letters, numbers, and symbols only.",
            carriedHereFromAModelTheUpstreamNoMessage:
                "Required. Carried here from a model the upstream no longer reports — submitting claims it back under the settings below.",
            creating: "Creating…",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "This model name is managed automatically. Edit the owning provider or pool prefix to change it.",
            deleteThisPublicRoute: "Delete this public route",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "Clients requesting its public ID will receive model-not-found. Physical inventory and pools remain stored.",
            publicRouteSettings: "Public route settings",
            enabled: "Enabled",
            publicID: "Public ID",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "The exact model name clients request. Renaming requires clients to update.",
            targetType: "Target type",
            pool: "Pool",
            upstreamID: "Upstream ID",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "Exact identifier requested on every member. Missing models do not prevent publication.",
            physicalModel: "Physical model",
            createPublicRoute: "Create public route",
            editPublicRoute: "Edit public route",
            publicRoutes: "Public routes",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "A model name maintained automatically by its provider or pool.",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "Clients request this public ID. Changing its target leaves that name intact.",
            openPool: "Open pool",
            openPhysicalModel: "Open physical model",
            deleteThisPublicRouteQuestion: "Delete this public route?",
            publicIDIsRequiredMessage: "Public ID is required.",
            directPhysicalModel: "Direct physical model",
            chooseAPool: "Choose a pool",
            upstreamIDIsRequiredMessage: "Upstream ID is required.",
            chooseAPhysicalModel: "Choose a physical model",
        },
        modelsScreen: {
            models: "Models",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "Models are automatically exposed through a standalone provider prefix or their owning pool prefixes.",
            createAliases: "Create aliases",
            createModel: "Create model",
            noModelsYet: "No models yet",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "Add a provider first, then discover its models — or add one by hand.",
            thisProviderHoldsNoModels: "This provider holds no models.",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "This provider is no longer listed — pick another in the Provider filter.",
            createProvider: "Create provider",
            searchModels: "Search models",
            searchByUpstreamIDOrProviderName: "Search by upstream ID or provider name",
            provider: "Provider",
            noModelsOnAnyProviderYetDiscoverOrMessage:
                "No models on any provider yet — discover or add one.",
        },
        notFoundScreen: {
            pageNotFound: "Page not found",
            goToOverview: "Go to Overview",
            providers: "Providers",
            models: "Models",
        },
        oauthFlowView: {
            flowExpires: "Flow expires",
            singleuseStateLabel: "Single-use state:",
            authorizationURL: "Authorization URL",
            openTheAuthorizationPage: "Open the authorization page",
            theURLAbove: "the URL above",
            copyTheURL: "Copy the URL",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "The state is single-use and the flow expires quickly — finish promptly after approving access.",
            mostLoginsEndOnAPageThatShowsMessage:
                "Most logins end on a page that shows nothing — that is expected. Paste the whole address the browser shows. If a page opens and displays a code, paste that code instead.",
            startOver: "Start over",
            callbackURLOrAuthorizationCode: "Callback URL or authorization code",
            pasteTheWholeCallbackURL: "Paste the whole callback URL",
            verificationCode: "Verification code",
            theVerificationCode: "the verification code",
            copyTheCode: "Copy the code",
            openTheVerificationPage: "Open the verification page",
            theVerificationURL: "the verification URL",
            approveTheCodeAtTheVerificationPageThisMessage:
                "Approve the code at the verification page; this page keeps asking the gateway for the outcome and moves on as soon as the flow completes.",
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "Stay on this page until the flow completes — navigating within the console discards the flow.",
        },
        poolAvailabilityCard: {
            refreshQuota: "Refresh quota",
            resetRestrictions: "Reset restrictions",
            quotaExhausted: "Quota exhausted",
            poolAvailability: "Pool availability",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "Stored observations. Refresh explicitly queries the upstream account. Reset clears restrictions and installs refresh grace; it does not reset upstream quota.",
            thisPoolHasNoMembersMessage: "This pool has no members.",
            readingStoredAvailability: "Reading stored availability…",
            quotaNotObserved: "Quota not observed",
            quotaReadsUnsupported: "Quota reads unsupported",
            administratorResetRefreshGraceInstalled: "Administrator reset; refresh grace installed",
            accountEnabled: "Account enabled",
            updatingAvailability: "Updating availability…",
            allModels: "All models",
            resetUnknown: "Reset unknown",
            resetTimePassedRefreshRequired: "Reset time passed; refresh required",
        },
        providerDetailScreen: {
            provider: "Provider",
            editProvider: "Edit provider",
            editPage: "edit page",
            storedAvailability: "Stored availability",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "All observed restrictions on this account. Reset clears restrictions and installs refresh grace; it does not reset upstream quota.",
            details: "Details",
            modelPrefix: "Model prefix",
            type: "Type",
            definition: "Definition",
            upstreamBaseURL: "Upstream base URL",
            credentials: "Credentials",
            status: "Status",
            discoveredFromUpstreamOrAddedByHandMessage:
                "Discovered from upstream, or added by hand.",
            createModel: "Create model",
            allModels: "All models",
            providers: "Providers",
            createProvider: "Create provider",
            enabled: "Enabled",
            disabled: "Disabled",
            theOAuthCredentialsHaveExpiredMessage: "The OAuth credentials have expired.",
            noneExactUpstreamIDs: "None — exact upstream IDs",
            discovering: "Discovering…",
            discoverModels: "Discover models",
            askingTheUpstreamWhatItReports: "Asking the upstream what it reports…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "No models on this provider yet — discover what the provider reports, or add one by hand.",
        },
        providerEditScreen: {
            editProvider: "Edit provider",
            settings: "Settings",
            credentials: "Credentials",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "OAuth credentials expire — reconnecting replaces them.",
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "This provider's definition evaluates requests against a base URL — restore one, or Revert.",
            deleteThisProvider: "Delete this provider",
            provider: "Provider",
            modelPrefix: "Model prefix",
            aProviderNeedsANameMessage: "A provider needs a name.",
            starting: "Starting…",
            reconnectTheAccount: "Reconnect the account",
            replacing: "Replacing…",
            replaceTheCredentials: "Replace the credentials",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "Deleting removes the account and its pool memberships. It has no physical models.",
        },
        providerNewScreen: {
            createProvider: "Create provider",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "One upstream account. Pick the preset — the credential path follows.",
            providersBackLabel: "Providers",
            readAgain: "Read again",
            preset: "Preset",
            switchPreset: "Switch preset",
            keepMyEdit: "Keep my edit",
            identity: "Identity",
            connection: "Connection",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage:
                "The key is write-only — the gateway never returns it.",
            connectTheAccount: "Connect the account",
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "Locked while the authorization is live — the provider is created from what the flow's start already sent.",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "Approve access where the flow points, then let it complete. This app never stores the credentials.",
            upstreamBaseURL: "Upstream base URL",
            definition: "Definition",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "The connection recipe the gateway stores. Editing the JSON of a stored definition registers the next version — the stored one is never rewritten.",
            notStoredYetSubmittingCreatesVersion1Message:
                "Not stored yet — submitting creates version 1.",
            advancedTheConnectionRecipePrefilledByThePreset:
                "Advanced: the connection recipe — prefilled by the preset",
            definitionJSON: "Definition JSON",
            resetToPreset: "Reset to preset",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "The connection recipe is invalid — open the Advanced disclosure above for the offending path.",
            modelPrefix: "Model prefix",
            creating: "Creating…",
            connecting: "Connecting…",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "Required. The upstream address this definition's requests evaluate against — the gateway refuses a flow start without one.",
            starting: "Starting…",
            startTheAuthorization: "Start the authorization",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "Interchangeable accounts for the same upstream service, tried in priority order.",
            inspectUpstreamID: "Inspect upstream ID",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "Inspects saved membership. Leaving this empty shows account-wide restrictions only.",
            deleteThisPool: "Delete this pool",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "Deleting removes automatic pool entries and restores standalone member prefixes. Authored routes must be removed or retargeted first.",
            poolSettings: "Pool settings",
            enabled: "Enabled",
            name: "Name",
            modelPrefix: "Model prefix",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "The pool automatically exposes member models as this prefix + upstream ID. Leave empty to use exact upstream IDs. Changing the prefix renames existing automatic entries.",
            accountsInPriorityOrder: "Accounts in priority order",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "Members must serve the same upstream service. Their own prefix entries are hidden while pooled. New models appear here automatically and route only to members that have them. Empty pools are allowed.",
            moveUp: "Move up",
            moveDown: "Move down",
            remove: "Remove",
            addAccount: "Add account",
            addMember: "Add member",
            createPool: "Create pool",
            editPool: "Edit pool",
            pools: "Pools",
            deleteThisPoolItsMembershipIsRemovedMessage:
                "Delete this pool? Its membership is removed.",
            nameIsRequiredMessage: "Name is required.",
            chooseAnAccount: "Choose an account",
        },
        providerQuotaCard: {
            upstreamQuota: "Upstream quota",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "Allowances belong to the upstream account and may include usage from other clients.",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage:
                "This provider definition does not declare quota viewing.",
            checkingWhetherThisDefinitionSupportsQuotaReads:
                "Checking whether this definition supports quota reads…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "Quota not observed. Refresh explicitly queries the upstream account.",
            readingUpstreamQuota: "Reading upstream quota…",
            showingThePreviousReadingMessage: "Showing the previous reading.",
            observed: "Observed",
            resetTimeReachedRefreshToUpdate: "Reset time reached; refresh to update",
            theUpstreamReturnedNoQuotaEntriesMessage: "The upstream returned no quota entries.",
            periodUnknown: "Period unknown",
            monthly: "Monthly",
            unknown: "Unknown",
            unlimited: "Unlimited",
            resetUnknown: "Reset unknown",
            readingQuota: "Reading quota…",
            refreshingQuota: "Refreshing quota…",
            refreshQuota: "Refresh quota",
            retryQuota: "Retry quota",
        },
        providersScreen: {
            providers: "Providers",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "Upstream accounts. Open one for its models and usage — editing lives one step further in.",
            createProvider: "Create provider",
            noProvidersYet: "No providers yet",
            aProviderIsOneUpstreamAccountMessage: "A provider is one upstream account.",
            searchProviders: "Search providers",
            searchByNameTypeOrFormat: "Search by name, type, or format",
        },
        readNotice: {
            readAgain: "Read again",
        },
        routingListsScreen: {
            pools: "Pools",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "Named model catalogs built from interchangeable accounts for one upstream service.",
            createPool: "Create pool",
            noPoolsYet: "No pools yet",
            createANamedPoolWithAModelPrefixMessage:
                "Create a named pool with a model prefix. Member models are exposed automatically.",
            noModelPrefix: "none",
            publicRoutes: "Public routes",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "Model names clients request. Standalone providers and pools expose model names automatically. Additional aliases are optional.",
            createPublicRoute: "Create public route",
            noPublicRoutesYet: "No public routes yet",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "Publish a physical model directly or target a pool and exact upstream ID.",
            automaticPrefix: "Automatic · ",
            on: "On",
            off: "Off",
        },
        screenFrame: {
            readAgain: "Read again",
        },
        usageQueryScreen: {
            usageAggregates: "Usage aggregates",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "One bucket per dimension combination inside the filter — counts and token sums, never content.",
            viewRequests: "View requests",
            question: "Question",
            groupBy: "Group by",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "Any dimension set works — none selected totals every matching row.",
            window: "Window",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "Fills the bounds below — from one window back, to left open — resolved now; the fields stay hand-editable.",
            from: "From",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "Inclusive, in this browser's zone — like 2026-09-23T10:00. Blank sets no bound.",
            to: "To",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "Exclusive, in this browser's zone — like 2026-09-23T10:00. Blank sets no bound.",
            clearFilters: "Clear filters",
            usageBackLabel: "Usage",
            theQueryDidNotRunAFilterValueIsInvalid:
                "The query did not run — a filter value is invalid.",
            total: "Total",
            nothingRecordedInThisWindowMessage: "Nothing recorded in this window.",
            noRowsInThisWindowMessage: "No rows in this window.",
            bucket: "Bucket",
            requests: "Requests",
            input: "Input",
            output: "Output",
            cached: "Cached",
            cacheWrite: "Cache write",
            reasoning: "Reasoning",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "Token totals skip rows where the upstream reported no usage — an absent breakdown never inflates or deflates a total.",
            querying: "Querying…",
            runQuery: "Run query",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "No filter or bound narrows the question, so an empty answer is the gateway's whole record — it holds no usage yet.",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "Nothing recorded matches these filters — widen the bounds, drop a filter, or query without one. Clear filters sits in the form above.",
        },
        usageScreen: {
            usage: "Usage",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "Every dispatched request, newest first — numbers only, never content. A row reporting none of the optional usage figures leaves them off whole: an all-zero tail and an unreported one cannot be told apart here.",
            viewAggregates: "View aggregates",
            filter: "Filter",
            theSharedAccountingFilterTheSameOneTheMessage:
                "The shared accounting filter — the same one the aggregates page narrows.",
            window: "Window",
            rollingResolvedWhenTheStreamIsReadMessage: "Rolling, resolved when the stream is read.",
            clearFilters: "Clear filters",
            noRequestsRecordedYet: "No requests recorded yet",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "One row appears per dispatched serving request — the moment the gateway sends upstream. If traffic flows and nothing appears, check the api_logs section in the gateway's configuration file.",
            noRecordedRequestsMatchThisFilterMessage: "No recorded requests match this filter.",
            filtersStayBehind: " — the window and every other filter stay behind.",
            streamMark: " · Stream",
            thisRowIsNoLongerListed: "This row is no longer listed.",
            reading: "Reading…",
            applyFilters: "Apply filters",
            widenTheWindowDropAFilterOrReadMessage:
                "Widen the window, drop a filter, or read the whole stream — Clear filters sits in the form above.",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "The gateway's filter locates rows; it never validates a value against them, so an unknown id or a status never recorded simply matches nothing.",
            thatIsEveryRowThisFilterMatchesMessage: "That is every row this filter matches.",
            thatIsEveryRecordedRowMessage: "That is every recorded row.",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "A read cycle is replacing the stream — extend it once the cycle lands.",
            loading: "Loading…",
            loadMore: "Load more",
        },
        usageStats: {
            usage: "Usage",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "Requests and tokens per day — numbers only, never content.",
            openAggregates: "Open aggregates",
            viewRequests: "View requests",
            requests: "Requests",
            dispatchedAllDays: "Dispatched, all days",
            inputTokens: "Input tokens",
            summedUsageReported: "Summed, usage reported",
            outputTokens: "Output tokens",
            day: "Day",
            input: "Input",
            output: "Output",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage:
                "Days are UTC — the gateway's own day boundary.",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage:
                "The upstream no longer reports this model.",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "This account already holds that exact upstream model. Open its existing inventory row.",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "Another public route holds that public ID. Choose an explicit different name or edit the existing route.",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "Another pool holds that name. Choose a different pool name.",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "Public routes still reference this pool. Change or delete those routes before deleting the pool.",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne:
            "Another key already holds that secret; generate a fresh one.",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "The stored version moved — use Refresh in the bar above; it re-reads the stored versions and leaves this form untouched — then submit again.",
        chooseAProviderNameNoOtherRowHolds: "Choose a provider name no other row holds.",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "Another provider took this name before the flow could finish. Start over ends this flow and unlocks the name; then create again with a name no other row holds.",
        chooseAnIdentifierNoOtherRowHolds: "Choose an identifier no other row holds.",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "If the provider's authorization lapsed, reconnect it on the provider's edit page; if the upstream genuinely lists no models yet, add one by hand from this page.",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "If the provider's base URL or API key is wrong, correct it on the provider's edit page; if the upstream genuinely lists no models yet, add one by hand from this page.",
        checkTheProvidersStateAndCredentials: "Check the provider's state and credentials.",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "The provider refused the exchange; start the flow again from this page.",
        checkTheProvidersCredentialsAndReachability:
            "Check the provider's credentials and reachability.",
        nothingAnsweredAtBaseUrl: "Nothing answered at [[baseUrl]].",
        theBrowserCouldNotCompleteTheConnection: "The browser could not complete the connection.",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "Check the address, that the gateway is running, and that its `listen` address is reachable from this browser.",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl:
            "An HTTPS page cannot call the HTTP address [[baseUrl]].",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "This page is served over HTTPS, and the browser blocks its calls to HTTP addresses outright, so the gateway was never contacted.",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "Serve this app over HTTP as well, or add a `tls` section to that instance's configuration file so the gateway answers HTTPS.",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "The gateway answered, but the browser blocked the response from [[baseUrl]].",
        theBrowserWithheldTheResponseFromThisPage:
            "The browser withheld the response from this page.",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "Add [[origin]] to `[[setting]]` in that instance's configuration file and restart the gateway. Serving this app from the gateway's own origin avoids the allowlist entirely.",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "Check the address, that the gateway is running, and that it is reachable from this browser over this scheme.",
        noAnswerFromBaseUrl: "No answer from [[baseUrl]].",
        theBrowserReceivedNoAnswerItCouldRead: "The browser received no answer it could read.",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "Either nothing is listening at that address, or the browser blocked the response before this page could read it — add [[origin]] to `[[setting]]` and restart the gateway.",
        theGatewayRejectedThatManagementToken: "The gateway rejected that management token.",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "The token must match `management_api.token` in that instance's configuration file; the gateway reads it live, so a changed token needs no restart.",
        theGatewayHoldsNoSuchRow: "The gateway holds no such row.",
        theGatewayRefusedAnIdentifierIsAlreadyTaken:
            "The gateway refused: an identifier is already taken.",
        theUpstreamProviderRefusedTheCallOrWasUnreachable:
            "The upstream provider refused the call or was unreachable.",
        theGatewayFailedInternally: "The gateway failed internally.",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "The cause is in the gateway's stderr log; internal causes are never returned to this page.",
        theGatewayRejectedTheRequest: "The gateway rejected the request.",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow:
            "The gateway answered with a shape this build does not know.",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "This build's generated bindings and the gateway's management API disagree; regenerate the bindings or rebuild this frontend from a matching revision.",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "The gateway answered HTTP {{httpStatus}} without a management error body.",
        checkTheGatewaysStderrLogForTheCause: "Check the gateway's stderr log for the cause.",
        theCallFailed: "The call failed.",
    },
    notices: {
        savedName: "[[name]] saved.",
        credentialsReplaced: "Credentials replaced.",
        expiryReplaced: "The provider's listing now carries the new expiry.",
        keyReplaced: "The upstream API key was replaced.",
        previousSecretRevoked:
            "The previous secret stopped working on this save — update every client that held it.",
        lastKeyDisabled:
            "This was the last enabled key: the gateway now serves nothing until a key is turned on.",
        modelSaved: "Model saved.",
        poolSaved: "Pool saved.",
        routeSaved: "Public route saved.",
        discovered_one: "[[quantity]] model discovered from [[providerName]].",
        discovered_other: "[[quantity]] models discovered from [[providerName]].",
        nothingDiscovered: "Nothing new discovered from [[providerName]].",
        manualUntouched_one: "{{quantity}} added by hand was left untouched.",
        manualUntouched_other: "{{quantity}} added by hand were left untouched.",
        vanished_one: "{{quantity}} model is now listed as no longer reported.",
        vanished_other: "{{quantity}} models are now listed as no longer reported.",
    },
    connectionValidation: {
        empty: "Enter the JanusGate base URL.",
        notAbsolute: "Enter an absolute URL, for example https://gateway.example.com.",
        scheme: "Use an http:// or https:// URL.",
        credentials: "Remove the credentials from the URL.",
        notOrigin: "Enter only the gateway origin, without a path, query, or fragment.",
        managementToken: "Enter the management token.",
        nameEmpty: "Name cannot be empty.",
    },
    operationNotices: {
        flowFailed: "The flow failed.",
        freshFlow: "Start a fresh flow and authorize it promptly.",
        flowExpired: "The flow expired.",
        flowExpiryPassed: "The flow's expiry passed while this page waited for the outcome.",
        pollStopped: "The outcome poll kept failing, so this page stopped asking.",
        pollFailed: "The last outcome poll failed.",
        invalidDefinition: "The definition JSON is not a definition the gateway could store.",
        fixDefinition: "Fix the JSON and submit again; nothing was written.",
        fixFilter: "Fix that field, then run the query again.",
        discoveryPending: "Discovering models from [[providerName]]…",
        discoveryQuery: "Asking the upstream what it reports right now.",
    },
    counts: {
        models_one: "{{total}} model",
        models_other: "{{total}} models",
        filteredModels_one: "{{shown}} of {{total}} model",
        filteredModels_other: "{{shown}} of {{total}} models",
        enabledKeys_one: "{{enabled}} of {{total}} key enabled",
        enabledKeys_other: "{{enabled}} of {{total}} keys enabled",
        shownKeys_one: "{{shown}} key shown",
        shownKeys_other: "{{shown}} keys shown",
        filteredKeys_one: "[[shownSentence]] · [[enabled]] of [[total]] key enabled",
        filteredKeys_other: "[[shownSentence]] · [[enabled]] of [[total]] keys enabled",
        providerDeleted_one:
            "Deleting removes the account, its pool memberships, and its {{total}} physical model with their direct routes. Pool routes remain stored.",
        providerDeleted_other:
            "Deleting removes the account, its pool memberships, and its {{total}} physical models with their direct routes. Pool routes remain stored.",
        requests_one: "{{total}} request",
        requests_other: "{{total}} requests",
        requestsPeak_one: "[[total]] request · peak [[peak]] on [[day]]",
        requestsPeak_other: "[[total]] requests · peak [[peak]] on [[day]]",
    },
    helpers: {
        accessControl: {
            allowed: "Allowed",
            denied: "Denied",
            modelOverride: "Model override",
            globallyDisabled: "Globally disabled",
            targetUnavailable: "Target unavailable",
            targetVanished: "Target vanished",
            globallyAvailable: "Globally available",
            noAvailablePoolMembers: "No available pool members",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "Base URL must be an http:// or https:// address with no spaces.",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "Any endpoint speaking the OpenAI Responses protocol with a static key.",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "Any endpoint speaking the Chat Completions protocol with a static key.",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "Any endpoint speaking the Gemini GenerateContent protocol with a static key.",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "Any endpoint speaking the Gemini Interactions protocol with a static key.",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "Any endpoint speaking the Anthropic Messages protocol with a static key.",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "The xAI Responses endpoint with a static key: native compaction rides the standalone `/responses/compact` operation, while the explicit trigger and the automatic compaction are declared unsupported — a standalone compact endpoint does not imply native trigger or automatic compaction.",
            claudeCodeClaudeAccount: "Claude Code (Claude account)",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "A Claude subscription account over the Messages API: the OAuth token rides `Authorization: Bearer` beside the `oauth-2025-04-20` beta header, on inference and model discovery alike.",
            kimiCodeKimiAccount: "Kimi Code (Kimi account)",
            githubCopilotGitHubAccount: "GitHub Copilot (GitHub account)",
            devinDevinAccount: "Devin (Devin account)",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "A Devin account whose ConnectRPC token exchange answers a static API key and API server URL — no refresh, no expiry; models are added manually.",
            codexChatGPTAccount: "Codex (ChatGPT account)",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "A ChatGPT account over the Codex Responses endpoint. Streaming is forced upstream and responses render with the OpenAI Responses format. The compaction section declares the explicit trigger and the automatic threshold compaction the Codex backend accepts.",
            antigravityGoogleAccount: "Antigravity (Google account)",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "A Google account over the Antigravity Gemini endpoint, with project onboarding and an enveloped GenerateContent request.",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "Zen over the OpenAI Chat Completions protocol with a static key.",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "Zen over the OpenAI Responses protocol with a static key.",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "Zen over the Anthropic Messages protocol with a static key.",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "Zen over the Gemini GenerateContent protocol with a static key.",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "That paste holds neither a bare code nor a URL with a code — copy the whole address the login landed on, or just its code, and paste it here.",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "The flow stays live; this page keeps asking for the outcome.",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "The gateway holds no pool here. It was deleted, or the address was typed wrong.",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "The gateway holds no public route here. It was deleted, or the address was typed wrong.",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "The gateway holds no API key here. It was deleted, or the address was typed wrong. Nothing here can bring it back.",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "The gateway holds no model here. It was deleted, or the address was typed wrong — a deleted discovered model returns as new on the next discovery.",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "The gateway holds no provider here. It was deleted, or the address was typed wrong. Nothing here can bring it back.",
        },
        app: {
            readThisScreensStateFromTheGatewayAgain:
                "Read this screen's state from the gateway again",
            thisScreenReadsNothingFromTheGatewaySo:
                "This screen reads nothing from the gateway, so there is nothing to refresh",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "Disconnect and forget the stored token? Reconnecting means entering it again.",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "Required. Which client holds this key.",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "Required. A serving client authenticates with it on every surface.",
        },
        clipboard: {
            copyToClipboard: "Copy to clipboard",
            copied: "Copied",
            copy: "Copy",
            theValueHere: "the value here",
        },
        confirmDeleteCard: {
            deleting: "Deleting…",
            deleteForGood: "Delete for good",
        },
        dirtyFormFooter: {
            saving: "Saving…",
            saveChanges: "Save changes",
        },
        layout: {
            back: "Back",
        },
        providerForm: {
            verifiedLive: "Verified live",
            unverified: "Unverified",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "Required. The upstream address this definition's requests point at.",
            upstreamAPIKey: "Upstream API key",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "Required. Sent upstream with every request. Never returned by the gateway.",
            showTheAPIKeyWhileTyping: "Show the API key while typing",
        },
        apiKeyAccess: {
            inherit: "Inherit",
            allow: "Allow",
            deny: "Deny",
            provider: "Provider",
            pool: "Pool",
            automatic: "Automatic",
            alias: "Alias",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels:
                "Following discovery — currently declares no levels.",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "Replaces the discovered set — an empty selection declares no levels.",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "Following discovery — currently unknown, so listings omit it.",
            replacesTheDiscoveredValueWithAPositiveInteger:
                "Replaces the discovered value with a positive integer.",
            unset: "Unset",
            discoveredNothingKnown: "Discovered: nothing known.",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "Following discovery — currently nothing is known, so the definition's declaration decides.",
        },
        modelsTable: {
            ready: "Ready",
            off: "Off",
            suspended: "Suspended",
            heldByProvider: "Held by provider",
            unknown: "Unknown",
            noAutomaticEntry: "No automatic entry",
            noDirectRoutes: "No direct routes",
        },
        providersTable: {
            valid: "Valid",
            expired: "Expired",
            stale: "Stale",
            on: "On",
            off: "Off",
        },
        usageFilterControls: {
            allOutcomes: "All outcomes",
            failuresUpstreamOrInternal: "Failures (upstream or internal)",
            any: "Any",
            streamed: "Streamed",
            nonstreamed: "Non-streamed",
            allEndpoints: "All endpoints",
        },
        usageFilter: {
            allTime: "All time",
            last24Hours: "Last 24 hours",
            last7Days: "Last 7 days",
            last30Days: "Last 30 days",
            invalidStatusList:
                "HTTP statuses must be plain integers, comma-separated — like 200, 429 — or blank for no status filter.",
        },
    },
    validation: {
        composition: {
            upstreamId:
                "Upstream ID accepts letters, numbers, and symbols only — no spaces or non-ASCII text.",
            publicId:
                "Public ID accepts letters, numbers, and symbols only — no spaces or non-ASCII text.",
            modelPrefix:
                "Model prefix accepts letters, numbers, and symbols only — no spaces or non-ASCII text.",
            apiKey: "API key accepts letters, numbers, and symbols only — no spaces or non-ASCII text.",
            key: "Key accepts letters, numbers, and symbols only — no spaces or non-ASCII text.",
        },
        routing: {
            upstreamId:
                'Upstream ID accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
            publicId:
                'Public ID accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
            modelPrefix:
                'Model prefix accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
            apiKey: 'API key accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
            key: 'Key accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
        },
        positive: {
            maxInputOverride: "Max input tokens override must be a positive integer.",
            maxOutputOverride: "Max output tokens override must be a positive integer.",
        },
    },
    quotaMessages: {
        day_one: "{{quantity}} day",
        day_other: "{{quantity}} days",
        hour_one: "{{quantity}} hour",
        hour_other: "{{quantity}} hours",
        minute_one: "{{quantity}} minute",
        minute_other: "{{quantity}} minutes",
        second_one: "{{quantity}} second",
        second_other: "{{quantity}} seconds",
        used: "{{percentage}} used",
        counter: "[[used]] / [[limit]] [[unit]] used",
        remaining: "[[remaining]] [[unit]] remaining",
        dateReset: "[[date]] · Exact reset time unknown",
        atReset: "Resets [[instant]] · [[distance]]",
        future: "in {{duration}}",
        observed: "Observed [[instant]] · [[distance]]",
        inventory: {
            pool_disabled: "Pool disabled",
            account_missing: "Account missing",
            account_disabled: "Account disabled",
            model_not_selected: "Choose an upstream ID to inspect model availability",
            model_missing: "Model missing",
            model_disabled: "Model disabled",
            model_vanished: "Model vanished",
            ready: "Inventory ready",
        },
        failure: {
            credential_renewal: "credential renewal failed",
            timeout: "timeout",
            transport: "transport failure",
            upstream_rejected: "upstream rejected the request",
            malformed_observation: "malformed observation",
        },
        quotaObserved: "Quota observed [[instant]]",
        quotaFailed: "Quota read failed · {{reason}}",
        deletedAccount: "Deleted account #{{id}}",
        priority: "Priority {{priority}}",
        models: "Models: [[identifiers]]",
        resets: "Resets [[instant]]",
    },
    dynamicMessages: {
        following: "Following discovery: {{value}}.",
        levelsReplace: "Replaces the discovered set — clients see {{value}}.",
        capReplace: "Replaces the discovered value — clients see {{value}}.",
        mechanismFact: "{{mechanism}}: {{support}}",
        mechanismDiscovered: "Discovered: {{support}}.",
        compactionReplace:
            "Replaces the discovered facts wholesale — a mechanism left unset falls back to discovery, then the definition's declaration.",
        noExpiry: "No expiry is stored for this provider.",
        goneFlow:
            "This authorization flow is no longer completable — its state expired or was already used.",
        refusedClipboard:
            "This browser refused the clipboard — select {{subject}} and copy it by hand.",
        enabledStored: "enabled of {{total}} stored",
        readyListed: "ready of {{total}} listed",
        copyEndpoint: "Copy the {{surface}} endpoint URL",
        copyApiKeySecret: "Copy the secret of {{name}}",
        disabledCredential:
            "{{expiry}} The provider is disabled — reconnect when you re-enable it.",
        noKeysDetail:
            " Serving clients authenticate with one — without it, the gateway has no ready inventory.",
    },
    remainingText: {
        notRefreshedYet: "Not refreshed yet",
        overrideMaxInputTokens: "Override max input tokens",
        maxInputTokensOverride: "Max input tokens override",
        overrideMaxOutputTokens: "Override max output tokens",
        maxOutputTokensOverride: "Max output tokens override",
        chooseAProvider: "Choose a provider",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "Naming a provider and upstream ID the upstream no longer reports claims that model back under the settings below. The model is automatically exposed through its standalone provider prefix or owning pool prefixes.",
        allProviders: "All providers",
        replaceTheUpstreamAPIKey: "Replace the upstream API key",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "Write-only: the gateway never returns it, so leaving this empty keeps what is stored.",
        backToPools: "Back to pools",
        publicRoute: "Public route",
        publicRoutes: "Public routes",
        backToPublicRoutes: "Back to public routes",
        backToAPIKeys: "Back to API keys",
        backToModels: "Back to models",
        backToProviders: "Back to providers",
        deletedRoute: "Deleted route",
        deletedModel: "Deleted model",
        deletedProvider: "Deleted provider",
        deletedKey: "Deleted key",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "The answer below is still the previous query's — the field showing an inline error names the problem.",
        fromAndToDoNotApplyThere: " — From and To do not apply there.",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "One row appears per dispatched serving request — the moment the gateway sends upstream.",
        showTheKeyWhileTyping: "Show the key while typing",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "Save writes your edits over the gateway's version; Revert picks the gateway's version up.",
        theOAuthCredentialsWereNeverFresh: "The OAuth credentials were never fresh.",
        allPublicRoutes: "All public routes",
        allPhysicalModels: "All physical models",
        allAPIKeys: "All API keys",
    },
    entityMessages: {
        editName: "Edit [[name]]",
        editIdentifier: "Edit [[identifier]]",
        deleteKey: "Delete [[name]]? Clients holding this secret lose access.",
        deleteLastKey:
            "Delete [[name]]? It is the last enabled key — the gateway will serve nothing until another key is turned on.",
        deleteModel:
            "Delete [[identifier]]? Its direct routes are deleted too. Pool routes remain stored. Rediscovery or manual creation produces a new physical row and does not restore deleted routes.",
        deleteProvider: "Delete [[name]]?",
        switchPreset: "Switch to [[name]]? Your edited connection recipe will be replaced.",
        noKeysMatch: 'No API keys match "[[query]]".',
        noProvidersMatch: 'No providers match "[[query]]".',
        noModelsMatch: 'No models match "[[query]]".',
        noProviderModelsMatch: 'No models of this provider match "[[query]]".',
        missingPath:
            "Nothing lives at [[path]]. It may have been deleted, or the address was typed wrong.",
        aliasCreated: "Alias created: [[identifier]]",
        addAlias: "Add alias for [[identifier]]",
    },
    modelCard: {
        heading: "Models · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "This key changed on the gateway while you have unsaved edits.",
            provider: "This provider changed on the gateway while you have unsaved edits.",
            model: "This model changed on the gateway while you have unsaved edits.",
            pool: "This pool changed on the gateway while you have unsaved edits.",
            "public route":
                "This public route changed on the gateway while you have unsaved edits.",
            "api key": "This api key changed on the gateway while you have unsaved edits.",
        },
        noRequests: {
            key: "No requests recorded for this key yet.",
            provider: "No requests recorded for this provider yet.",
            model: "No requests recorded for this model yet.",
            pool: "No requests recorded for this pool yet.",
            "public route": "No requests recorded for this public route yet.",
            "api key": "No requests recorded for this api key yet.",
        },
        notFound: {
            api_key: "API key not found",
            provider: "Provider not found",
            model: "Model not found",
            pool: "Pool not found",
            model_route: "Public route not found",
        },
        replaceSecret: {
            typed: "Replace the typed secret?",
            current: "Replace the current secret?",
        },
        bound: {
            From: "From must be a complete date and time, like 2026-09-23T10:00 — or blank for no bound.",
            To: "To must be a complete date and time, like 2026-09-23T10:00 — or blank for no bound.",
        },
        updated: "Updated {{instant}}",
        created: "Created {{instant}}",
        version: "Stored as version {{version}}.",
        unlisted: "{{value}} (no longer listed)",
        accountUnlisted: "Account #{{id}} (no longer listed)",
        moveUp: "Move {{name}} up",
        moveDown: "Move {{name}} down",
        remove: "Remove {{name}}",
        publicIdFor: "Public ID for {{subject}}",
        source: {
            key: "from key",
            provider: "from provider",
            pool: "from pool",
            route: "model override",
        },
        deleted: {
            provider: "Deleted provider #{{id}}",
            pool: "Deleted pool #{{id}}",
            route: "Deleted route #{{id}}",
        },
        status: "Client {{client}} · Upstream {{upstream}}",
        metrics: " · {{duration}} · in {{input}} · out {{output}}{{tail}}",
        optionalTokens: " · cached {{cached}} · cache write {{write}} · reasoning {{reasoning}}",
    },
    routingMessages: {
        directRoutes_one: "{{quantity}} direct route",
        directRoutes_other: "{{quantity}} direct routes",
        poolMembers_one: "[[quantity]] account member · Model prefix: [[prefix]]",
        poolMembers_other: "[[quantity]] account members · Model prefix: [[prefix]]",
        directMissing: "Direct · Physical model #[[id]]",
        direct: "Direct · [[provider]] · [[model]]",
        pool: "Pool · [[pool]] · [[model]]",
        publicIds: "Public ID [[identifiers]]",
        modelAccess: "[[identifier]] model access",
        ownerDefault: {
            provider: "[[name]] provider default",
            pool: "[[name]] pool default",
        },
        directTarget: "Direct · model #[[id]]",
        poolTarget: "Pool · [[identifier]]",
        reconnect: "Reconnect on the [[editLink]] — its Credentials card replaces them.",
        reconnectDisabled:
            "The provider is disabled — reconnect when you re-enable it; the [[editLink]]'s Credentials card replaces them.",
    },
    linkedMessages: {
        keyEndpoint:
            "Point your client at an endpoint — every surface serves permitted public entries; the URLs are on the [[overviewLink]].",
        servingKey:
            "A serving client authenticates with a serving API key — create one under [[keysLink]] before pointing anything here.",
        vanished:
            "It serves nothing meanwhile. [[discoverLink]] when the upstream lists it, or [[claimLink]].",
        discoverLabel: "Discover again on the provider's page",
        claimLabel: "claim it back from the Create model page",
        selected: "{{quantity}} selected. Successful rows will not be submitted again.",
        kimiDescription:
            "A Kimi account over the Coding endpoint: a device-code flow whose device ID slot rides the custom `X-Msh-*` headers on authorization, polling, and refresh alike.",
        githubDescription:
            "A GitHub account whose durable token is exchanged by reprovisioning for the short-lived Copilot session token and the account's API endpoint.",
        chartPoint_one: "[[day]]: [[quantity]] request",
        chartPoint_other: "[[day]]: [[quantity]] requests",
        inventoryEmpty: "[[name]] has no models yet.",
        inventoryUnready:
            "[[name]] has no ready inventory — every model is switched off or vanished.",
    },
    vocabulary: {
        credentialKind: {
            api_key: "Static API key",
            oauth: "OAuth account",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "Standalone compact",
            responses_explicit_trigger: "Explicit trigger",
            responses_automatic_compaction: "Automatic compaction",
            messages_on_demand_compaction: "On-demand compaction",
            messages_threshold_compaction: "Threshold compaction",
        },
        mechanismSupport: {
            supported: "Supported",
            unsupported: "Unsupported",
            unknown: "Unknown",
        },
        discoveryState: {
            discovered: "Discovered",
            vanished: "No longer reported",
            manual: "Manual",
        },
        reasoningLevel: {
            none: "none",
            minimal: "minimal",
            low: "low",
            medium: "medium",
            high: "high",
            xhigh: "xhigh",
            max: "max",
        },
        apiLogOutcome: {
            completed: "Completed",
            client_aborted: "Client aborted",
            upstream_error: "Upstream error",
            internal: "Internal",
        },
        apiLogEndpointType: {
            openai_chat_completions: "Chat Completions",
            openai_responses: "Responses",
            openai_responses_compact: "Responses compact",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interactions: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        usageGroupBy: {
            day: "Day",
            model: "Physical model",
            model_route: "Public route",
            provider: "Provider",
            api_key: "API key",
            endpoint_type: "Endpoint",
            outcome: "Outcome",
            is_stream: "Stream mode",
            http_status: "HTTP status",
        },
    },
    common: {
        save: "Save changes",
        cancel: "Cancel",
        retry: "Retry",
        language: "Language",
    },
    language: {
        system: "Follow browser",
        loading: "Loading {{languageName}}…",
        failed: "Could not load {{languageName}}. Your current language is still displayed.",
        startupFailed: "Could not load {{languageName}}. English is currently displayed.",
    },
    models: {
        discoveredCount_one: "Discovered {{count}} model.",
        discoveredCount_other: "Discovered {{count}} models.",
    },
} as const;
export default english;
