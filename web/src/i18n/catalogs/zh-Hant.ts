/** Traditional Chinese administration messages. */
const traditionalChinese = {
    shell: {
        skipToContent: "跳至主要內容",
        primary: "主要導覽",
        refresh: "重新整理",
        disconnect: "中斷連線",
        keep: "保留",
    },
    connection: {
        connectToJanusGate: "連線至 JanusGate",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "此應用程式用於管理 JanusGate 閘道的上游帳戶、其模型與認證資訊。",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "一個正在執行且可從此瀏覽器連線的 JanusGate 執行個體。",
        theOriginBesideIt: "旁邊的來源位址",
        copyThisAppsOrigin: "複製此應用程式的來源位址",
        gatewayAddress: "閘道位址",
        schemeHostAndPortOnly: "僅填寫通訊協定、主機與連接埠。",
        managementToken: "管理權杖",
        sentOnlyToThatInstanceAndKeptOnly: "僅傳送給該執行個體，並只保留在此瀏覽器中。",
        cancel: "取消",
    },
    controls: {
        modelAccess: "模型存取權限",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "預設值會影響現有繼承的項目；帳戶池存取權限也涵蓋日後符合資格的成員。全域可用性與權限分開管理。",
        defaultModelAccessLabel: "預設模型存取權限：",
        defaultModelAccess: "預設模型存取權限",
        appliesToEntriesWithoutAProviderPoolOr:
            "適用於未設定上游帳戶、帳戶池或模型覆寫值的項目，包括日後新增的項目。",
        searchModelAccess: "搜尋模型存取權限",
        publicIDOrOwnerName: "公開 ID 或擁有者名稱",
        overridesOnly: "僅顯示覆寫值",
        removeOverride: "移除覆寫值",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "儲存前請先移除已刪除項目的參照，或重新整理以重新核對模型清單。",
        thisDefaultAppliesToFutureEntries: "此預設值也適用於日後新增的項目。",
        noEntriesMatchThisViewHiddenOverridesAre: "沒有項目符合此檢視。隱藏的覆寫值仍會保留。",
        name: "名稱",
        key: "金鑰",
        skOrGenerateOne: "sk-… 或產生一組",
        generate: "產生",
        replaceIt: "取代",
        keepIt: "保留",
        theSecretHere: "此處的密鑰",
        delete: "刪除…",
        revert: "還原",
        unsavedChanges: "尚未儲存的變更",
        overrideReasoningLevels: "覆寫推理等級",
        reasoningLevels: "推理等級",
        pickEveryLevelThisModelDeclaresUntickAll:
            "選取此模型宣告的每一個等級。全部取消選取即表示不宣告任何等級。",
        aPositiveIntegerClearTheSwitchToFollow: "請輸入正整數。清除開關可再次依循探索結果。",
        overrideCompactionFacts: "覆寫壓縮資訊",
        compactionMechanisms: "壓縮機制",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            "設定此模型要取代其資訊的每一個機制。「未知」會明確表示沒有資訊，並抑制該機制已探索到的資訊。",
        addAlias: "新增別名",
        modelPrefix: "模型前綴",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "在帳戶池之外，模型會以此前綴加上上游 ID 呼叫。帳戶池成員則使用所屬帳戶池的前綴。留空則直接使用上游 ID。變更前綴會改變現有的自動模型名稱。",
        upstreamBaseURL: "上游基礎 URL",
        enabled: "已啟用",
        publicRoute: "公開模型路由",
        provider: "上游帳戶",
        physicalModel: "實體模型",
        apiKey: "API 金鑰",
        endpoint: "端點",
        outcome: "結果",
        streamMode: "串流模式",
        httpStatuses: "HTTP 狀態",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "以逗號分隔，例如 200, 429。未收到回應的請求不會符合任何狀態。",
    },
    navigation: {
        overview: "總覽",
        providers: "上游帳戶",
        inventory: "模型清單",
        pools: "帳戶池",
        routes: "公開模型路由",
        apiKeys: "API 金鑰",
        usage: "用量",
        lightTheme: "淺色主題 — 切換為深色",
        darkTheme: "深色主題 — 跟隨系統",
        systemTheme: "系統主題 — 切換為淺色",
    },
    rich: {
        connected: "已連線至 [[baseUrl]] — 權杖只保留在此瀏覽器中。",
        setupToken: "其管理權杖：該執行個體設定檔中的 [[setting]]。",
        setupCors:
            "在該執行個體的 [[setting]] 中允許此應用程式的來源 — 除非此應用程式由閘道本身提供。",
    },
    titles: {
        bulkPublication: "發佈模型清單",
        providerPools: "帳戶池",
        providerPoolNew: "建立帳戶池",
        providerPoolEdit: "編輯帳戶池 #{{id}}",
        modelRoutes: "公開模型路由",
        modelRouteNew: "建立公開模型路由",
        modelRouteEdit: "編輯公開模型路由 #{{id}}",
        home: "總覽",
        providers: "上游帳戶",
        providerNew: "建立上游帳戶",
        providerDetail: "上游帳戶 #{{id}}",
        providerEdit: "編輯上游帳戶 #{{id}}",
        models: "模型",
        modelNew: "建立模型",
        modelDetail: "模型 #{{id}}",
        modelEdit: "編輯模型 #{{id}}",
        apiKeys: "API 金鑰",
        apiKeyNew: "建立 API 金鑰",
        apiKeyDetail: "API 金鑰 #{{id}}",
        apiKeyEdit: "編輯 API 金鑰 #{{id}}",
        usage: "用量",
        usageQuery: "用量彙總",
        notFound: "找不到頁面",
    },
    format: {
        lessThanMinute: "不到一分鐘",
        minutes: "{{minutes}} 分",
        hours: "{{hours}} 小時",
        hoursMinutes: "{{hours}} 小時 {{minutes}} 分",
        days: "{{days}} 天",
        daysHours: "{{days}} 天 {{hours}} 小時",
        expired: "已於 {{distance}}前過期 — 過期時間為 {{instant}}。",
        valid: "有效期限還剩 {{distance}} — 到 {{instant}} 為止。",
        durationSeconds: "{{seconds}} 秒",
        durationMinutesSeconds: "{{minutes}} 分 {{seconds}} 秒",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "API 金鑰",
            editAPIKey: "編輯 API 金鑰",
            secret: "密鑰",
            overview: "總覽",
            details: "詳細資料",
            name: "名稱",
            created: "建立時間",
            status: "狀態",
            apiKeys: "API 金鑰",
            createAPIKey: "建立 API 金鑰",
            enabled: "已啟用",
            disabled: "已停用",
        },
        apiKeyEditScreen: {
            editAPIKey: "編輯 API 金鑰",
            settings: "設定",
            rotatingMeansGenerateThenSaveMessage: "輪換是指先產生，再儲存。",
            enabled: "已啟用",
            replacingTheSecretRevokesTheOldValueOnMessage: "取代密鑰後，儲存時舊值即會失效。",
            aKeyNeedsASecret: "金鑰必須有密鑰。",
            deleteThisKey: "刪除此金鑰",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "刪除後金鑰立即失效，持有此金鑰的用戶端會開始收到 401。此金鑰的模型存取權限規則會一併移除；歷史用量記錄仍會保留。",
            apiKey: "API 金鑰",
            showTheCurrentKey: "顯示目前的密鑰",
        },
        apiKeyNewScreen: {
            createAPIKey: "建立 API 金鑰",
            oneSecretYourServingClientsShareMessage: "供取用服務的用戶端共用的一組密鑰。",
            apiKeysBackLabel: "API 金鑰",
            key: "金鑰",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "密鑰只能包含英文字母、數字與符號，不能有空格，且不得與其他金鑰重複。",
            enabled: "已啟用",
            loadModelAccessInventoryBeforeCreatingAKeyMessage:
                "建立金鑰前，請先載入模型存取權限清單。",
            creating: "正在建立…",
        },
        apiKeysScreen: {
            apiKeys: "API 金鑰",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "服務金鑰。OpenAI 用戶端會以 Authorization: Bearer <key> 傳送，Gemini 使用 x-goog-api-key，Anthropic 使用 x-api-key；開啟任一金鑰即可查看其密鑰與用量。",
            createAPIKey: "建立 API 金鑰",
            noAPIKeysYet: "尚無 API 金鑰",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "全新的閘道在第一個金鑰建立前不會提供任何服務。",
            searchAPIKeys: "搜尋 API 金鑰",
            searchByNameOrKey: "依名稱或金鑰搜尋",
            theSecretHere: "此處的密鑰",
            on: "開啟",
            off: "關閉",
        },
        bulkPublicationScreen: {
            createAliases: "建立別名",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "選擇實體模型並填寫其公開 ID。每個選取項目會建立一條已啟用的直接路由；名稱衝突會個別回報，不會自動重新命名或覆寫。",
            inventoryBackLabel: "模型清單",
            noPhysicalModelsToAlias: "沒有可建立別名的實體模型",
            discoverOrAddInventoryFirstMessage: "請先探索或新增模型清單。",
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "另一個選取的資料列已使用此公開 ID。請選擇互不相同的明確名稱。",
            openRoute: "開啟路由",
            createSelectedAliases: "建立選取的別名",
            publicIDIsRequiredMessage: "必須填寫公開 ID。",
            noDirectRoutes: "沒有直接路由",
            modelVanishedItsNewRouteWillBeSuspended: "模型已消失；其新路由將暫停服務",
            modelEnabled: "模型已啟用",
            modelDisabled: "模型已停用",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "輸入時顯示權杖",
            probingTheGateway: "正在探測閘道…",
            connect: "連線",
        },
        homeScreen: {
            overview: "總覽",
            theHealthOfThisGatewayAtAGlanceMessage: "一眼掌握此閘道的運作狀況。",
            createProvider: "建立上游帳戶",
            providers: "上游帳戶",
            publicRoutes: "公開模型路由",
            inventoryReady: "就緒模型",
            apiKeys: "API 金鑰",
            recentRequests24h: "近期請求（24 小時）",
            recentErrors24h: "近期錯誤（24 小時）",
            requestsPerDay: "每日請求數",
            theLast14DaysInUTCTheGatewaysMessage: "最近 14 天，以 UTC 計算 — 這是閘道自身的日界。",
            openUsage: "開啟用量",
            storedSettingsReviewedMessage: "已檢閱儲存的設定。",
            needsAttention: "需要處理",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "沒有可用的模型。請在已啟用的上游帳戶上探索模型。",
            noAPIKeysYetMessage: "尚無 API 金鑰。",
            endpoints: "端點",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "每個介面都會提供所有模型。將任何 OpenAI、Gemini 或 Anthropic 格式的用戶端指向此執行個體即可。",
            theURLBesideIt: "旁邊的 URL",
            noProvidersYet: "尚無上游帳戶",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "一個上游帳戶對應一個上游服務帳戶。其模型會在你探索後出現。",
            createYourFirstProvider: "建立你的第一個上游帳戶",
            connectedToTheGateway: "已連線至閘道",
            discoverModels: "探索模型",
            createAPIKey: "建立 API 金鑰",
            openAndDiscover: "開啟並探索",
            reviewModels: "檢閱模型",
            credentialNotFresh: "認證資訊從未有效",
            credentialExpired: "認證資訊已過期",
            reconnect: "重新連線",
            openProvider: "開啟上游帳戶",
            createAProvider: "建立上游帳戶",
            discoverItsModelsFromUpstream: "從上游探索其模型",
            issueAnAPIKeyForServingClients: "為取用服務的用戶端簽發 API 金鑰",
            pointAClientAtAnEndpointBelow: "將用戶端指向下方的端點",
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "請在公開模型路由中檢查配額可用性。這些數字描述的是已儲存的開關與模型清單。",
            noRequestsRecordedMessage: "沒有請求記錄。",
        },
        modelDetailScreen: {
            model: "模型",
            upstreamOnProvider: "{{provider}} 上的 {{upstreamId}}。",
            modelsBackLabel: "模型",
            capabilities: "能力",
            whatTheModelListingsDeclareForThisModelMessage:
                "模型清單對此模型宣告的內容。這裡的任何項目都不會限制請求 — 上游回應什麼就是什麼。",
            reasoningLevels: "推理等級",
            maxInputTokens: "輸入權杖上限",
            maxOutputTokens: "輸出權杖上限",
            compaction: "壓縮",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "此資料列本身的壓縮資訊 — 有覆寫值就用覆寫值，否則採用最近一次探索的結果。定義中的宣告則決定兩者都未提及的部分。",
            noLevels: "沒有等級",
            addAlias: "新增別名",
            editModel: "編輯模型",
            details: "詳細資料",
            publicModelNames: "公開模型名稱",
            directRoutes: "直接路由",
            upstreamID: "上游 ID",
            provider: "上游帳戶",
            discovery: "探索",
            status: "狀態",
            models: "模型",
            overriddenByAdministratorMessage: "已由管理員覆寫。",
            fromTheLatestDiscoveryMessage: "來自最近一次的探索。",
            unknownOmittedFromModelListingsMessage: "未知 — 模型清單會略過。",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "此資料列沒有相關資訊 — 由定義中的宣告決定。",
            unknown: "未知",
            noAutomaticEntry: "沒有自動項目",
            enabled: "已啟用",
            disabled: "已停用",
        },
        modelEditScreen: {
            editModel: "編輯模型",
            modelSettings: "模型設定",
            enabled: "已啟用",
            deleteThisModel: "刪除此模型",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "刪除會移除此實體模型及其直接路由。帳戶池路由仍會保留。",
            model: "模型",
        },
        modelNewScreen: {
            createModel: "建立模型",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "探索不會產生的項目 — 或上游已不再回報、想重新取回的模型。",
            noProvidersYet: "尚無上游帳戶",
            aModelBelongsToAProviderAndThereMessage:
                "模型必須屬於某個上游帳戶，但目前沒有可承載的帳戶。請先新增上游帳戶 — 探索可能就能產生此模型。",
            createProvider: "建立上游帳戶",
            aModelAddedByHandIsNeverRemovedMessage:
                "手動新增的模型永遠不會自動移除 — 也能取回探索遺漏的模型。",
            models: "模型",
            identity: "身分",
            provider: "上游帳戶",
            upstreamID: "上游 ID",
            modelSettings: "模型設定",
            enabled: "已啟用",
            aModelBelongsToOneProviderMessage: "必填。模型只屬於一個上游帳戶。",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "必填。會原樣送往上游 — 只能包含英文字母、數字與符號。",
            carriedHereFromAModelTheUpstreamNoMessage:
                "必填。此處帶入的是上游已不再回報的模型 — 提交後會依下方設定將其取回。",
            creating: "正在建立…",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "此模型名稱由系統自動管理。請編輯擁有它的上游帳戶或帳戶池前綴來變更。",
            deleteThisPublicRoute: "刪除此公開模型路由",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "要求此公開 ID 的用戶端會收到找不到模型的錯誤。實體模型清單與帳戶池仍會保留。",
            publicRouteSettings: "公開模型路由設定",
            enabled: "已啟用",
            publicID: "公開 ID",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "用戶端要求的確切模型名稱。重新命名後，用戶端也必須跟著更新。",
            targetType: "目標類型",
            pool: "帳戶池",
            upstreamID: "上游 ID",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "向每個成員要求的確切識別碼。成員缺少模型並不會妨礙發佈。",
            physicalModel: "實體模型",
            createPublicRoute: "建立公開模型路由",
            editPublicRoute: "編輯公開模型路由",
            publicRoutes: "公開模型路由",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "由其上游帳戶或帳戶池自動維護的模型名稱。",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "用戶端會要求此公開 ID。變更其目標不會影響該名稱。",
            openPool: "開啟帳戶池",
            openPhysicalModel: "開啟實體模型",
            deleteThisPublicRouteQuestion: "要刪除此公開模型路由嗎？",
            publicIDIsRequiredMessage: "必須填寫公開 ID。",
            directPhysicalModel: "直接實體模型",
            chooseAPool: "選擇帳戶池",
            upstreamIDIsRequiredMessage: "必須填寫上游 ID。",
            chooseAPhysicalModel: "選擇實體模型",
        },
        modelsScreen: {
            models: "模型",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "模型會透過獨立上游帳戶前綴或其擁有的帳戶池前綴自動公開。",
            createAliases: "建立別名",
            createModel: "建立模型",
            noModelsYet: "尚無模型",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "請先新增上游帳戶，再探索其模型 — 或手動新增一個。",
            thisProviderHoldsNoModels: "此上游帳戶沒有模型。",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "此上游帳戶已不在清單中 — 請在上游帳戶篩選中選擇其他帳戶。",
            createProvider: "建立上游帳戶",
            searchModels: "搜尋模型",
            searchByUpstreamIDOrProviderName: "依上游 ID 或帳戶名稱搜尋",
            provider: "上游帳戶",
            noModelsOnAnyProviderYetDiscoverOrMessage:
                "任何上游帳戶都還沒有模型 — 請探索或新增一個。",
        },
        notFoundScreen: {
            pageNotFound: "找不到頁面",
            goToOverview: "前往總覽",
            providers: "上游帳戶",
            models: "模型",
        },
        oauthFlowView: {
            flowExpires: "流程到期時間",
            singleuseStateLabel: "一次性 state：",
            authorizationURL: "授權 URL",
            openTheAuthorizationPage: "開啟授權頁面",
            theURLAbove: "上方的 URL",
            copyTheURL: "複製 URL",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "state 僅能使用一次，且流程很快就會到期 — 核可存取後請儘快完成。",
            mostLoginsEndOnAPageThatShowsMessage:
                "多數登入會停在一個沒有顯示任何內容的頁面 — 這是正常的。請貼上瀏覽器顯示的完整位址；若頁面顯示驗證碼，則改為貼上該驗證碼。",
            startOver: "重新開始",
            callbackURLOrAuthorizationCode: "回呼 URL 或授權碼",
            pasteTheWholeCallbackURL: "貼上完整的回呼 URL",
            verificationCode: "驗證碼",
            theVerificationCode: "此驗證碼",
            copyTheCode: "複製驗證碼",
            openTheVerificationPage: "開啟驗證頁面",
            theVerificationURL: "此驗證 URL",
            approveTheCodeAtTheVerificationPageThisMessage:
                "在驗證頁面核可驗證碼；此頁面會持續向閘道查詢結果，並在流程完成後立即繼續。",
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "請留在此頁面直到流程完成 — 在主控台內切換頁面會捨棄此流程。",
        },
        poolAvailabilityCard: {
            refreshQuota: "重新整理配額",
            resetRestrictions: "重設限制",
            quotaExhausted: "配額已用盡",
            poolAvailability: "帳戶池可用性",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "已儲存的觀測結果。重新整理會實際查詢上游帳戶。重設會清除限制並設定重新整理寬限期；不會重設上游配額。",
            thisPoolHasNoMembersMessage: "此帳戶池沒有成員。",
            readingStoredAvailability: "正在讀取已儲存的可用性…",
            quotaNotObserved: "尚未觀測到配額",
            quotaReadsUnsupported: "不支援讀取配額",
            administratorResetRefreshGraceInstalled: "管理員已重設；已設定重新整理寬限期",
            accountEnabled: "帳戶已啟用",
            updatingAvailability: "正在更新可用性…",
            allModels: "所有模型",
            resetUnknown: "重設時間未知",
            resetTimePassedRefreshRequired: "重設時間已過；需要重新整理",
        },
        providerDetailScreen: {
            provider: "上游帳戶",
            editProvider: "編輯上游帳戶",
            editPage: "編輯頁面",
            storedAvailability: "已儲存的可用性",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "此帳戶上所有已觀測到的限制。重設會清除限制並設定重新整理寬限期；不會重設上游配額。",
            details: "詳細資料",
            modelPrefix: "模型前綴",
            type: "類型",
            definition: "定義",
            upstreamBaseURL: "上游基礎 URL",
            credentials: "認證資訊",
            status: "狀態",
            discoveredFromUpstreamOrAddedByHandMessage: "從上游探索而來，或手動新增。",
            createModel: "建立模型",
            allModels: "所有模型",
            providers: "上游帳戶",
            createProvider: "建立上游帳戶",
            enabled: "已啟用",
            disabled: "已停用",
            theOAuthCredentialsHaveExpiredMessage: "OAuth 認證資訊已過期。",
            noneExactUpstreamIDs: "無 — 使用確切的上游 ID",
            discovering: "正在探索…",
            discoverModels: "探索模型",
            askingTheUpstreamWhatItReports: "正在詢問上游回報的內容…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "此上游帳戶尚無模型 — 請探索上游所回報的模型，或手動新增一個。",
        },
        providerEditScreen: {
            editProvider: "編輯上游帳戶",
            settings: "設定",
            credentials: "認證資訊",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "OAuth 認證資訊會過期 — 重新連線即可取代。",
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "此上游帳戶的定義需要以基礎 URL 評估請求 — 請還原一個，或選擇「還原」。",
            deleteThisProvider: "刪除此上游帳戶",
            provider: "上游帳戶",
            modelPrefix: "模型前綴",
            aProviderNeedsANameMessage: "上游帳戶必須有名稱。",
            starting: "正在開始…",
            reconnectTheAccount: "重新連線帳戶",
            replacing: "正在取代…",
            replaceTheCredentials: "取代認證資訊",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "刪除會移除此帳戶及其帳戶池成員資格。此帳戶沒有實體模型。",
        },
        providerNewScreen: {
            createProvider: "建立上游帳戶",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "一個上游帳戶。選擇預設後，認證資訊的取得方式便隨之決定。",
            providersBackLabel: "上游帳戶",
            readAgain: "重新讀取",
            preset: "預設",
            switchPreset: "切換預設",
            keepMyEdit: "保留我的編輯",
            identity: "身分",
            connection: "連線",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage: "金鑰僅供寫入 — 閘道永遠不會回傳。",
            connectTheAccount: "連線帳戶",
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "授權進行期間會鎖定 — 上游帳戶會依流程啟動時已送出的內容建立。",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "在流程指示的位置核可存取，然後讓流程完成。此應用程式不會儲存認證資訊。",
            upstreamBaseURL: "上游基礎 URL",
            definition: "定義",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "閘道儲存的連線設定。編輯已儲存定義的 JSON 會註冊下一個版本 — 已儲存的版本永遠不會被改寫。",
            notStoredYetSubmittingCreatesVersion1Message: "尚未儲存 — 提交後會建立版本 1。",
            advancedTheConnectionRecipePrefilledByThePreset: "進階：連線設定 — 由預設預先填入",
            definitionJSON: "定義 JSON",
            resetToPreset: "重設為預設",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "連線設定無效 — 請展開上方的「進階」區段，查看有問題的路徑。",
            modelPrefix: "模型前綴",
            creating: "正在建立…",
            connecting: "正在連線…",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "必填。此定義的請求所要評估的上游位址 — 缺少時閘道會拒絕啟動流程。",
            starting: "正在開始…",
            startTheAuthorization: "開始授權",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "同一上游服務的可互換帳戶，會依優先順序嘗試。",
            inspectUpstreamID: "檢查上游 ID",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "檢查已儲存的成員資格。留空時只會顯示帳戶層級的限制。",
            deleteThisPool: "刪除此帳戶池",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "刪除會移除自動產生的帳戶池項目，並還原獨立的成員前綴。手動建立的路由必須先移除或重新指定目標。",
            poolSettings: "帳戶池設定",
            enabled: "已啟用",
            name: "名稱",
            modelPrefix: "模型前綴",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "帳戶池會自動以此前綴加上上游 ID 公開成員模型。留空則使用確切的上游 ID。變更前綴會重新命名現有的自動項目。",
            accountsInPriorityOrder: "依優先順序排列的帳戶",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "成員必須提供相同的上游服務。加入帳戶池後，其本身的前綴項目會隱藏。新模型會自動出現在這裡，並且只會路由至擁有該模型的成員。允許空白的帳戶池。",
            moveUp: "上移",
            moveDown: "下移",
            remove: "移除",
            addAccount: "新增帳戶",
            addMember: "新增成員",
            createPool: "建立帳戶池",
            editPool: "編輯帳戶池",
            pools: "帳戶池",
            deleteThisPoolItsMembershipIsRemovedMessage: "要刪除此帳戶池嗎？其成員資格會一併移除。",
            nameIsRequiredMessage: "必須填寫名稱。",
            chooseAnAccount: "選擇帳戶",
        },
        providerQuotaCard: {
            upstreamQuota: "上游配額",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "額度屬於上游帳戶，可能包含其他用戶端產生的用量。",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage:
                "此上游帳戶的定義未宣告可檢視配額。",
            checkingWhetherThisDefinitionSupportsQuotaReads: "正在檢查此定義是否支援讀取配額…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "尚未觀測到配額。重新整理會實際查詢上游帳戶。",
            readingUpstreamQuota: "正在讀取上游配額…",
            showingThePreviousReadingMessage: "目前顯示的是上一次的讀取結果。",
            observed: "觀測時間",
            resetTimeReachedRefreshToUpdate: "已到重設時間；請重新整理以更新",
            theUpstreamReturnedNoQuotaEntriesMessage: "上游未回傳任何配額項目。",
            periodUnknown: "週期未知",
            monthly: "每月",
            unknown: "未知",
            unlimited: "無限制",
            resetUnknown: "重設時間未知",
            readingQuota: "正在讀取配額…",
            refreshingQuota: "正在重新整理配額…",
            refreshQuota: "重新整理配額",
            retryQuota: "重試配額",
        },
        providersScreen: {
            providers: "上游帳戶",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "上游帳戶。開啟任一帳戶即可查看其模型與用量 — 編輯則在更進一層的頁面。",
            createProvider: "建立上游帳戶",
            noProvidersYet: "尚無上游帳戶",
            aProviderIsOneUpstreamAccountMessage: "一個上游帳戶對應一個上游服務帳戶。",
            searchProviders: "搜尋上游帳戶",
            searchByNameTypeOrFormat: "依名稱、類型或格式搜尋",
        },
        readNotice: {
            readAgain: "重新讀取",
        },
        routingListsScreen: {
            pools: "帳戶池",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "由同一上游服務的可互換帳戶組成的具名模型目錄。",
            createPool: "建立帳戶池",
            noPoolsYet: "尚無帳戶池",
            createANamedPoolWithAModelPrefixMessage:
                "建立具名的帳戶池並設定模型前綴，成員模型就會自動公開。",
            noModelPrefix: "無",
            publicRoutes: "公開模型路由",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "用戶端要求的模型名稱。獨立上游帳戶與帳戶池會自動公開模型名稱。其他別名則為選用。",
            createPublicRoute: "建立公開模型路由",
            noPublicRoutesYet: "尚無公開模型路由",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "直接發佈實體模型，或指定帳戶池與確切的上游 ID。",
            automaticPrefix: "自動 · ",
            on: "開啟",
            off: "關閉",
        },
        screenFrame: {
            readAgain: "重新讀取",
        },
        usageQueryScreen: {
            usageAggregates: "用量彙總",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "在篩選條件內，每個維度組合各一個分組 — 只含次數與權杖加總，絕不含內容。",
            viewRequests: "檢視請求",
            question: "查詢條件",
            groupBy: "分組依據",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "任何維度組合皆可 — 未選取任何維度時，會加總所有符合的資料列。",
            window: "時間範圍",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "填入下方的界線 — 起點回溯一個時間範圍，終點留空 — 以現在為準；這些欄位仍可手動編輯。",
            from: "開始",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "包含此時間點，使用此瀏覽器的時區 — 例如 2026-09-23T10:00。留空表示不設界線。",
            to: "結束",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "不包含此時間點，使用此瀏覽器的時區 — 例如 2026-09-23T10:00。留空表示不設界線。",
            clearFilters: "清除篩選",
            usageBackLabel: "用量",
            theQueryDidNotRunAFilterValueIsInvalid: "查詢未執行 — 某個篩選值無效。",
            total: "總計",
            nothingRecordedInThisWindowMessage: "此時間範圍內沒有任何記錄。",
            noRowsInThisWindowMessage: "此時間範圍內沒有任何資料列。",
            bucket: "分組",
            requests: "請求",
            input: "輸入",
            output: "輸出",
            cached: "快取",
            cacheWrite: "快取寫入",
            reasoning: "推理",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "權杖加總會略過上游未回報用量的資料列 — 缺少的細項絕不會灌大或縮小總計。",
            querying: "正在查詢…",
            runQuery: "執行查詢",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "沒有篩選條件或界線可縮小查詢範圍，因此空的回答就代表閘道的全部記錄 — 目前還沒有任何用量。",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "沒有記錄符合這些篩選條件 — 請放寬界線、移除篩選條件，或在不加篩選的情況下查詢。上方表單中可找到「清除篩選」。",
        },
        usageScreen: {
            usage: "用量",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "每個已派送的請求，最新在前 — 只有數字，絕不含內容。未回報任何可選用量數字的資料列會整批略過：全為零的尾端與未回報的尾端在此無法分辨。",
            viewAggregates: "檢視彙總",
            filter: "篩選",
            theSharedAccountingFilterTheSameOneTheMessage:
                "共用的計量篩選條件 — 與彙總頁面所限縮的是同一組。",
            window: "時間範圍",
            rollingResolvedWhenTheStreamIsReadMessage: "滾動式，在讀取串流時決定。",
            clearFilters: "清除篩選",
            noRequestsRecordedYet: "尚無請求記錄",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "每個已派送的服務請求都會出現一個資料列 — 就在閘道送往上游的那一刻。若有流量卻沒有出現任何資料列，請檢查閘道設定檔中的 api_logs 區段。",
            noRecordedRequestsMatchThisFilterMessage: "沒有已記錄的請求符合此篩選條件。",
            filtersStayBehind: " — 時間範圍與所有其他篩選條件都保留於此處。",
            streamMark: " · 串流",
            thisRowIsNoLongerListed: "此資料列已不在清單中。",
            reading: "正在讀取…",
            applyFilters: "套用篩選",
            widenTheWindowDropAFilterOrReadMessage:
                "請放寬時間範圍、移除篩選條件，或讀取整份串流 — 上方表單中可找到「清除篩選」。",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "閘道的篩選是用來找出資料列；它不會驗證值是否符合，因此未知的 id 或從未記錄過的狀態只會找不到任何結果。",
            thatIsEveryRowThisFilterMatchesMessage: "這就是此篩選條件符合的每一個資料列。",
            thatIsEveryRecordedRowMessage: "這就是每一個已記錄的資料列。",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "目前有讀取循環正在取代串流 — 待循環完成後再延伸。",
            loading: "正在載入…",
            loadMore: "載入更多",
        },
        usageStats: {
            usage: "用量",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "每日請求數與權杖數 — 只有數字，絕不含內容。",
            openAggregates: "開啟彙總",
            viewRequests: "檢視請求",
            requests: "請求",
            dispatchedAllDays: "已派送，涵蓋所有日期",
            inputTokens: "輸入權杖",
            summedUsageReported: "加總，已回報用量",
            outputTokens: "輸出權杖",
            day: "日期",
            input: "輸入",
            output: "輸出",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage: "日期以 UTC 計算 — 這是閘道自身的日界。",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage: "上游已不再回報此模型。",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "此帳戶已擁有該確切的上游模型。請開啟其現有的模型清單資料列。",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "另一個公開模型路由已使用該公開 ID。請改用明確的不同名稱，或編輯現有的路由。",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "另一個帳戶池已使用該名稱。請改用其他帳戶池名稱。",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "仍有公開模型路由參照此帳戶池。請先變更或刪除那些路由，再刪除此帳戶池。",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne:
            "另一個金鑰已擁有該密鑰；請產生新的密鑰。",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "已儲存的版本已變更 — 請使用上方工具列的「重新整理」，這會重新讀取已儲存的版本，且不會動到此表單 — 然後再次提交。",
        chooseAProviderNameNoOtherRowHolds: "請選擇沒有其他資料列使用的上游帳戶名稱。",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "在流程完成前，另一個上游帳戶已取用此名稱。「重新開始」會結束此流程並釋出該名稱；之後再以沒有其他資料列使用的名稱建立。",
        chooseAnIdentifierNoOtherRowHolds: "請選擇沒有其他資料列使用的識別碼。",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "若上游帳戶的授權已失效，請在該帳戶的編輯頁面重新連線；若上游確實尚未列出任何模型，請從此頁面手動新增一個。",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "若上游帳戶的基礎 URL 或 API 金鑰有誤，請在該帳戶的編輯頁面更正；若上游確實尚未列出任何模型，請從此頁面手動新增一個。",
        checkTheProvidersStateAndCredentials: "請檢查上游帳戶的狀態與認證資訊。",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "上游拒絕了認證資訊交換；請從此頁面重新開始流程。",
        checkTheProvidersCredentialsAndReachability: "請檢查上游帳戶的認證資訊與可連線性。",
        nothingAnsweredAtBaseUrl: "[[baseUrl]] 沒有任何回應。",
        theBrowserCouldNotCompleteTheConnection: "瀏覽器無法完成連線。",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "請檢查位址、確認閘道正在執行，且其 listen 位址可從此瀏覽器連線。",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl: "HTTPS 頁面無法呼叫 HTTP 位址 [[baseUrl]]。",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "此頁面透過 HTTPS 提供，而瀏覽器會直接封鎖對 HTTP 位址的呼叫，因此從未連上閘道。",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "請也透過 HTTP 提供此應用程式，或在該執行個體的設定檔中加入 tls 區段，讓閘道以 HTTPS 回應。",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "閘道已回應，但瀏覽器封鎖了來自 [[baseUrl]] 的回應。",
        theBrowserWithheldTheResponseFromThisPage: "瀏覽器未將回應交給此頁面。",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "請將 [[origin]] 加入該執行個體設定檔的 [[setting]]，並重新啟動閘道。由閘道本身的來源提供此應用程式，即可完全免除允許清單。",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "請檢查位址、確認閘道正在執行，且可從此瀏覽器以此通訊協定連線。",
        noAnswerFromBaseUrl: "[[baseUrl]] 沒有回應。",
        theBrowserReceivedNoAnswerItCouldRead: "瀏覽器沒有收到可讀取的回應。",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "該位址可能沒有任何服務在接聽，或瀏覽器在頁面讀取前就封鎖了回應 — 請將 [[origin]] 加入 [[setting]]，並重新啟動閘道。",
        theGatewayRejectedThatManagementToken: "閘道拒絕了該管理權杖。",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "權杖必須與該執行個體設定檔中的 management_api.token 相符；閘道會即時讀取，因此變更權杖後無須重新啟動。",
        theGatewayHoldsNoSuchRow: "閘道中沒有這個資料列。",
        theGatewayRefusedAnIdentifierIsAlreadyTaken: "閘道拒絕了請求：某個識別碼已被使用。",
        theUpstreamProviderRefusedTheCallOrWasUnreachable: "上游帳戶拒絕了呼叫，或無法連線。",
        theGatewayFailedInternally: "閘道內部發生錯誤。",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "原因記錄在閘道的 stderr 記錄中；內部原因永遠不會回傳至此頁面。",
        theGatewayRejectedTheRequest: "閘道拒絕了請求。",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow: "閘道回應了此版本無法辨識的資料結構。",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "此版本產生的繫結與閘道的管理 API 不一致；請重新產生繫結，或以相符的版本重新建置此前端。",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "閘道回應了 HTTP {{httpStatus}}，但沒有管理 API 的錯誤內容。",
        checkTheGatewaysStderrLogForTheCause: "請查看閘道的 stderr 記錄以找出原因。",
        theCallFailed: "呼叫失敗。",
    },
    notices: {
        savedName: "已儲存 [[name]]。",
        credentialsReplaced: "認證資訊已取代。",
        expiryReplaced: "上游帳戶的清單現在會顯示新的到期時間。",
        keyReplaced: "上游 API 金鑰已取代。",
        previousSecretRevoked: "先前的密鑰已在這次儲存後失效 — 請更新所有持有該密鑰的用戶端。",
        lastKeyDisabled: "這是唯一已啟用的金鑰：在啟用其他金鑰前，閘道不會提供任何服務。",
        modelSaved: "模型已儲存。",
        poolSaved: "帳戶池已儲存。",
        routeSaved: "公開模型路由已儲存。",
        discovered_other: "已從 [[providerName]] 探索到 [[quantity]] 個模型。",
        nothingDiscovered: "未從 [[providerName]] 探索到任何新項目。",
        manualUntouched_other: "{{quantity}} 個手動新增的項目維持不變。",
        vanished_other: "{{quantity}} 個模型現在已列為不再回報。",
    },
    connectionValidation: {
        empty: "請輸入 JanusGate 基礎 URL。",
        notAbsolute: "請輸入絕對 URL，例如 https://gateway.example.com。",
        scheme: "請使用 http:// 或 https:// URL。",
        credentials: "請從 URL 中移除認證資訊。",
        notOrigin: "僅輸入閘道來源，不要包含路徑、查詢或片段。",
        managementToken: "請輸入管理權杖。",
        nameEmpty: "名稱不能是空的。",
    },
    operationNotices: {
        flowFailed: "流程失敗。",
        freshFlow: "請重新開始一個流程，並儘快完成授權。",
        flowExpired: "流程已過期。",
        flowExpiryPassed: "此頁面等待結果期間，流程的有效期限已過。",
        pollStopped: "結果輪詢持續失敗，因此此頁面已停止查詢。",
        pollFailed: "最後一次結果輪詢失敗。",
        invalidDefinition: "定義 JSON 不是閘道能儲存的定義。",
        fixDefinition: "請修正 JSON 後重新提交；沒有寫入任何內容。",
        fixFilter: "請修正該欄位後重新執行查詢。",
        discoveryPending: "正在從 [[providerName]] 探索模型…",
        discoveryQuery: "正在詢問上游目前回報的內容。",
    },
    counts: {
        models_other: "{{total}} 個模型",
        filteredModels_other: "{{total}} 個模型中的 {{shown}} 個",
        enabledKeys_other: "{{total}} 個金鑰中有 {{enabled}} 個已啟用",
        shownKeys_other: "顯示 {{shown}} 個金鑰",
        filteredKeys_other: "[[shownSentence]] · [[total]] 個金鑰中有 [[enabled]] 個已啟用",
        providerDeleted_other:
            "刪除會一併移除此帳戶、其帳戶池成員資格，以及其 {{total}} 個實體模型與其直接路由。帳戶池路由仍會保留。",
        requests_other: "{{total}} 次請求",
        requestsPeak_other: "[[total]] 次請求 · [[day]] 的尖峰為 [[peak]]",
    },
    helpers: {
        accessControl: {
            allowed: "已允許",
            denied: "已拒絕",
            modelOverride: "模型覆寫值",
            globallyDisabled: "全域已停用",
            targetUnavailable: "目標無法使用",
            targetVanished: "目標已消失",
            globallyAvailable: "全域可用",
            noAvailablePoolMembers: "沒有可用的帳戶池成員",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "基礎 URL 必須是 http:// 或 https:// 位址，且不能有空格。",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "任何使用靜態金鑰、採用 OpenAI Responses 通訊協定的端點。",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "任何使用靜態金鑰、採用 Chat Completions 通訊協定的端點。",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "任何使用靜態金鑰、採用 Gemini GenerateContent 通訊協定的端點。",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "任何使用靜態金鑰、採用 Gemini Interactions 通訊協定的端點。",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "任何使用靜態金鑰、採用 Anthropic Messages 通訊協定的端點。",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "使用靜態金鑰的 xAI Responses 端點：原生壓縮採用獨立的 /responses/compact 操作，而明確觸發與自動壓縮則宣告為不支援 — 獨立的壓縮端點並不代表原生觸發或自動壓縮受到支援。",
            claudeCodeClaudeAccount: "Claude Code（Claude 帳戶）",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "透過 Messages API 連線的 Claude 訂閱帳戶：OAuth 權杖會以 Authorization: Bearer 傳送，並搭配 oauth-2025-04-20 beta 標頭，推論與模型探索皆同。",
            kimiCodeKimiAccount: "Kimi Code（Kimi 帳戶）",
            githubCopilotGitHubAccount: "GitHub Copilot（GitHub 帳戶）",
            devinDevinAccount: "Devin（Devin 帳戶）",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "透過 ConnectRPC 權杖交換取得靜態 API 金鑰與 API 伺服器 URL 的 Devin 帳戶 — 不需重新整理、不會過期；模型需手動新增。",
            codexChatGPTAccount: "Codex（ChatGPT 帳戶）",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "透過 Codex Responses 端點連線的 ChatGPT 帳戶。上游強制使用串流，回應則以 OpenAI Responses 格式呈現。壓縮區段宣告了 Codex 後端接受的明確觸發與自動閾值壓縮。",
            antigravityGoogleAccount: "Antigravity（Google 帳戶）",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "透過 Antigravity Gemini 端點連線的 Google 帳戶，具備專案初始設定與經過封裝的 GenerateContent 請求。",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "使用靜態金鑰、以 OpenAI Chat Completions 通訊協定連線的 Zen。",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "使用靜態金鑰、以 OpenAI Responses 通訊協定連線的 Zen。",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "使用靜態金鑰、以 Anthropic Messages 通訊協定連線的 Zen。",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "使用靜態金鑰、以 Gemini GenerateContent 通訊協定連線的 Zen。",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "貼上的內容既不是單純的驗證碼，也不是含有驗證碼的 URL — 請複製登入後停留頁面的完整位址，或只複製其中的驗證碼，再貼到這裡。",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "流程會持續進行；此頁面會不斷詢問結果。",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "閘道中沒有這個帳戶池。它可能已被刪除，或位址輸入有誤。",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "閘道中沒有這個公開模型路由。它可能已被刪除，或位址輸入有誤。",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "閘道中沒有這個 API 金鑰。它可能已被刪除，或位址輸入有誤。這裡無法將它救回。",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "閘道中沒有這個模型。它可能已被刪除，或位址輸入有誤 — 已刪除的探索模型會在下次探索時以新模型之姿回來。",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "閘道中沒有這個上游帳戶。它可能已被刪除，或位址輸入有誤。這裡無法將它救回。",
        },
        app: {
            readThisScreensStateFromTheGatewayAgain: "再次從閘道讀取此頁面的狀態",
            thisScreenReadsNothingFromTheGatewaySo:
                "此頁面不會從閘道讀取任何內容，因此沒有可重新整理的東西",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "要中斷連線並清除已儲存的權杖嗎？重新連線時必須再次輸入權杖。",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "必填。指出哪個用戶端持有此金鑰。",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "必填。取用服務的用戶端會在每個介面上用它進行驗證。",
        },
        clipboard: {
            copyToClipboard: "複製到剪貼簿",
            copied: "已複製",
            copy: "複製",
            theValueHere: "此處的值",
        },
        confirmDeleteCard: {
            deleting: "正在刪除…",
            deleteForGood: "永久刪除",
        },
        dirtyFormFooter: {
            saving: "正在儲存…",
            saveChanges: "儲存變更",
        },
        layout: {
            back: "返回",
        },
        providerForm: {
            verifiedLive: "已實測",
            unverified: "未驗證",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "必填。此定義的請求所指向的上游位址。",
            upstreamAPIKey: "上游 API 金鑰",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "必填。每個請求都會送往上游。閘道永遠不會回傳。",
            showTheAPIKeyWhileTyping: "輸入時顯示 API 金鑰",
        },
        apiKeyAccess: {
            inherit: "繼承",
            allow: "允許",
            deny: "拒絕",
            provider: "上游帳戶",
            pool: "帳戶池",
            automatic: "自動",
            alias: "別名",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels: "依循探索結果 — 目前未宣告任何等級。",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "取代已探索的集合 — 空白的選取表示不宣告任何等級。",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "依循探索結果 — 目前未知，因此清單會略過此項。",
            replacesTheDiscoveredValueWithAPositiveInteger: "以正整數取代已探索的值。",
            unset: "未設定",
            discoveredNothingKnown: "已探索：沒有任何已知資訊。",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "依循探索結果 — 目前沒有任何已知資訊，因此由定義中的宣告決定。",
        },
        modelsTable: {
            ready: "就緒",
            off: "關閉",
            suspended: "已暫停",
            heldByProvider: "由上游帳戶持有",
            unknown: "未知",
            noAutomaticEntry: "沒有自動項目",
            noDirectRoutes: "沒有直接路由",
        },
        providersTable: {
            valid: "有效",
            expired: "已過期",
            stale: "從未有效",
            on: "開啟",
            off: "關閉",
        },
        usageFilterControls: {
            allOutcomes: "所有結果",
            failuresUpstreamOrInternal: "失敗（上游或內部）",
            any: "不限",
            streamed: "串流",
            nonstreamed: "非串流",
            allEndpoints: "所有端點",
        },
        usageFilter: {
            allTime: "所有時間",
            last24Hours: "最近 24 小時",
            last7Days: "最近 7 天",
            last30Days: "最近 30 天",
            invalidStatusList:
                "HTTP 狀態必須是以逗號分隔的普通整數，例如 200, 429；留空則不依狀態篩選。",
        },
    },
    validation: {
        composition: {
            upstreamId: "上游 ID 只能包含英文字母、數字與符號 — 不能有空格或非 ASCII 字元。",
            publicId: "公開 ID 只能包含英文字母、數字與符號 — 不能有空格或非 ASCII 字元。",
            modelPrefix: "模型前綴只能包含英文字母、數字與符號 — 不能有空格或非 ASCII 字元。",
            apiKey: "API 金鑰只能包含英文字母、數字與符號 — 不能有空格或非 ASCII 字元。",
            key: "金鑰只能包含英文字母、數字與符號 — 不能有空格或非 ASCII 字元。",
        },
        routing: {
            upstreamId:
                '上游 ID 只能包含英文字母、數字與符號 — 不能有空格、非 ASCII 字元、"/" 或 ":"。',
            publicId:
                '公開 ID 只能包含英文字母、數字與符號 — 不能有空格、非 ASCII 字元、"/" 或 ":"。',
            modelPrefix:
                '模型前綴只能包含英文字母、數字與符號 — 不能有空格、非 ASCII 字元、"/" 或 ":"。',
            apiKey: 'API 金鑰只能包含英文字母、數字與符號 — 不能有空格、非 ASCII 字元、"/" 或 ":"。',
            key: '金鑰只能包含英文字母、數字與符號 — 不能有空格、非 ASCII 字元、"/" 或 ":"。',
        },
        positive: {
            maxInputOverride: "輸入權杖上限的覆寫值必須是正整數。",
            maxOutputOverride: "輸出權杖上限的覆寫值必須是正整數。",
        },
    },
    quotaMessages: {
        day_other: "{{quantity}} 天",
        hour_other: "{{quantity}} 小時",
        minute_other: "{{quantity}} 分鐘",
        second_other: "{{quantity}} 秒",
        used: "已使用 {{percentage}}",
        counter: "已使用 [[used]] / [[limit]] [[unit]]",
        remaining: "還剩 [[remaining]] [[unit]]",
        dateReset: "[[date]] · 確切重設時間未知",
        atReset: "於 [[instant]] 重設 · [[distance]]",
        future: "{{duration}}後",
        observed: "觀測於 [[instant]] · [[distance]]",
        inventory: {
            pool_disabled: "帳戶池已停用",
            account_missing: "帳戶不存在",
            account_disabled: "帳戶已停用",
            model_not_selected: "選擇上游 ID 以檢查模型可用性",
            model_missing: "模型不存在",
            model_disabled: "模型已停用",
            model_vanished: "模型已消失",
            ready: "模型清單就緒",
        },
        failure: {
            credential_renewal: "認證資訊續期失敗",
            timeout: "逾時",
            transport: "傳輸失敗",
            upstream_rejected: "上游拒絕了請求",
            malformed_observation: "觀測資料格式錯誤",
        },
        quotaObserved: "配額觀測於 [[instant]]",
        quotaFailed: "讀取配額失敗 · {{reason}}",
        deletedAccount: "已刪除的帳戶 #{{id}}",
        priority: "優先順序 {{priority}}",
        models: "模型：[[identifiers]]",
        resets: "於 [[instant]] 重設",
    },
    dynamicMessages: {
        following: "依循探索結果：{{value}}。",
        levelsReplace: "取代已探索的集合 — 用戶端看到的是 {{value}}。",
        capReplace: "取代已探索的值 — 用戶端看到的是 {{value}}。",
        mechanismFact: "{{mechanism}}：{{support}}",
        mechanismDiscovered: "已探索：{{support}}。",
        compactionReplace:
            "整批取代已探索的資訊 — 未設定的機制會先回退到探索結果，再回退到定義中的宣告。",
        noExpiry: "此上游帳戶沒有儲存任何到期時間。",
        goneFlow: "此授權流程已無法完成 — 其 state 已過期或已被使用。",
        refusedClipboard: "此瀏覽器拒絕了剪貼簿操作 — 請選取{{subject}}並手動複製。",
        enabledStored: "已儲存 {{total}} 個中的已啟用數量",
        readyListed: "已列出 {{total}} 個中的就緒數量",
        copyEndpoint: "複製 {{surface}} 端點 URL",
        copyApiKeySecret: "複製 {{name}} 的密鑰",
        disabledCredential: "{{expiry}} 上游帳戶已停用 — 重新啟用時請重新連線。",
        noKeysDetail: " 取用服務的用戶端需要 API 金鑰才能驗證 — 少了它，閘道就沒有就緒的模型清單。",
    },
    remainingText: {
        notRefreshedYet: "尚未重新整理",
        overrideMaxInputTokens: "覆寫輸入權杖上限",
        maxInputTokensOverride: "輸入權杖上限覆寫值",
        overrideMaxOutputTokens: "覆寫輸出權杖上限",
        maxOutputTokensOverride: "輸出權杖上限覆寫值",
        chooseAProvider: "選擇上游帳戶",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "指定上游帳戶與上游已不再回報的上游 ID 後，即可依下方設定取回該模型。此模型會透過其獨立上游帳戶前綴或所屬帳戶池前綴自動公開。",
        allProviders: "所有上游帳戶",
        replaceTheUpstreamAPIKey: "取代上游 API 金鑰",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "僅供寫入：閘道永遠不會回傳，因此留空會保留已儲存的內容。",
        backToPools: "返回帳戶池",
        publicRoute: "公開模型路由",
        publicRoutes: "公開模型路由",
        backToPublicRoutes: "返回公開模型路由",
        backToAPIKeys: "返回 API 金鑰",
        backToModels: "返回模型",
        backToProviders: "返回上游帳戶",
        deletedRoute: "已刪除的路由",
        deletedModel: "已刪除的模型",
        deletedProvider: "已刪除的上游帳戶",
        deletedKey: "已刪除的金鑰",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "下方的回答仍是前一次查詢的結果 — 顯示行內錯誤的欄位說明了問題所在。",
        fromAndToDoNotApplyThere: " — 「開始」與「結束」不適用於該選項。",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "每個已派送的服務請求都會出現一個資料列 — 就在閘道送往上游的那一刻。",
        showTheKeyWhileTyping: "輸入時顯示金鑰",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "儲存會以你的編輯覆寫閘道上的版本；還原則會採用閘道上的版本。",
        theOAuthCredentialsWereNeverFresh: "OAuth 認證資訊從未有效。",
        allPublicRoutes: "所有公開模型路由",
        allPhysicalModels: "所有實體模型",
        allAPIKeys: "所有 API 金鑰",
    },
    entityMessages: {
        editName: "編輯 [[name]]",
        editIdentifier: "編輯 [[identifier]]",
        deleteKey: "要刪除 [[name]] 嗎？持有此密鑰的用戶端將失去存取權。",
        deleteLastKey:
            "要刪除 [[name]] 嗎？這是唯一已啟用的金鑰 — 在啟用其他金鑰前，閘道不會提供任何服務。",
        deleteModel:
            "要刪除 [[identifier]] 嗎？其直接路由也會一併刪除。帳戶池路由仍會保留。重新探索或手動建立會產生新的實體資料列，且不會還原已刪除的路由。",
        deleteProvider: "要刪除 [[name]] 嗎？",
        switchPreset: "要切換至 [[name]] 嗎？你編輯過的連線設定將被取代。",
        noKeysMatch: "沒有 API 金鑰符合「[[query]]」。",
        noProvidersMatch: "沒有上游帳戶符合「[[query]]」。",
        noModelsMatch: "沒有模型符合「[[query]]」。",
        noProviderModelsMatch: "此上游帳戶沒有符合「[[query]]」的模型。",
        missingPath: "[[path]] 沒有任何內容。它可能已被刪除，或位址輸入有誤。",
        aliasCreated: "已建立別名：[[identifier]]",
        addAlias: "為 [[identifier]] 新增別名",
    },
    modelCard: {
        heading: "模型 · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "此金鑰已在閘道上變更，而你還有尚未儲存的編輯。",
            provider: "此上游帳戶已在閘道上變更，而你還有尚未儲存的編輯。",
            model: "此模型已在閘道上變更，而你還有尚未儲存的編輯。",
            pool: "此帳戶池已在閘道上變更，而你還有尚未儲存的編輯。",
            "public route": "此公開模型路由已在閘道上變更，而你還有尚未儲存的編輯。",
            "api key": "此 API 金鑰已在閘道上變更，而你還有尚未儲存的編輯。",
        },
        noRequests: {
            key: "此金鑰尚無請求記錄。",
            provider: "此上游帳戶尚無請求記錄。",
            model: "此模型尚無請求記錄。",
            pool: "此帳戶池尚無請求記錄。",
            "public route": "此公開模型路由尚無請求記錄。",
            "api key": "此 API 金鑰尚無請求記錄。",
        },
        notFound: {
            api_key: "找不到 API 金鑰",
            provider: "找不到上游帳戶",
            model: "找不到模型",
            pool: "找不到帳戶池",
            model_route: "找不到公開模型路由",
        },
        replaceSecret: {
            typed: "要取代已輸入的密鑰嗎？",
            current: "要取代目前的密鑰嗎？",
        },
        bound: {
            From: "「開始」必須是完整的日期與時間，例如 2026-09-23T10:00 — 留空表示不設界線。",
            To: "「結束」必須是完整的日期與時間，例如 2026-09-23T10:00 — 留空表示不設界線。",
        },
        updated: "更新於 {{instant}}",
        created: "建立於 {{instant}}",
        version: "已儲存為版本 {{version}}。",
        unlisted: "{{value}}（已不在清單中）",
        accountUnlisted: "帳戶 #{{id}}（已不在清單中）",
        moveUp: "將 {{name}} 上移",
        moveDown: "將 {{name}} 下移",
        remove: "移除 {{name}}",
        publicIdFor: "{{subject}} 的公開 ID",
        source: {
            key: "來自金鑰",
            provider: "來自上游帳戶",
            pool: "來自帳戶池",
            route: "模型覆寫值",
        },
        deleted: {
            provider: "已刪除的上游帳戶 #{{id}}",
            pool: "已刪除的帳戶池 #{{id}}",
            route: "已刪除的路由 #{{id}}",
        },
        status: "用戶端 {{client}} · 上游 {{upstream}}",
        metrics: " · {{duration}} · 輸入 {{input}} · 輸出 {{output}}{{tail}}",
        optionalTokens: " · 快取 {{cached}} · 快取寫入 {{write}} · 推理 {{reasoning}}",
    },
    routingMessages: {
        directRoutes_other: "{{quantity}} 條直接路由",
        poolMembers_other: "[[quantity]] 個帳戶成員 · 模型前綴：[[prefix]]",
        directMissing: "直接 · 實體模型 #[[id]]",
        direct: "直接 · [[provider]] · [[model]]",
        pool: "帳戶池 · [[pool]] · [[model]]",
        publicIds: "公開 ID [[identifiers]]",
        modelAccess: "[[identifier]] 的模型存取權限",
        ownerDefault: {
            provider: "[[name]] 的帳戶預設值",
            pool: "[[name]] 的帳戶池預設值",
        },
        directTarget: "直接 · 模型 #[[id]]",
        poolTarget: "帳戶池 · [[identifier]]",
        reconnect: "請在[[editLink]]重新連線 — 其認證資訊卡片會加以取代。",
        reconnectDisabled:
            "上游帳戶已停用 — 重新啟用時請重新連線；[[editLink]]的認證資訊卡片會加以取代。",
    },
    linkedMessages: {
        keyEndpoint:
            "將用戶端指向某個端點 — 每個介面都會提供獲允許的公開項目；URL 位於[[overviewLink]]。",
        servingKey:
            "取用服務的用戶端會以服務 API 金鑰進行驗證 — 請先在[[keysLink]]建立一個，再把任何用戶端指向這裡。",
        vanished:
            "在此期間不會提供任何服務。上游重新列出此模型時，請[[discoverLink]]，或[[claimLink]]。",
        discoverLabel: "在該上游帳戶頁面重新探索",
        claimLabel: "從「建立模型」頁面取回",
        selected: "已選取 {{quantity}} 個。成功的資料列不會再次提交。",
        kimiDescription:
            "透過 Coding 端點連線的 Kimi 帳戶：裝置碼流程的裝置 ID 會透過自訂 X-Msh-* 標頭傳送，授權、輪詢與重新整理皆同。",
        githubDescription:
            "透過重新佈建交換的 GitHub 帳戶：以長效權杖換取短期的 Copilot 工作階段權杖與該帳戶的 API 端點。",
        chartPoint_other: "[[day]]：[[quantity]] 次請求",
        inventoryEmpty: "[[name]] 尚無任何模型。",
        inventoryUnready: "[[name]] 沒有就緒的模型清單 — 每個模型都已關閉或消失。",
    },
    vocabulary: {
        credentialKind: {
            api_key: "靜態 API 金鑰",
            oauth: "OAuth 帳戶",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "獨立壓縮",
            responses_explicit_trigger: "明確觸發",
            responses_automatic_compaction: "自動壓縮",
            messages_on_demand_compaction: "隨需壓縮",
            messages_threshold_compaction: "閾值壓縮",
        },
        mechanismSupport: {
            supported: "支援",
            unsupported: "不支援",
            unknown: "未知",
        },
        discoveryState: {
            discovered: "已探索",
            vanished: "不再回報",
            manual: "手動新增",
        },
        reasoningLevel: {
            none: "無",
            minimal: "最低",
            low: "低",
            medium: "中",
            high: "高",
            xhigh: "極高",
            max: "最高",
        },
        apiLogOutcome: {
            completed: "已完成",
            client_aborted: "用戶端已中止",
            upstream_error: "上游錯誤",
            internal: "內部",
        },
        apiLogEndpointType: {
            openai_chat_completions: "Chat Completions",
            openai_responses: "Responses",
            openai_responses_compact: "Responses 壓縮",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interactions: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        usageGroupBy: {
            day: "日期",
            model: "實體模型",
            model_route: "公開模型路由",
            provider: "上游帳戶",
            api_key: "API 金鑰",
            endpoint_type: "端點",
            outcome: "結果",
            is_stream: "串流模式",
            http_status: "HTTP 狀態",
        },
    },
    common: {
        save: "儲存變更",
        cancel: "取消",
        retry: "重試",
        language: "語言",
    },
    language: {
        system: "跟隨瀏覽器",
        loading: "正在載入 {{languageName}}…",
        failed: "無法載入 {{languageName}}。目前仍顯示你目前的語言。",
        startupFailed: "無法載入 {{languageName}}。目前顯示英文。",
    },
    models: {
        discoveredCount_other: "已探索 {{count}} 個模型。",
    },
} as const;
export default traditionalChinese;
