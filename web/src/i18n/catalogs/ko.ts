/** Korean administration messages. */
const korean = {
    shell: {
        skipToContent: "본문으로 건너뛰기",
        primary: "기본 탐색",
        refresh: "새로 고침",
        disconnect: "연결 끊기",
        keep: "유지",
    },
    connection: {
        connectToJanusGate: "JanusGate에 연결",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "이 앱은 JanusGate 게이트웨이, 즉 그 업스트림 계정과 모델, 자격 증명을 관리합니다.",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "이 브라우저에서 접근할 수 있는 실행 중인 JanusGate 인스턴스입니다.",
        theOriginBesideIt: "옆의 오리진",
        copyThisAppsOrigin: "이 앱의 오리진 복사",
        gatewayAddress: "게이트웨이 주소",
        schemeHostAndPortOnly: "스킴, 호스트, 포트만 입력합니다.",
        managementToken: "관리 토큰",
        sentOnlyToThatInstanceAndKeptOnly: "해당 인스턴스로만 전송되며 이 브라우저에만 보관됩니다.",
        cancel: "취소",
    },
    controls: {
        modelAccess: "모델 접근",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "기본값은 상속 중인 기존 항목에 적용됩니다. 계정 풀 접근에는 앞으로 자격을 갖추는 구성원도 포함됩니다. 전역 가용성은 권한과 별개입니다.",
        defaultModelAccessLabel: "기본 모델 접근:",
        defaultModelAccess: "기본 모델 접근",
        appliesToEntriesWithoutAProviderPoolOr:
            "공급자, 계정 풀, 모델 재정의가 없는 항목에 적용되며, 앞으로 생성될 항목도 포함됩니다.",
        searchModelAccess: "모델 접근 검색",
        publicIDOrOwnerName: "공개 ID 또는 소유자 이름",
        overridesOnly: "재정의만",
        removeOverride: "재정의 제거",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "저장하기 전에 삭제된 참조를 제거하거나, 새로 고침하여 모델 목록 불일치를 해소하세요.",
        thisDefaultAppliesToFutureEntries: "이 기본값은 앞으로 생성될 항목에 적용됩니다.",
        noEntriesMatchThisViewHiddenOverridesAre:
            "이 보기에 일치하는 항목이 없습니다. 숨겨진 재정의는 유지됩니다.",
        name: "이름",
        key: "키",
        skOrGenerateOne: "sk-… 또는 새로 생성",
        generate: "생성",
        replaceIt: "교체",
        keepIt: "유지",
        theSecretHere: "여기의 시크릿",
        delete: "삭제…",
        revert: "되돌리기",
        unsavedChanges: "저장하지 않은 변경 사항",
        overrideReasoningLevels: "추론 수준 재정의",
        reasoningLevels: "추론 수준",
        pickEveryLevelThisModelDeclaresUntickAll:
            "이 모델이 선언한 모든 수준을 선택합니다. 모두 선택 해제하면 아무 수준도 선언하지 않습니다.",
        aPositiveIntegerClearTheSwitchToFollow:
            "양의 정수입니다. 스위치를 해제하면 다시 탐색 값을 따릅니다.",
        overrideCompactionFacts: "압축 정보 재정의",
        compactionMechanisms: "압축 메커니즘",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            '이 모델이 대체하는 정보가 있는 메커니즘을 모두 지정합니다. 알 수 없음은 "정보 없음"을 분명히 밝히며, 해당 메커니즘의 탐색된 정보를 억제합니다.',
        addAlias: "별칭 추가",
        modelPrefix: "모델 접두사",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "계정 풀 밖에서는 모델을 이 접두사 + 업스트림 ID로 호출합니다. 구성원은 계정 풀 접두사를 사용합니다. 비워 두면 업스트림 ID를 그대로 사용합니다. 접두사를 변경하면 기존 자동 모델 이름도 변경됩니다.",
        upstreamBaseURL: "업스트림 기본 URL",
        enabled: "사용",
        publicRoute: "공개 라우트",
        provider: "공급자",
        physicalModel: "물리 모델",
        apiKey: "API 키",
        endpoint: "엔드포인트",
        outcome: "결과",
        streamMode: "스트림 모드",
        httpStatuses: "HTTP 상태",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "쉼표로 구분합니다(예: 200, 429). 응답이 없는 요청은 어떤 상태에도 일치하지 않습니다.",
    },
    navigation: {
        overview: "개요",
        providers: "공급자",
        inventory: "모델 목록",
        pools: "계정 풀",
        routes: "공개 라우트",
        apiKeys: "API 키",
        usage: "사용량",
        lightTheme: "라이트 테마 — 다크로 전환",
        darkTheme: "다크 테마 — 시스템으로 전환",
        systemTheme: "시스템 테마 — 라이트로 전환",
    },
    rich: {
        connected: "[[baseUrl]]에 연결되었습니다 — 토큰은 이 브라우저에만 저장됩니다.",
        setupToken: "관리 토큰은 해당 인스턴스 설정 파일의 [[setting]]입니다.",
        setupCors:
            "해당 인스턴스의 [[setting]]에서 이 앱의 오리진을 허용하세요. 단, 이 앱이 게이트웨이 자체에서 제공되는 경우는 예외입니다.",
    },
    titles: {
        bulkPublication: "모델 목록 게시",
        providerPools: "계정 풀",
        providerPoolNew: "계정 풀 만들기",
        providerPoolEdit: "계정 풀 #{{id}} 편집",
        modelRoutes: "공개 라우트",
        modelRouteNew: "공개 라우트 만들기",
        modelRouteEdit: "공개 라우트 #{{id}} 편집",
        home: "개요",
        providers: "공급자",
        providerNew: "공급자 만들기",
        providerDetail: "공급자 #{{id}}",
        providerEdit: "공급자 #{{id}} 편집",
        models: "모델",
        modelNew: "모델 만들기",
        modelDetail: "모델 #{{id}}",
        modelEdit: "모델 #{{id}} 편집",
        apiKeys: "API 키",
        apiKeyNew: "API 키 만들기",
        apiKeyDetail: "API 키 #{{id}}",
        apiKeyEdit: "API 키 #{{id}} 편집",
        usage: "사용량",
        usageQuery: "사용량 집계",
        notFound: "페이지를 찾을 수 없음",
    },
    format: {
        lessThanMinute: "1분 미만",
        minutes: "{{minutes}}분",
        hours: "{{hours}}시간",
        hoursMinutes: "{{hours}}시간 {{minutes}}분",
        days: "{{days}}일",
        daysHours: "{{days}}일 {{hours}}시간",
        expired: "{{distance}} 전에 만료되었습니다 — {{instant}}.",
        valid: "{{distance}} 동안 유효합니다 — {{instant}}까지.",
        durationSeconds: "{{seconds}}초",
        durationMinutesSeconds: "{{minutes}}분 {{seconds}}초",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "API 키",
            editAPIKey: "API 키 편집",
            secret: "시크릿",
            overview: "개요",
            details: "세부 정보",
            name: "이름",
            created: "생성 시각",
            status: "상태",
        },
        apiKeyEditScreen: {
            editAPIKey: "API 키 편집",
            settings: "설정",
            rotatingMeansGenerateThenSaveMessage: "키 교체는 생성을 누른 뒤 저장하는 것입니다.",
            enabled: "사용",
            replacingTheSecretRevokesTheOldValueOnMessage:
                "시크릿을 교체하면 저장할 때 이전 값이 폐기됩니다.",
            aKeyNeedsASecret: "키에는 시크릿이 필요합니다.",
            deleteThisKey: "이 키 삭제",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "삭제하면 키가 즉시 폐기되어, 키를 보유한 클라이언트는 401 응답을 받기 시작합니다. 모델 접근 규칙은 제거되고 기록된 사용량은 남습니다.",
        },
        apiKeyNewScreen: {
            createAPIKey: "API 키 만들기",
            oneSecretYourServingClientsShareMessage:
                "서비스 클라이언트가 함께 사용하는 하나의 시크릿입니다.",
            apiKeysBackLabel: "API 키",
            key: "키",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "시크릿은 영문자, 숫자, 기호로만 구성되며 공백은 포함할 수 없고 다른 키와 중복될 수 없습니다.",
            enabled: "사용",
            loadModelAccessInventoryBeforeCreatingAKeyMessage:
                "키를 만들기 전에 모델 접근 목록을 먼저 불러오세요.",
        },
        apiKeysScreen: {
            apiKeys: "API 키",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "서비스 키입니다. OpenAI 클라이언트는 Authorization: Bearer <key>로, Gemini는 x-goog-api-key로, Anthropic은 x-api-key로 전송합니다. 키를 열면 시크릿과 사용량을 볼 수 있습니다.",
            createAPIKey: "API 키 만들기",
            noAPIKeysYet: "API 키가 아직 없습니다",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "새 게이트웨이는 첫 키가 생기기 전까지 아무것도 서비스하지 않습니다.",
            searchAPIKeys: "API 키 검색",
            searchByNameOrKey: "이름 또는 키로 검색",
            theSecretHere: "여기의 시크릿",
        },
        bulkPublicationScreen: {
            createAliases: "별칭 만들기",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "물리 모델을 선택하고 공개 ID를 지정합니다. 선택할 때마다 사용 설정된 직접 라우트가 하나씩 생성되며, 이름 충돌은 이름을 바꾸거나 덮어쓰지 않고 별도로 보고됩니다.",
            inventoryBackLabel: "모델 목록",
            noPhysicalModelsToAlias: "별칭을 만들 물리 모델이 없습니다",
            discoverOrAddInventoryFirstMessage: "먼저 모델을 탐색하거나 추가하세요.",
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "다른 선택된 행이 이 공개 ID를 사용합니다. 서로 다른 이름을 분명히 지정하세요.",
            openRoute: "라우트 열기",
            createSelectedAliases: "선택한 별칭 만들기",
        },
        homeScreen: {
            overview: "개요",
            theHealthOfThisGatewayAtAGlanceMessage: "게이트웨이의 상태를 한눈에 보여줍니다.",
            createProvider: "공급자 만들기",
            providers: "공급자",
            publicRoutes: "공개 라우트",
            inventoryReady: "사용 가능한 모델",
            apiKeys: "API 키",
            recentRequests24h: "최근 요청(24시간)",
            recentErrors24h: "최근 오류(24시간)",
            requestsPerDay: "일별 요청 수",
            theLast14DaysInUTCTheGatewaysMessage:
                "최근 14일, UTC 기준 — 게이트웨이 자체의 일자 경계입니다.",
            openUsage: "사용량 열기",
            storedSettingsReviewedMessage: "저장된 설정을 확인했습니다.",
            needsAttention: "확인 필요",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "사용 가능한 모델이 없습니다. 사용 설정된 공급자에서 모델을 탐색하세요.",
            noAPIKeysYetMessage: "API 키가 아직 없습니다.",
            endpoints: "엔드포인트",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "모든 인터페이스가 모든 모델을 서비스합니다. OpenAI, Gemini, Anthropic 형식의 클라이언트를 이 인스턴스로 지정하기만 하면 됩니다.",
            theURLBesideIt: "옆의 URL",
            noProvidersYet: "공급자가 아직 없습니다",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "공급자 하나는 업스트림 계정 하나입니다. 모델은 탐색할 때 들어옵니다.",
            createYourFirstProvider: "첫 공급자 만들기",
            connectedToTheGateway: "게이트웨이에 연결됨",
            discoverModels: "모델 탐색",
            createAPIKey: "API 키 만들기",
            openAndDiscover: "열고 탐색",
            reviewModels: "모델 검토",
            credentialNotFresh: "자격 증명이 유효했던 적 없음",
            credentialExpired: "자격 증명 만료됨",
            reconnect: "다시 연결",
            openProvider: "공급자 열기",
            createAProvider: "공급자 만들기",
            discoverItsModelsFromUpstream: "업스트림에서 모델 탐색",
            issueAnAPIKeyForServingClients: "서비스 클라이언트용 API 키 발급",
            pointAClientAtAnEndpointBelow: "아래 엔드포인트로 클라이언트 지정",
        },
        modelDetailScreen: {
            model: "모델",
            upstreamOnProvider: "{{provider}}의 {{upstreamId}}.",
            modelsBackLabel: "모델",
            capabilities: "기능",
            whatTheModelListingsDeclareForThisModelMessage:
                "모델 목록이 이 모델에 대해 선언한 내용입니다. 여기의 값은 요청을 제한하지 않으며, 응답은 업스트림이 결정합니다.",
            reasoningLevels: "추론 수준",
            maxInputTokens: "최대 입력 토큰",
            maxOutputTokens: "최대 출력 토큰",
            compaction: "압축",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "이 행 자체의 압축 정보입니다. 재정의가 있으면 재정의를, 없으면 최근 탐색 결과를 사용합니다. 둘 다 밝히지 않은 부분은 정의의 선언이 결정합니다.",
            noLevels: "수준 없음",
            addAlias: "별칭 추가",
            editModel: "모델 편집",
            details: "세부 정보",
            publicModelNames: "공개 모델 이름",
            directRoutes: "직접 라우트",
            upstreamID: "업스트림 ID",
            provider: "공급자",
            discovery: "탐색",
            status: "상태",
        },
        modelEditScreen: {
            editModel: "모델 편집",
            modelSettings: "모델 설정",
            enabled: "사용",
            deleteThisModel: "이 모델 삭제",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "삭제하면 이 물리 모델과 그 직접 라우트가 제거됩니다. 계정 풀 라우트는 저장된 상태로 남습니다.",
        },
        modelNewScreen: {
            createModel: "모델 만들기",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "탐색으로는 나오지 않는 모델을 추가하거나, 업스트림이 더 이상 보고하지 않는 모델을 되찾아옵니다.",
            noProvidersYet: "공급자가 아직 없습니다",
            aModelBelongsToAProviderAndThereMessage:
                "모델은 공급자에 속해야 하는데, 이를 담을 공급자가 없습니다. 공급자를 먼저 추가하세요. 탐색으로 이 모델이 이미 나올 수도 있습니다.",
            createProvider: "공급자 만들기",
            aModelAddedByHandIsNeverRemovedMessage:
                "직접 추가한 모델은 자동으로 제거되지 않으며, 탐색에서 놓친 모델을 되찾아옵니다.",
            models: "모델",
            identity: "식별 정보",
            provider: "공급자",
            upstreamID: "업스트림 ID",
            modelSettings: "모델 설정",
            enabled: "사용",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "이 모델 이름은 자동으로 관리됩니다. 소유한 공급자 또는 계정 풀의 접두사를 편집해 변경하세요.",
            deleteThisPublicRoute: "이 공개 라우트 삭제",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "이 공개 ID를 요청하는 클라이언트는 모델 없음 응답을 받습니다. 물리 모델 목록과 계정 풀은 저장된 상태로 남습니다.",
            publicRouteSettings: "공개 라우트 설정",
            enabled: "사용",
            publicID: "공개 ID",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "클라이언트가 요청하는 정확한 모델 이름입니다. 이름을 바꾸면 클라이언트도 업데이트해야 합니다.",
            targetType: "대상 유형",
            pool: "계정 풀",
            upstreamID: "업스트림 ID",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "모든 구성원에게 요청하는 정확한 식별자입니다. 모델이 없어도 게시는 막지 않습니다.",
            physicalModel: "물리 모델",
            createPublicRoute: "공개 라우트 만들기",
        },
        modelsScreen: {
            models: "모델",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "모델은 독립 공급자 접두사 또는 소유한 계정 풀 접두사를 통해 자동으로 노출됩니다.",
            createAliases: "별칭 만들기",
            createModel: "모델 만들기",
            noModelsYet: "모델이 아직 없습니다",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "공급자를 먼저 추가한 뒤 모델을 탐색하거나, 직접 하나 추가하세요.",
            thisProviderHoldsNoModels: "이 공급자에는 모델이 없습니다.",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "이 공급자는 더 이상 목록에 없습니다 — 공급자 필터에서 다른 공급자를 선택하세요.",
            createProvider: "공급자 만들기",
            searchModels: "모델 검색",
            searchByUpstreamIDOrProviderName: "업스트림 ID 또는 공급자 이름으로 검색",
            provider: "공급자",
        },
        notFoundScreen: {
            pageNotFound: "페이지를 찾을 수 없음",
            goToOverview: "개요로 이동",
            providers: "공급자",
            models: "모델",
        },
        oauthFlowView: {
            flowExpires: "흐름 만료",
            singleuseStateLabel: "일회용 state:",
            authorizationURL: "인증 URL",
            openTheAuthorizationPage: "인증 페이지 열기",
            theURLAbove: "위의 URL",
            copyTheURL: "URL 복사",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "state는 한 번만 사용할 수 있고 흐름은 곧 만료되므로, 접근을 승인한 뒤 신속히 마무리하세요.",
            mostLoginsEndOnAPageThatShowsMessage:
                "대부분의 로그인은 아무 내용도 표시하지 않는 페이지에서 끝납니다. 이는 정상입니다. 브라우저에 표시된 주소 전체를 붙여넣으세요. 페이지가 열리고 코드가 표시되면 그 코드를 대신 붙여넣으세요.",
            startOver: "다시 시작",
            callbackURLOrAuthorizationCode: "콜백 URL 또는 인증 코드",
            pasteTheWholeCallbackURL: "콜백 URL 전체 붙여넣기",
            verificationCode: "인증 코드",
            theVerificationCode: "인증 코드",
            copyTheCode: "코드 복사",
            openTheVerificationPage: "인증 페이지 열기",
            theVerificationURL: "인증 URL",
            approveTheCodeAtTheVerificationPageThisMessage:
                "인증 페이지에서 코드를 승인하세요. 이 페이지는 게이트웨이에 결과를 계속 질의하며, 흐름이 완료되는 즉시 다음으로 넘어갑니다.",
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "흐름이 완료될 때까지 이 페이지에 머무르세요 — 콘솔 안에서 이동하면 흐름이 폐기됩니다.",
        },
        poolAvailabilityCard: {
            refreshQuota: "할당량 새로 고침",
            resetRestrictions: "제한 초기화",
            quotaExhausted: "할당량 소진",
            poolAvailability: "계정 풀 가용성",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "저장된 관측값입니다. 새로 고침은 업스트림 계정에 직접 질의합니다. 초기화는 제한을 해제하고 새로 고침 유예를 설정하며, 업스트림 할당량은 초기화하지 않습니다.",
            thisPoolHasNoMembersMessage: "이 계정 풀에는 구성원이 없습니다.",
        },
        providerDetailScreen: {
            provider: "공급자",
            editProvider: "공급자 편집",
            editPage: "편집 페이지",
            storedAvailability: "저장된 가용성",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "이 계정에서 관측된 모든 제한입니다. 초기화는 제한을 해제하고 새로 고침 유예를 설정하며, 업스트림 할당량은 초기화하지 않습니다.",
            details: "세부 정보",
            modelPrefix: "모델 접두사",
            type: "유형",
            definition: "정의",
            upstreamBaseURL: "업스트림 기본 URL",
            credentials: "자격 증명",
            status: "상태",
            discoveredFromUpstreamOrAddedByHandMessage:
                "업스트림에서 탐색했거나 직접 추가한 모델입니다.",
            createModel: "모델 만들기",
            allModels: "모든 모델",
        },
        providerEditScreen: {
            editProvider: "공급자 편집",
            settings: "설정",
            credentials: "자격 증명",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "OAuth 자격 증명은 만료되며, 다시 연결하면 교체됩니다.",
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "이 공급자의 정의는 기본 URL을 기준으로 요청을 평가합니다 — URL을 복원하거나 되돌리기를 사용하세요.",
            deleteThisProvider: "이 공급자 삭제",
        },
        providerNewScreen: {
            createProvider: "공급자 만들기",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "업스트림 계정 하나입니다. 프리셋을 선택하면 자격 증명 경로가 그에 맞춰 정해집니다.",
            providersBackLabel: "공급자",
            readAgain: "다시 읽기",
            preset: "프리셋",
            switchPreset: "프리셋 전환",
            keepMyEdit: "내 편집 유지",
            identity: "식별 정보",
            connection: "연결",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage:
                "키는 쓰기 전용이며 게이트웨이는 값을 반환하지 않습니다.",
            connectTheAccount: "계정 연결",
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "인증이 진행되는 동안에는 잠깁니다 — 공급자는 흐름 시작 시 이미 전송한 값으로 만들어집니다.",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "흐름이 안내하는 곳에서 접근을 승인한 뒤 완료되도록 두세요. 이 앱은 자격 증명을 저장하지 않습니다.",
            upstreamBaseURL: "업스트림 기본 URL",
            definition: "정의",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "게이트웨이가 저장하는 연결 레시피입니다. 저장된 정의의 JSON을 편집하면 다음 버전이 등록되며, 저장된 버전은 다시 쓰이지 않습니다.",
            notStoredYetSubmittingCreatesVersion1Message:
                "아직 저장되지 않았습니다. 제출하면 버전 1이 생성됩니다.",
            advancedTheConnectionRecipePrefilledByThePreset:
                "고급: 연결 레시피 — 프리셋으로 미리 채워짐",
            definitionJSON: "정의 JSON",
            resetToPreset: "프리셋으로 초기화",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "연결 레시피가 잘못되었습니다. 위의 고급 항목을 열어 문제가 있는 경로를 확인하세요.",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "동일한 업스트림 서비스를 위한 교체 가능한 계정으로, 우선순위 순서대로 시도합니다.",
            inspectUpstreamID: "업스트림 ID 확인",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "저장된 구성원 자격을 확인합니다. 비워 두면 계정 전체 제한만 표시됩니다.",
            deleteThisPool: "이 계정 풀 삭제",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "삭제하면 자동 계정 풀 항목이 제거되고 구성원의 독립 접두사가 복원됩니다. 직접 작성한 라우트는 먼저 제거하거나 대상을 바꿔야 합니다.",
            poolSettings: "계정 풀 설정",
            enabled: "사용",
            name: "이름",
            modelPrefix: "모델 접두사",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "계정 풀은 구성원 모델을 이 접두사 + 업스트림 ID로 자동 노출합니다. 비워 두면 정확한 업스트림 ID를 사용합니다. 접두사를 변경하면 기존 자동 항목의 이름이 바뀝니다.",
            accountsInPriorityOrder: "우선순위 순서의 계정",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "구성원은 동일한 업스트림 서비스를 제공해야 합니다. 계정 풀에 속한 동안에는 각자의 접두사 항목이 숨겨집니다. 새 모델은 여기에 자동으로 나타나며, 해당 모델을 보유한 구성원에게만 라우팅됩니다. 빈 계정 풀도 허용됩니다.",
            moveUp: "위로 이동",
            moveDown: "아래로 이동",
            remove: "제거",
            addAccount: "계정 추가",
            addMember: "구성원 추가",
            createPool: "계정 풀 만들기",
        },
        providerQuotaCard: {
            upstreamQuota: "업스트림 할당량",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "허용량은 업스트림 계정에 속하며 다른 클라이언트의 사용량을 포함할 수 있습니다.",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage:
                "이 공급자 정의는 할당량 조회를 선언하지 않았습니다.",
            checkingWhetherThisDefinitionSupportsQuotaReads:
                "이 정의가 할당량 조회를 지원하는지 확인하는 중…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "할당량이 관측되지 않았습니다. 새로 고침은 업스트림 계정에 직접 질의합니다.",
            readingUpstreamQuota: "업스트림 할당량을 읽는 중…",
            showingThePreviousReadingMessage: "이전 읽기 결과를 표시합니다.",
            observed: "관측 시각",
            resetTimeReachedRefreshToUpdate:
                "초기화 시각에 도달했습니다, 새로 고침하여 업데이트하세요",
            theUpstreamReturnedNoQuotaEntriesMessage:
                "업스트림이 할당량 항목을 반환하지 않았습니다.",
        },
        providersScreen: {
            providers: "공급자",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "업스트림 계정입니다. 계정을 열면 모델과 사용량을 볼 수 있고, 편집은 그 안에서 한 단계 더 들어가면 됩니다.",
            createProvider: "공급자 만들기",
            noProvidersYet: "공급자가 아직 없습니다",
            aProviderIsOneUpstreamAccountMessage: "공급자 하나는 업스트림 계정 하나입니다.",
            searchProviders: "공급자 검색",
            searchByNameTypeOrFormat: "이름, 유형 또는 형식으로 검색",
        },
        readNotice: {
            readAgain: "다시 읽기",
        },
        routingListsScreen: {
            pools: "계정 풀",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "하나의 업스트림 서비스를 위한 교체 가능한 계정들로 구성한 이름 있는 모델 목록입니다.",
            createPool: "계정 풀 만들기",
            noPoolsYet: "계정 풀이 아직 없습니다",
            createANamedPoolWithAModelPrefixMessage:
                "모델 접두사를 지정해 이름 있는 계정 풀을 만드세요. 구성원 모델은 자동으로 노출됩니다.",
            noModelPrefix: "없음",
            publicRoutes: "공개 라우트",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "클라이언트가 요청하는 모델 이름입니다. 독립 공급자와 계정 풀은 모델 이름을 자동으로 노출합니다. 추가 별칭은 선택 사항입니다.",
            createPublicRoute: "공개 라우트 만들기",
            noPublicRoutesYet: "공개 라우트가 아직 없습니다",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "물리 모델을 직접 게시하거나, 계정 풀과 정확한 업스트림 ID를 대상으로 지정하세요.",
            automaticPrefix: "자동 · ",
        },
        screenFrame: {
            readAgain: "다시 읽기",
        },
        usageQueryScreen: {
            usageAggregates: "사용량 집계",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "필터 안의 차원 조합마다 버킷을 하나씩 만듭니다. 요청 수와 토큰 합계만 포함하며 내용은 포함하지 않습니다.",
            viewRequests: "요청 보기",
            question: "질의",
            groupBy: "그룹 기준",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "어떤 차원 조합이든 사용할 수 있습니다. 아무것도 선택하지 않으면 일치하는 모든 행을 합산합니다.",
            window: "기간",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "아래 범위를 채웁니다. 시작은 한 기간 전, 끝은 열린 상태로 두며, 현재 시각을 기준으로 계산됩니다. 입력 필드는 계속 직접 편집할 수 있습니다.",
            from: "시작",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "해당 시각을 포함하며 이 브라우저의 시간대를 기준으로 합니다(예: 2026-09-23T10:00). 비워 두면 범위를 지정하지 않습니다.",
            to: "끝",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "해당 시각을 제외하며 이 브라우저의 시간대를 기준으로 합니다(예: 2026-09-23T10:00). 비워 두면 범위를 지정하지 않습니다.",
            clearFilters: "필터 지우기",
            usageBackLabel: "사용량",
            theQueryDidNotRunAFilterValueIsInvalid:
                "질의가 실행되지 않았습니다 — 필터 값이 잘못되었습니다.",
            total: "합계",
            nothingRecordedInThisWindowMessage: "이 기간에는 기록된 내용이 없습니다.",
            noRowsInThisWindowMessage: "이 기간에는 행이 없습니다.",
            bucket: "버킷",
            requests: "요청",
            input: "입력",
            output: "출력",
            cached: "캐시됨",
            cacheWrite: "캐시 쓰기",
            reasoning: "추론",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "토큰 합계는 업스트림이 사용량을 보고하지 않은 행을 건너뜁니다. 없는 내역이 합계를 늘리거나 줄이지 않습니다.",
        },
        usageScreen: {
            usage: "사용량",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "발송된 모든 요청을 최신순으로 표시합니다. 숫자만 포함하며 내용은 포함하지 않습니다. 선택적 사용량 수치를 전혀 보고하지 않은 행은 그 수치들을 통째로 생략합니다. 모두 0인 값과 보고되지 않은 값은 여기서 구분할 수 없습니다.",
            viewAggregates: "집계 보기",
            filter: "필터",
            theSharedAccountingFilterTheSameOneTheMessage:
                "집계 페이지와 공유하는 사용량 필터입니다.",
            window: "기간",
            rollingResolvedWhenTheStreamIsReadMessage: "스트림을 읽을 때 확정되는 롤링 기간입니다.",
            clearFilters: "필터 지우기",
            noRequestsRecordedYet: "기록된 요청이 아직 없습니다",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "게이트웨이가 업스트림으로 보내는 순간, 발송된 서비스 요청마다 행 하나가 나타납니다. 트래픽이 흐르는데 아무것도 나타나지 않으면 게이트웨이 설정 파일의 api_logs 섹션을 확인하세요.",
            noRecordedRequestsMatchThisFilterMessage: "이 필터와 일치하는 기록된 요청이 없습니다.",
            filtersStayBehind: " — 기간과 나머지 모든 필터는 여기에 남습니다.",
            streamMark: " · 스트리밍",
            thisRowIsNoLongerListed: "이 행은 더 이상 목록에 없습니다.",
        },
        usageStats: {
            usage: "사용량",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "일별 요청 수와 토큰 수입니다. 숫자만 포함하며 내용은 포함하지 않습니다.",
            openAggregates: "집계 열기",
            viewRequests: "요청 보기",
            requests: "요청",
            dispatchedAllDays: "전체 기간 발송량",
            inputTokens: "입력 토큰",
            summedUsageReported: "사용량이 보고된 항목의 합계",
            outputTokens: "출력 토큰",
            day: "일",
            input: "입력",
            output: "출력",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage:
                "일자는 UTC 기준 — 게이트웨이 자체의 일자 경계입니다.",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage:
                "업스트림이 더 이상 이 모델을 보고하지 않습니다.",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "이 계정에 이미 정확히 그 업스트림 모델이 있습니다. 기존 모델 목록 행을 여세요.",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "다른 공개 라우트가 그 공개 ID를 사용하고 있습니다. 다른 이름을 분명히 지정하거나 기존 라우트를 편집하세요.",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "다른 계정 풀이 그 이름을 사용하고 있습니다. 다른 계정 풀 이름을 선택하세요.",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "공개 라우트가 아직 이 계정 풀을 참조합니다. 계정 풀을 삭제하기 전에 해당 라우트를 변경하거나 삭제하세요.",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne:
            "다른 키가 이미 그 시크릿을 사용하고 있습니다. 새로 생성하세요.",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "저장된 버전이 변경되었습니다. 위 막대의 새로 고침을 사용하세요. 저장된 버전을 다시 읽고 이 양식은 그대로 둡니다. 그런 다음 다시 제출하세요.",
        chooseAProviderNameNoOtherRowHolds: "다른 행이 사용하지 않는 공급자 이름을 선택하세요.",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "흐름이 끝나기 전에 다른 공급자가 이 이름을 가져갔습니다. 다시 시작하면 이 흐름이 종료되고 이름이 풀립니다. 그런 다음 다른 행이 사용하지 않는 이름으로 다시 만드세요.",
        chooseAnIdentifierNoOtherRowHolds: "다른 행이 사용하지 않는 식별자를 선택하세요.",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "공급자의 인증이 만료되었다면 공급자 편집 페이지에서 다시 연결하세요. 업스트림이 정말 아직 모델을 나열하지 않는다면 이 페이지에서 직접 하나 추가하세요.",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "공급자의 기본 URL이나 API 키가 잘못되었다면 공급자 편집 페이지에서 바로잡으세요. 업스트림이 정말 아직 모델을 나열하지 않는다면 이 페이지에서 직접 하나 추가하세요.",
        checkTheProvidersStateAndCredentials: "공급자의 상태와 자격 증명을 확인하세요.",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "공급자가 교환을 거부했습니다. 이 페이지에서 흐름을 다시 시작하세요.",
        checkTheProvidersCredentialsAndReachability:
            "공급자의 자격 증명과 연결 가능 여부를 확인하세요.",
        nothingAnsweredAtBaseUrl: "[[baseUrl]]에서 아무 응답도 없습니다.",
        theBrowserCouldNotCompleteTheConnection: "브라우저가 연결을 완료하지 못했습니다.",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "주소를 확인하고, 게이트웨이가 실행 중인지, 그리고 그 `listen` 주소가 이 브라우저에서 접근 가능한지 확인하세요.",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl:
            "HTTPS 페이지는 HTTP 주소 [[baseUrl]]를 호출할 수 없습니다.",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "이 페이지는 HTTPS로 제공되며, 브라우저가 HTTP 주소로의 호출을 원천적으로 차단하므로 게이트웨이에 요청이 닿지 않았습니다.",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "이 앱을 HTTP로도 제공하거나, 해당 인스턴스의 설정 파일에 `tls` 섹션을 추가해 게이트웨이가 HTTPS로 응답하도록 하세요.",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "게이트웨이가 응답했지만 브라우저가 [[baseUrl]]의 응답을 차단했습니다.",
        theBrowserWithheldTheResponseFromThisPage:
            "브라우저가 이 페이지에 응답을 전달하지 않았습니다.",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "해당 인스턴스의 설정 파일에서 `[[setting]]`에 [[origin]]을 추가하고 게이트웨이를 다시 시작하세요. 게이트웨이 자체 오리진에서 이 앱을 제공하면 허용 목록이 전혀 필요 없습니다.",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "주소를 확인하고, 게이트웨이가 실행 중인지, 이 브라우저에서 이 스킴으로 접근 가능한지 확인하세요.",
        noAnswerFromBaseUrl: "[[baseUrl]]에서 응답이 없습니다.",
        theBrowserReceivedNoAnswerItCouldRead: "브라우저가 읽을 수 있는 응답을 받지 못했습니다.",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "그 주소에서 수신 대기 중인 것이 없거나, 브라우저가 이 페이지가 읽기 전에 응답을 차단했습니다. `[[setting]]`에 [[origin]]을 추가하고 게이트웨이를 다시 시작하세요.",
        theGatewayRejectedThatManagementToken: "게이트웨이가 그 관리 토큰을 거부했습니다.",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "토큰은 해당 인스턴스 설정 파일의 `management_api.token`과 일치해야 합니다. 게이트웨이는 이를 실시간으로 읽으므로 토큰을 바꿔도 다시 시작할 필요가 없습니다.",
        theGatewayHoldsNoSuchRow: "게이트웨이에 그런 행이 없습니다.",
        theGatewayRefusedAnIdentifierIsAlreadyTaken:
            "게이트웨이가 거부했습니다. 식별자가 이미 사용 중입니다.",
        theUpstreamProviderRefusedTheCallOrWasUnreachable:
            "업스트림 공급자가 호출을 거부했거나 연결할 수 없었습니다.",
        theGatewayFailedInternally: "게이트웨이 내부에서 오류가 발생했습니다.",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "원인은 게이트웨이의 stderr 로그에 있습니다. 내부 원인은 이 페이지로 반환되지 않습니다.",
        theGatewayRejectedTheRequest: "게이트웨이가 요청을 거부했습니다.",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow:
            "게이트웨이가 이 빌드가 알지 못하는 형식으로 응답했습니다.",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "이 빌드의 생성된 바인딩과 게이트웨이 관리 API가 일치하지 않습니다. 바인딩을 다시 생성하거나 일치하는 리비전에서 이 프런트엔드를 다시 빌드하세요.",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "게이트웨이가 관리 오류 본문 없이 HTTP {{httpStatus}}로 응답했습니다.",
        checkTheGatewaysStderrLogForTheCause: "게이트웨이의 stderr 로그에서 원인을 확인하세요.",
        theCallFailed: "호출에 실패했습니다.",
    },
    notices: {
        savedName: "[[name]] 저장이 완료되었습니다.",
        credentialsReplaced: "자격 증명을 교체했습니다.",
        expiryReplaced: "공급자 목록에 새 만료 시각이 반영되었습니다.",
        keyReplaced: "업스트림 API 키를 교체했습니다.",
        previousSecretRevoked:
            "이번 저장으로 이전 시크릿이 작동을 멈췄습니다. 해당 시크릿을 보유한 모든 클라이언트를 업데이트하세요.",
        lastKeyDisabled:
            "마지막으로 사용 설정된 키였습니다. 키를 다시 사용 설정할 때까지 게이트웨이는 아무것도 서비스하지 않습니다.",
        modelSaved: "모델을 저장했습니다.",
        poolSaved: "계정 풀을 저장했습니다.",
        routeSaved: "공개 라우트를 저장했습니다.",
        discovered_other: "[[providerName]]에서 모델 [[quantity]]개를 탐색했습니다.",
        nothingDiscovered: "[[providerName]]에서 새로 탐색된 항목이 없습니다.",
        manualUntouched_other: "직접 추가한 {{quantity}}개는 그대로 두었습니다.",
        vanished_other: "{{quantity}}개 모델이 이제 더 이상 보고되지 않는 것으로 표시됩니다.",
    },
    connectionValidation: {
        empty: "JanusGate 기본 URL을 입력하세요.",
        notAbsolute: "절대 URL을 입력하세요. 예: https://gateway.example.com",
        scheme: "http:// 또는 https:// URL을 사용하세요.",
        credentials: "URL에서 자격 증명을 제거하세요.",
        notOrigin: "경로, 쿼리, 프래그먼트 없이 게이트웨이 오리진만 입력하세요.",
        managementToken: "관리 토큰을 입력하세요.",
        nameEmpty: "이름은 비워 둘 수 없습니다.",
    },
    additional: {
        app: {
            readThisScreensStateFromTheGatewayAgain:
                "게이트웨이에서 이 화면의 상태를 다시 읽습니다",
            thisScreenReadsNothingFromTheGatewaySo:
                "이 화면은 게이트웨이에서 아무것도 읽지 않으므로 새로 고칠 내용이 없습니다",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "연결을 끊고 저장된 토큰을 지울까요? 다시 연결하려면 토큰을 다시 입력해야 합니다.",
        },
        apiKeyAccess: {
            allow: "허용",
            deny: "거부",
            provider: "공급자",
            pool: "계정 풀",
            automatic: "자동",
            alias: "별칭",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "필수. 이 키를 사용하는 클라이언트입니다.",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "필수. 서비스 클라이언트가 모든 인터페이스에서 이 키로 인증합니다.",
        },
        clipboard: {
            copyToClipboard: "클립보드에 복사",
            copied: "복사됨",
            copy: "복사",
            theValueHere: "여기의 값",
        },
        confirmDeleteCard: {
            deleting: "삭제 중…",
            deleteForGood: "완전히 삭제",
        },
        dirtyFormFooter: {
            saving: "저장 중…",
            saveChanges: "변경 사항 저장",
        },
        layout: {
            back: "뒤로",
        },
        modelForm: {
            followingDiscoveryCurrentlyUnknownSoListingsOmitItMessage:
                "탐색 값을 따르는 중 — 현재 알 수 없어 목록에서 생략됩니다.",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "탐색 값을 따르는 중 — 현재 알려진 값이 없어 정의의 선언이 결정합니다.",
        },
        providerForm: {
            verifiedLive: "실시간 확인됨",
            unverified: "미확인",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "필수. 이 정의의 요청이 향하는 업스트림 주소입니다.",
            upstreamAPIKey: "업스트림 API 키",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "필수. 모든 요청과 함께 업스트림으로 전송됩니다. 게이트웨이는 값을 반환하지 않습니다.",
            showTheAPIKeyWhileTyping: "입력하는 동안 API 키 표시",
        },
        providersTable: {
            on: "켜짐",
            off: "꺼짐",
        },
        usageFilterControls: {
            allEndpoints: "모든 엔드포인트",
        },
        apiKeyDetailScreen: {
            apiKeys: "API 키",
            apiKey: "API 키",
            createAPIKey: "API 키 만들기",
            enabled: "사용",
            disabled: "사용 안 함",
        },
        apiKeyEditScreen: {
            apiKey: "API 키",
            editAPIKey: "API 키 편집",
            showTheCurrentKey: "현재 키 표시",
        },
        apiKeyNewScreen: {
            creating: "만드는 중…",
            createAPIKey: "API 키 만들기",
        },
        apiKeysScreen: {
            on: "켜짐",
            off: "꺼짐",
        },
        bulkPublicationScreen: {
            publicIDIsRequiredMessage: "공개 ID는 필수입니다.",
            noDirectRoutes: "직접 라우트 없음",
            modelVanishedItsNewRouteWillBeSuspended: "모델이 사라짐 — 새 라우트가 일시 중단됩니다",
            modelEnabled: "모델 사용 설정됨",
            modelDisabled: "모델 사용 안 함",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "입력하는 동안 토큰 표시",
            probingTheGateway: "게이트웨이 확인 중…",
            connect: "연결",
        },
        homeScreen: {
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "할당량 가용성은 공개 라우트에서 확인하세요. 이 수치는 저장된 스위치와 모델 목록을 나타냅니다.",
            noRequestsRecordedMessage: "기록된 요청이 없습니다.",
        },
        modelDetailScreen: {
            models: "모델",
            model: "모델",
            overriddenByAdministratorMessage: "관리자가 재정의했습니다.",
            fromTheLatestDiscoveryMessage: "최근 탐색 결과입니다.",
            unknownOmittedFromModelListingsMessage: "알 수 없음 — 모델 목록에서 생략됩니다.",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "이 행에는 정보가 없습니다 — 정의의 선언이 결정합니다.",
            unknown: "알 수 없음",
            noAutomaticEntry: "자동 항목 없음",
            enabled: "사용",
            disabled: "사용 안 함",
        },
        modelEditScreen: {
            model: "모델",
            editModel: "모델 편집",
        },
        modelNewScreen: {
            aModelBelongsToOneProviderMessage: "필수. 모델은 공급자 하나에 속합니다.",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "필수. 업스트림으로 그대로 전송되며 영문자, 숫자, 기호만 사용할 수 있습니다.",
            carriedHereFromAModelTheUpstreamNoMessage:
                "필수. 업스트림이 더 이상 보고하지 않는 모델에서 가져온 값입니다. 제출하면 아래 설정에 따라 그 모델을 되찾아옵니다.",
            creating: "만드는 중…",
            createModel: "모델 만들기",
        },
        modelRouteEditorScreen: {
            createPublicRoute: "공개 라우트 만들기",
            editPublicRoute: "공개 라우트 편집",
            publicRoutes: "공개 라우트",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "공급자 또는 계정 풀이 자동으로 관리하는 모델 이름입니다.",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "클라이언트가 이 공개 ID를 요청합니다. 대상을 바꿔도 이 이름은 그대로 유지됩니다.",
            openPool: "계정 풀 열기",
            openPhysicalModel: "물리 모델 열기",
            deleteThisPublicRoute: "이 공개 라우트를 삭제할까요?",
            publicIDIsRequiredMessage: "공개 ID는 필수입니다.",
            publicID: "공개 ID",
            directPhysicalModel: "직접 물리 모델",
            pool: "계정 풀",
            chooseAPool: "계정 풀 선택",
            upstreamIDIsRequiredMessage: "업스트림 ID는 필수입니다.",
            upstreamID: "업스트림 ID",
            chooseAPhysicalModel: "물리 모델 선택",
        },
        modelsScreen: {
            noModelsOnAnyProviderYetDiscoverOrMessage:
                "어떤 공급자에도 아직 모델이 없습니다 — 탐색하거나 하나 추가하세요.",
        },
        poolAvailabilityCard: {
            readingStoredAvailability: "저장된 가용성을 읽는 중…",
            quotaNotObserved: "할당량이 관측되지 않음",
            quotaReadsUnsupported: "할당량 조회를 지원하지 않음",
            administratorResetRefreshGraceInstalled: "관리자가 초기화함, 새로 고침 유예 설정됨",
            accountEnabled: "계정 사용 설정됨",
            updatingAvailability: "가용성을 업데이트하는 중…",
            allModels: "모든 모델",
            resetUnknown: "초기화 시각 알 수 없음",
            resetTimePassedRefreshRequired: "초기화 시각이 지남, 새로 고침 필요",
        },
        providerDetailScreen: {
            providers: "공급자",
            provider: "공급자",
            createProvider: "공급자 만들기",
            enabled: "사용",
            disabled: "사용 안 함",
            theOAuthCredentialsHaveExpiredMessage: "OAuth 자격 증명이 만료되었습니다.",
            noneExactUpstreamIDs: "없음 — 정확한 업스트림 ID 사용",
            discovering: "탐색 중…",
            discoverModels: "모델 탐색",
            askingTheUpstreamWhatItReports: "업스트림이 보고하는 내용을 묻는 중…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "이 공급자에는 아직 모델이 없습니다 — 공급자가 보고하는 모델을 탐색하거나 직접 하나 추가하세요.",
        },
        providerEditScreen: {
            provider: "공급자",
            editProvider: "공급자 편집",
            modelPrefix: "모델 접두사",
            aProviderNeedsANameMessage: "공급자에는 이름이 필요합니다.",
            starting: "시작하는 중…",
            reconnectTheAccount: "계정 다시 연결",
            replacing: "교체하는 중…",
            replaceTheCredentials: "자격 증명 교체",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "삭제하면 이 계정과 계정 풀 구성원 자격이 제거됩니다. 이 계정에는 물리 모델이 없습니다.",
        },
        providerNewScreen: {
            modelPrefix: "모델 접두사",
            creating: "만드는 중…",
            createProvider: "공급자 만들기",
            connecting: "연결하는 중…",
            connectTheAccount: "계정 연결",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "필수. 이 정의의 요청이 평가 기준으로 삼는 업스트림 주소입니다. 주소가 없으면 게이트웨이는 흐름 시작을 거부합니다.",
            starting: "시작하는 중…",
            startTheAuthorization: "인증 시작",
        },
        providerPoolEditorScreen: {
            createPool: "계정 풀 만들기",
            editPool: "계정 풀 편집",
            pools: "계정 풀",
            deleteThisPoolItsMembershipIsRemovedMessage:
                "이 계정 풀을 삭제할까요? 구성원 자격이 제거됩니다.",
            nameIsRequiredMessage: "이름은 필수입니다.",
            modelPrefix: "모델 접두사",
            chooseAnAccount: "계정 선택",
        },
        providerQuotaCard: {
            periodUnknown: "기간 알 수 없음",
            monthly: "월간",
            unknown: "알 수 없음",
            unlimited: "무제한",
            resetUnknown: "초기화 시각 알 수 없음",
            readingQuota: "할당량을 읽는 중…",
            refreshingQuota: "할당량을 새로 고치는 중…",
            refreshQuota: "할당량 새로 고침",
            retryQuota: "할당량 다시 시도",
        },
        routingListsScreen: {
            on: "켜짐",
            off: "꺼짐",
        },
        usageQueryScreen: {
            querying: "질의하는 중…",
            runQuery: "질의 실행",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "필터나 범위가 질의를 좁히지 않으므로, 빈 답변은 게이트웨이의 전체 기록입니다. 아직 사용량이 없습니다.",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "이 필터와 일치하는 기록이 없습니다. 범위를 넓히거나 필터를 제거하거나 필터 없이 질의하세요. 필터 지우기는 위 양식에 있습니다.",
        },
        usageScreen: {
            reading: "읽는 중…",
            applyFilters: "필터 적용",
            widenTheWindowDropAFilterOrReadMessage:
                "기간을 넓히거나, 필터를 제거하거나, 스트림 전체를 읽으세요. 필터 지우기는 위 양식에 있습니다.",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "게이트웨이의 필터는 행을 찾을 뿐 값을 검증하지 않으므로, 알 수 없는 ID나 기록된 적 없는 상태는 그저 아무것도 일치하지 않습니다.",
            thatIsEveryRowThisFilterMatchesMessage: "이 필터와 일치하는 모든 행입니다.",
            thatIsEveryRecordedRowMessage: "기록된 모든 행입니다.",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "읽기 주기가 스트림을 교체하는 중입니다 — 주기가 끝난 뒤 더 불러오세요.",
            loading: "불러오는 중…",
            loadMore: "더 불러오기",
        },
    },
    operationNotices: {
        flowFailed: "흐름이 실패했습니다.",
        freshFlow: "새 흐름을 시작하고 신속히 인증하세요.",
        flowExpired: "흐름이 만료되었습니다.",
        flowExpiryPassed: "이 페이지가 결과를 기다리는 동안 흐름의 만료 시각이 지났습니다.",
        pollStopped: "결과 조회가 계속 실패하여 이 페이지는 질의를 중단했습니다.",
        pollFailed: "마지막 결과 조회가 실패했습니다.",
        invalidDefinition: "정의 JSON이 게이트웨이가 저장할 수 있는 정의가 아닙니다.",
        fixDefinition: "JSON을 수정하고 다시 제출하세요. 아무것도 기록되지 않았습니다.",
        fixFilter: "해당 필드를 수정한 뒤 질의를 다시 실행하세요.",
        discoveryPending: "[[providerName]]에서 모델을 탐색하는 중…",
        discoveryQuery: "업스트림이 지금 보고하는 내용을 묻는 중입니다.",
    },
    counts: {
        models_other: "모델 {{total}}개",
        filteredModels_other: "전체 {{total}}개 중 {{shown}}개",
        enabledKeys_other: "전체 {{total}}개 키 중 {{enabled}}개 사용 설정됨",
        shownKeys_other: "키 {{shown}}개 표시",
        filteredKeys_other: "[[shownSentence]] · 전체 [[total]]개 키 중 [[enabled]]개 사용 설정됨",
        providerDeleted_other:
            "삭제하면 계정과 계정 풀 구성원 자격, 그리고 직접 라우트를 거느린 물리 모델 {{total}}개가 제거됩니다. 계정 풀 라우트는 저장된 상태로 남습니다.",
        requests_other: "요청 {{total}}건",
        requestsPeak_other: "요청 [[total]]건 · [[day]] 최다 [[peak]]건",
    },
    helpers: {
        accessControl: {
            allowed: "허용됨",
            denied: "거부됨",
            modelOverride: "모델 재정의",
            globallyDisabled: "전역 사용 안 함",
            targetUnavailable: "대상 사용 불가",
            targetVanished: "대상 사라짐",
            globallyAvailable: "전역 사용 가능",
            noAvailablePoolMembers: "사용 가능한 계정 풀 구성원 없음",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "기본 URL은 공백이 없는 http:// 또는 https:// 주소여야 합니다.",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "정적 키를 사용하는 OpenAI Responses 프로토콜 엔드포인트입니다.",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "정적 키를 사용하는 Chat Completions 프로토콜 엔드포인트입니다.",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "정적 키를 사용하는 Gemini GenerateContent 프로토콜 엔드포인트입니다.",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "정적 키를 사용하는 Gemini Interactions 프로토콜 엔드포인트입니다.",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "정적 키를 사용하는 Anthropic Messages 프로토콜 엔드포인트입니다.",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "정적 키를 사용하는 xAI Responses 엔드포인트입니다. 네이티브 압축은 독립형 `/responses/compact` 작업을 사용하며, 명시적 트리거와 자동 압축은 지원되지 않는 것으로 선언됩니다. 독립형 압축 엔드포인트가 있다고 해서 네이티브 트리거나 자동 압축이 지원되는 것은 아닙니다.",
            claudeCodeClaudeAccount: "Claude Code(Claude 계정)",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "Messages API를 사용하는 Claude 구독 계정입니다. OAuth 토큰은 추론과 모델 탐색 모두에서 `oauth-2025-04-20` 베타 헤더와 함께 `Authorization: Bearer`로 전송됩니다.",
            kimiCodeKimiAccount: "Kimi Code(Kimi 계정)",
            githubCopilotGitHubAccount: "GitHub Copilot(GitHub 계정)",
            devinDevinAccount: "Devin(Devin 계정)",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "ConnectRPC 토큰 교환으로 정적 API 키와 API 서버 URL을 받는 Devin 계정입니다. 갱신도 만료도 없으며, 모델은 직접 추가합니다.",
            codexChatGPTAccount: "Codex(ChatGPT 계정)",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "Codex Responses 엔드포인트를 사용하는 ChatGPT 계정입니다. 업스트림으로 스트리밍이 강제되며 응답은 OpenAI Responses 형식으로 렌더링됩니다. 압축 섹션은 Codex 백엔드가 허용하는 명시적 트리거와 자동 임계값 압축을 선언합니다.",
            antigravityGoogleAccount: "Antigravity(Google 계정)",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "Antigravity Gemini 엔드포인트를 사용하는 Google 계정으로, 프로젝트 온보딩과 함께 GenerateContent 요청이 봉투로 감싸집니다.",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "정적 키를 사용하는 OpenAI Chat Completions 프로토콜의 Zen입니다.",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "정적 키를 사용하는 OpenAI Responses 프로토콜의 Zen입니다.",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "정적 키를 사용하는 Anthropic Messages 프로토콜의 Zen입니다.",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "정적 키를 사용하는 Gemini GenerateContent 프로토콜의 Zen입니다.",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "붙여넣은 내용에는 코드만 있지도, 코드가 담긴 URL도 아닙니다 — 로그인이 끝난 주소 전체 또는 코드만 복사해 여기에 붙여넣으세요.",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "흐름은 계속 진행 중이며, 이 페이지는 결과를 계속 질의합니다.",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "게이트웨이에 이 계정 풀이 없습니다. 삭제되었거나 주소를 잘못 입력했습니다.",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "게이트웨이에 이 공개 라우트가 없습니다. 삭제되었거나 주소를 잘못 입력했습니다.",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "게이트웨이에 이 API 키가 없습니다. 삭제되었거나 주소를 잘못 입력했습니다. 이 페이지에서는 되돌릴 수 없습니다.",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "게이트웨이에 이 모델이 없습니다. 삭제되었거나 주소를 잘못 입력했습니다 — 삭제된 탐색 모델은 다음 탐색에서 새 모델로 돌아옵니다.",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "게이트웨이에 이 공급자가 없습니다. 삭제되었거나 주소를 잘못 입력했습니다. 이 페이지에서는 되돌릴 수 없습니다.",
        },
        usageLabels: {
            streamed: "스트리밍",
            nonstreamed: "비스트리밍",
        },
        apiKeyAccess: {
            inherit: "상속",
            allow: "허용",
            deny: "거부",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels:
                "탐색 값을 따르는 중 — 현재 선언된 수준이 없습니다.",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "탐색된 집합을 대체합니다 — 아무것도 선택하지 않으면 수준을 선언하지 않습니다.",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "탐색 값을 따르는 중 — 현재 알 수 없어 목록에서 생략됩니다.",
            replacesTheDiscoveredValueWithAPositiveInteger: "탐색된 값을 양의 정수로 대체합니다.",
            unset: "설정 안 됨",
            discoveredNothingKnown: "탐색 결과: 알려진 내용 없음.",
        },
        modelsTable: {
            ready: "준비됨",
            off: "꺼짐",
            suspended: "일시 중단됨",
            heldByProvider: "공급자가 보유",
            unknown: "알 수 없음",
            noAutomaticEntry: "자동 항목 없음",
            noDirectRoutes: "직접 라우트 없음",
        },
        providersTable: {
            valid: "유효",
            expired: "만료됨",
            stale: "오래됨",
        },
        usageFilterControls: {
            allOutcomes: "모든 결과",
            failuresUpstreamOrInternal: "실패(업스트림 또는 내부)",
            any: "전체",
            streamed: "스트리밍",
            nonstreamed: "비스트리밍",
        },
        usageFilter: {
            allTime: "전체 기간",
            last24Hours: "최근 24시간",
            last7Days: "최근 7일",
            last30Days: "최근 30일",
            invalidStatusList:
                "HTTP 상태는 200, 429 같은 일반 정수를 쉼표로 구분해서 입력해야 하며, 비워 두면 상태로 좁히지 않습니다.",
        },
    },
    validation: {
        composition: {
            upstreamId:
                "업스트림 ID는 영문자, 숫자, 기호만 사용할 수 있으며 공백이나 비ASCII 문자는 사용할 수 없습니다.",
            publicId:
                "공개 ID는 영문자, 숫자, 기호만 사용할 수 있으며 공백이나 비ASCII 문자는 사용할 수 없습니다.",
            modelPrefix:
                "모델 접두사는 영문자, 숫자, 기호만 사용할 수 있으며 공백이나 비ASCII 문자는 사용할 수 없습니다.",
            apiKey: "API 키는 영문자, 숫자, 기호만 사용할 수 있으며 공백이나 비ASCII 문자는 사용할 수 없습니다.",
            key: "키는 영문자, 숫자, 기호만 사용할 수 있으며 공백이나 비ASCII 문자는 사용할 수 없습니다.",
        },
        routing: {
            upstreamId:
                '업스트림 ID는 영문자, 숫자, 기호만 사용할 수 있으며 공백, 비ASCII 문자, "/", ":"는 사용할 수 없습니다.',
            publicId:
                '공개 ID는 영문자, 숫자, 기호만 사용할 수 있으며 공백, 비ASCII 문자, "/", ":"는 사용할 수 없습니다.',
            modelPrefix:
                '모델 접두사는 영문자, 숫자, 기호만 사용할 수 있으며 공백, 비ASCII 문자, "/", ":"는 사용할 수 없습니다.',
            apiKey: 'API 키는 영문자, 숫자, 기호만 사용할 수 있으며 공백, 비ASCII 문자, "/", ":"는 사용할 수 없습니다.',
            key: '키는 영문자, 숫자, 기호만 사용할 수 있으며 공백, 비ASCII 문자, "/", ":"는 사용할 수 없습니다.',
        },
        positive: {
            maxInputOverride: "최대 입력 토큰 재정의는 양의 정수여야 합니다.",
            maxOutputOverride: "최대 출력 토큰 재정의는 양의 정수여야 합니다.",
        },
    },
    quotaMessages: {
        day_other: "{{quantity}}일",
        hour_other: "{{quantity}}시간",
        minute_other: "{{quantity}}분",
        second_other: "{{quantity}}초",
        used: "{{percentage}} 사용",
        counter: "[[used]] / [[limit]] [[unit]] 사용",
        remaining: "[[remaining]] [[unit]] 남음",
        dateReset: "[[date]] · 정확한 초기화 시각 알 수 없음",
        atReset: "[[instant]]에 초기화 · [[distance]]",
        future: "{{duration}} 후",
        observed: "[[instant]] 관측 · [[distance]]",
        inventory: {
            pool_disabled: "계정 풀 사용 안 함",
            account_missing: "계정 없음",
            account_disabled: "계정 사용 안 함",
            model_not_selected: "모델 가용성을 확인하려면 업스트림 ID를 선택하세요",
            model_missing: "모델 없음",
            model_disabled: "모델 사용 안 함",
            model_vanished: "모델 사라짐",
            ready: "사용 가능",
        },
        failure: {
            credential_renewal: "자격 증명 갱신 실패",
            timeout: "시간 초과",
            transport: "전송 실패",
            upstream_rejected: "업스트림이 요청을 거부함",
            malformed_observation: "관측 형식 오류",
        },
        quotaObserved: "[[instant]]에 할당량 관측",
        quotaFailed: "할당량 조회 실패 · {{reason}}",
        deletedAccount: "삭제된 계정 #{{id}}",
        priority: "우선순위 {{priority}}",
        models: "모델: [[identifiers]]",
        resets: "[[instant]]에 초기화",
    },
    dynamicMessages: {
        following: "탐색 값 따르기: {{value}}.",
        levelsReplace: "탐색된 집합을 대체합니다 — 클라이언트는 {{value}}를 봅니다.",
        capReplace: "탐색된 값을 대체합니다 — 클라이언트는 {{value}}를 봅니다.",
        mechanismFact: "{{mechanism}}: {{support}}",
        mechanismDiscovered: "탐색 결과: {{support}}.",
        compactionReplace:
            "탐색된 정보를 전부 대체합니다 — 설정하지 않은 메커니즘은 탐색 결과를, 그다음 정의의 선언을 따릅니다.",
        noExpiry: "이 공급자에는 저장된 만료 시각이 없습니다.",
        goneFlow:
            "이 인증 흐름은 더 이상 완료할 수 없습니다 — state가 만료되었거나 이미 사용되었습니다.",
        refusedClipboard:
            "이 브라우저가 클립보드를 거부했습니다 — {{subject}}를 선택해 직접 복사하세요.",
        enabledStored: "저장된 {{total}}개 중 사용 설정 개수",
        readyListed: "나열된 {{total}}개 중 준비 개수",
        copyEndpoint: "{{surface}} 엔드포인트 URL 복사",
        disabledCredential:
            "{{expiry}} 공급자가 사용 안 함 상태입니다 — 다시 사용 설정할 때 다시 연결하세요.",
        noKeysDetail:
            " 서비스 클라이언트는 이 키로 인증합니다 — 키가 없으면 게이트웨이에는 사용 가능한 모델이 없습니다.",
    },
    remainingText: {
        notRefreshedYet: "아직 새로 고치지 않음",
        overrideMaxInputTokens: "최대 입력 토큰 재정의",
        maxInputTokensOverride: "최대 입력 토큰 재정의 값",
        overrideMaxOutputTokens: "최대 출력 토큰 재정의",
        maxOutputTokensOverride: "최대 출력 토큰 재정의 값",
        chooseAProvider: "공급자 선택",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "업스트림이 더 이상 보고하지 않는 공급자와 업스트림 ID를 지정하면 아래 설정에 따라 그 모델을 되찾아옵니다. 모델은 독립 공급자 접두사 또는 소유한 계정 풀 접두사를 통해 자동으로 노출됩니다.",
        allProviders: "모든 공급자",
        replaceTheUpstreamAPIKey: "업스트림 API 키 교체",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "쓰기 전용입니다. 게이트웨이가 값을 반환하지 않으므로 비워 두면 저장된 값이 유지됩니다.",
        backToPools: "계정 풀 목록으로",
        publicRoute: "공개 라우트",
        publicRoutes: "공개 라우트",
        backToPublicRoutes: "공개 라우트 목록으로",
        backToAPIKeys: "API 키 목록으로",
        backToModels: "모델 목록으로",
        backToProviders: "공급자 목록으로",
        deletedRoute: "삭제된 라우트",
        deletedModel: "삭제된 모델",
        deletedProvider: "삭제된 공급자",
        deletedKey: "삭제된 키",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "아래 답변은 아직 이전 질의의 결과입니다 — 인라인 오류를 표시하는 필드가 문제를 알려줍니다.",
        fromAndToDoNotApplyThere: " — 시작과 끝은 여기에 적용되지 않습니다.",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "발송된 서비스 요청마다 행 하나가 나타납니다 — 게이트웨이가 업스트림으로 보내는 순간입니다.",
        showTheKeyWhileTyping: "입력하는 동안 키 표시",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "저장은 게이트웨이 버전 위에 편집 내용을 씁니다. 되돌리기는 게이트웨이 버전을 가져옵니다.",
        theOAuthCredentialsWereNeverFresh: "OAuth 자격 증명이 유효했던 적이 없습니다.",
        allPublicRoutes: "모든 공개 라우트",
        allPhysicalModels: "모든 물리 모델",
        allAPIKeys: "모든 API 키",
    },
    entityMessages: {
        editName: "[[name]] 편집",
        editIdentifier: "[[identifier]] 편집",
        deleteKey: "[[name]] 삭제할까요? 이 시크릿을 보유한 클라이언트는 접근 권한을 잃습니다.",
        deleteLastKey:
            "[[name]] 삭제할까요? 마지막으로 사용 설정된 키입니다 — 다른 키를 사용 설정할 때까지 게이트웨이는 아무것도 서비스하지 않습니다.",
        deleteModel:
            "[[identifier]] 삭제할까요? 직접 라우트도 함께 삭제됩니다. 계정 풀 라우트는 저장된 상태로 남습니다. 다시 탐색하거나 직접 만들어도 새로운 물리 행이 생길 뿐, 삭제된 라우트는 복원되지 않습니다.",
        deleteProvider: "[[name]] 삭제할까요?",
        switchPreset: "[[name]] 프리셋으로 전환할까요? 편집한 연결 레시피가 교체됩니다.",
        noKeysMatch: '"[[query]]"에 해당하는 API 키가 없습니다.',
        noProvidersMatch: '"[[query]]"에 해당하는 공급자가 없습니다.',
        noModelsMatch: '"[[query]]"에 해당하는 모델이 없습니다.',
        noProviderModelsMatch: '이 공급자에는 "[[query]]"에 해당하는 모델이 없습니다.',
        missingPath:
            "[[path]]에는 아무것도 없습니다. 삭제되었거나 주소를 잘못 입력했을 수 있습니다.",
        aliasCreated: "별칭 생성됨: [[identifier]]",
        addAlias: "[[identifier]]의 별칭 추가",
    },
    modelCard: {
        heading: "모델 · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "저장하지 않은 편집이 있는 동안 이 키가 게이트웨이에서 변경되었습니다.",
            provider: "저장하지 않은 편집이 있는 동안 이 공급자가 게이트웨이에서 변경되었습니다.",
            model: "저장하지 않은 편집이 있는 동안 이 모델이 게이트웨이에서 변경되었습니다.",
            pool: "저장하지 않은 편집이 있는 동안 이 계정 풀이 게이트웨이에서 변경되었습니다.",
            "public route":
                "저장하지 않은 편집이 있는 동안 이 공개 라우트가 게이트웨이에서 변경되었습니다.",
            "api key": "저장하지 않은 편집이 있는 동안 이 API 키가 게이트웨이에서 변경되었습니다.",
        },
        noRequests: {
            key: "이 키에는 아직 기록된 요청이 없습니다.",
            provider: "이 공급자에는 아직 기록된 요청이 없습니다.",
            model: "이 모델에는 아직 기록된 요청이 없습니다.",
            pool: "이 계정 풀에는 아직 기록된 요청이 없습니다.",
            "public route": "이 공개 라우트에는 아직 기록된 요청이 없습니다.",
            "api key": "이 API 키에는 아직 기록된 요청이 없습니다.",
        },
        notFound: {
            api_key: "API 키를 찾을 수 없음",
            provider: "공급자를 찾을 수 없음",
            model: "모델을 찾을 수 없음",
            pool: "계정 풀을 찾을 수 없음",
            model_route: "공개 라우트를 찾을 수 없음",
        },
        replaceSecret: {
            typed: "입력한 시크릿을 교체할까요?",
            current: "현재 시크릿을 교체할까요?",
        },
        bound: {
            From: "시작은 2026-09-23T10:00 같은 완전한 날짜와 시각이어야 하며, 비워 두면 범위를 지정하지 않습니다.",
            To: "끝은 2026-09-23T10:00 같은 완전한 날짜와 시각이어야 하며, 비워 두면 범위를 지정하지 않습니다.",
        },
        updated: "{{instant}}에 업데이트됨",
        created: "{{instant}}에 생성됨",
        version: "버전 {{version}}으로 저장되었습니다.",
        unlisted: "{{value}}(더 이상 목록에 없음)",
        accountUnlisted: "계정 #{{id}}(더 이상 목록에 없음)",
        moveUp: "{{name}} 위로 이동",
        moveDown: "{{name}} 아래로 이동",
        remove: "{{name}} 제거",
        publicIdFor: "{{subject}}의 공개 ID",
        source: {
            key: "키에서",
            provider: "공급자에서",
            pool: "계정 풀에서",
            route: "모델 재정의",
        },
        deleted: {
            provider: "삭제된 공급자 #{{id}}",
            pool: "삭제된 계정 풀 #{{id}}",
            route: "삭제된 라우트 #{{id}}",
        },
        status: "클라이언트 {{client}} · 업스트림 {{upstream}}",
        metrics: " · {{duration}} · 입력 {{input}} · 출력 {{output}}{{tail}}",
        optionalTokens: " · 캐시 {{cached}} · 캐시 쓰기 {{write}} · 추론 {{reasoning}}",
    },
    routingMessages: {
        directRoutes_other: "직접 라우트 {{quantity}}개",
        poolMembers_other: "계정 구성원 [[quantity]]개 · 모델 접두사: [[prefix]]",
        directMissing: "직접 · 물리 모델 #[[id]]",
        direct: "직접 · [[provider]] · [[model]]",
        pool: "계정 풀 · [[pool]] · [[model]]",
        publicIds: "공개 ID [[identifiers]]",
        modelAccess: "[[identifier]] 모델 접근",
        ownerDefault: {
            provider: "[[name]] 공급자 기본값",
            pool: "[[name]] 계정 풀 기본값",
        },
        directTarget: "직접 · 모델 #[[id]]",
        poolTarget: "계정 풀 · [[identifier]]",
        reconnect: "[[editLink]]에서 다시 연결하세요 — 자격 증명 카드가 교체합니다.",
        reconnectDisabled:
            "공급자가 사용 안 함 상태입니다 — 다시 사용 설정할 때 다시 연결하세요. [[editLink]]의 자격 증명 카드가 교체합니다.",
    },
    linkedMessages: {
        keyEndpoint:
            "클라이언트를 엔드포인트로 지정하세요 — 모든 인터페이스가 허용된 공개 항목을 서비스하며, URL은 [[overviewLink]]에 있습니다.",
        servingKey:
            "서비스 클라이언트는 서비스 API 키로 인증합니다 — 여기로 무엇이든 지정하기 전에 [[keysLink]]에서 키를 만드세요.",
        vanished:
            "그동안에는 아무것도 서비스하지 않습니다. 업스트림이 이 모델을 다시 나열하면 [[discoverLink]]하거나 [[claimLink]]하세요.",
        discoverLabel: "공급자 페이지에서 다시 탐색",
        claimLabel: "모델 만들기 페이지에서 되찾기",
        selected: "{{quantity}}개를 선택했습니다. 성공한 행은 다시 제출되지 않습니다.",
        kimiDescription:
            "Coding 엔드포인트를 사용하는 Kimi 계정입니다. 디바이스 코드 흐름이며, 디바이스 ID 슬롯이 인증과 폴링, 갱신 모두에서 사용자 지정 `X-Msh-*` 헤더로 전달됩니다.",
        githubDescription:
            "GitHub 계정으로, 다시 프로비저닝할 때 수명이 긴 토큰을 단기 Copilot 세션 토큰과 계정의 API 엔드포인트로 교환합니다.",
        chartPoint_other: "[[day]]: 요청 [[quantity]]건",
        inventoryEmpty: "[[name]]에는 아직 모델이 없습니다.",
        inventoryUnready:
            "[[name]]에는 사용 가능한 모델이 없습니다 — 모든 모델이 사용 안 함이거나 사라졌습니다.",
    },
    vocabulary: {
        credentialKind: {
            api_key: "정적 API 키",
            oauth: "OAuth 계정",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "독립형 압축",
            responses_explicit_trigger: "명시적 트리거",
            responses_automatic_compaction: "자동 압축",
            messages_on_demand_compaction: "요청 시 압축",
            messages_threshold_compaction: "임계값 압축",
        },
        mechanismSupport: {
            supported: "지원",
            unsupported: "지원 안 함",
            unknown: "알 수 없음",
        },
        discoveryState: {
            discovered: "탐색됨",
            vanished: "더 이상 보고되지 않음",
            manual: "직접 추가",
        },
        reasoningLevel: {
            none: "없음",
            minimal: "최소",
            low: "낮음",
            medium: "중간",
            high: "높음",
            xhigh: "매우 높음",
            max: "최대",
        },
        apiLogOutcome: {
            completed: "완료됨",
            client_aborted: "클라이언트 중단",
            upstream_error: "업스트림 오류",
            internal: "내부 오류",
        },
        apiLogEndpointType: {
            openai_chat_completions: "Chat Completions",
            openai_responses: "Responses",
            openai_responses_compact: "Responses 압축",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interactions: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        usageGroupBy: {
            day: "일",
            model: "물리 모델",
            model_route: "공개 라우트",
            provider: "공급자",
            api_key: "API 키",
            endpoint_type: "엔드포인트",
            outcome: "결과",
            is_stream: "스트림 모드",
            http_status: "HTTP 상태",
        },
    },
    common: {
        save: "변경 사항 저장",
        cancel: "취소",
        retry: "다시 시도",
        language: "언어",
    },
    language: {
        system: "브라우저 설정 따르기",
        loading: "{{languageName}} 불러오는 중…",
        failed: "{{languageName}} 불러오기에 실패했습니다. 현재 언어가 계속 표시됩니다.",
        startupFailed: "{{languageName}} 불러오기에 실패했습니다. 현재 영어가 표시됩니다.",
    },
    models: {
        discoveredCount_other: "모델 {{count}}개를 탐색했습니다.",
    },
} as const;
export default korean;
