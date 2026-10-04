/** Simplified Chinese administration messages. */
const chinese = {
    shell: {
        skipToContent: "跳转到主要内容",
        primary: "主要导航",
        refresh: "刷新",
        disconnect: "断开连接",
        keep: "保留",
    },
    connection: {
        connectToJanusGate: "连接 JanusGate",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "此应用用于管理 JanusGate 网关中的上游账户、模型和凭据。",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "一个正在运行且可从此浏览器访问的 JanusGate 实例。",
        theOriginBesideIt: "旁边的源地址",
        copyThisAppsOrigin: "复制此应用的源地址",
        gatewayAddress: "网关地址",
        schemeHostAndPortOnly: "仅填写协议、主机和端口。",
        managementToken: "管理令牌",
        sentOnlyToThatInstanceAndKeptOnly: "仅发送给该实例，并仅保存在此浏览器中。",
        cancel: "取消",
    },
    controls: {
        modelAccess: "模型访问权限",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "默认权限会影响当前继承该权限的条目。账户池权限也适用于以后符合条件的成员。全局可用性与访问权限分别管理。",
        defaultModelAccessLabel: "默认模型权限：",
        defaultModelAccess: "默认模型权限",
        appliesToEntriesWithoutAProviderPoolOr:
            "适用于未设置上游账户、账户池或模型覆盖权限的条目，包括以后新增的条目。",
        searchModelAccess: "搜索模型权限",
        publicIDOrOwnerName: "公共 ID 或所属名称",
        overridesOnly: "仅显示覆盖值",
        removeOverride: "移除覆盖值",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "保存前请移除已删除条目的引用，或刷新以重新核对模型清单。",
        thisDefaultAppliesToFutureEntries: "此默认权限也适用于以后新增的条目。",
        noEntriesMatchThisViewHiddenOverridesAre: "没有符合筛选条件的条目。隐藏的覆盖值仍会保留。",
        name: "名称",
        key: "密钥",
        skOrGenerateOne: "sk-… 或生成一个密钥",
        generate: "生成",
        replaceIt: "替换",
        keepIt: "保留",
        theSecretHere: "此处的密钥",
        delete: "删除…",
        revert: "还原",
        unsavedChanges: "有未保存的更改",
        overrideReasoningLevels: "覆盖推理级别",
        reasoningLevels: "推理级别",
        pickEveryLevelThisModelDeclaresUntickAll:
            "选择此模型支持的所有推理级别。全部取消选择表示不支持推理。",
        aPositiveIntegerClearTheSwitchToFollow: "请输入正整数。关闭覆盖开关可重新使用发现的值。",
        overrideCompactionFacts: "覆盖压缩能力",
        compactionMechanisms: "压缩机制",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            "设置需要覆盖的压缩机制。“未知”明确表示没有可用信息，并会取代该机制已发现的值。",
        addAlias: "添加别名",
        modelPrefix: "模型前缀",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "未加入账户池时，使用此前缀加上游 ID 调用模型。账户池成员使用账户池的前缀。留空可直接使用上游 ID。更改前缀会更改现有自动模型名称。",
        upstreamBaseURL: "上游基础 URL",
        enabled: "已启用",
        publicRoute: "公共模型路由",
        provider: "上游账户",
        physicalModel: "物理模型",
        apiKey: "API 密钥",
        endpoint: "端点",
        outcome: "结果",
        streamMode: "流式模式",
        httpStatuses: "HTTP 状态",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "用逗号分隔，例如 200, 429。未收到响应的请求不匹配任何状态。",
    },
    navigation: {
        overview: "概览",
        providers: "上游账户",
        inventory: "模型清单",
        pools: "账户池",
        routes: "公共模型路由",
        apiKeys: "API 密钥",
        usage: "用量",
        lightTheme: "浅色主题 — 切换为深色",
        darkTheme: "深色主题 — 跟随系统",
        systemTheme: "系统主题 — 切换为浅色",
    },
    rich: {
        connected: "已连接 [[baseUrl]] — 令牌仅保存在此浏览器中。",
        providerModel: "[[modelId]] 所属的上游账户为 [[providerLink]]。",
        setupToken: "实例配置文件中 [[setting]] 的管理令牌。",
        setupCors: "在实例的 [[setting]] 中允许此应用的源地址，除非此应用由网关自身提供。",
    },
    titles: {
        bulkPublication: "发布模型清单",
        providerPools: "账户池",
        providerPoolNew: "创建账户池",
        providerPoolEdit: "编辑账户池 #{{id}}",
        modelRoutes: "公共模型路由",
        modelRouteNew: "创建公共模型路由",
        modelRouteEdit: "编辑公共模型路由 #{{id}}",
        home: "概览",
        providers: "上游账户",
        providerNew: "创建上游账户",
        providerDetail: "上游账户 #{{id}}",
        providerEdit: "编辑上游账户 #{{id}}",
        models: "模型",
        modelNew: "创建模型",
        modelDetail: "模型 #{{id}}",
        modelEdit: "编辑模型 #{{id}}",
        apiKeys: "API 密钥",
        apiKeyNew: "创建 API 密钥",
        apiKeyDetail: "API 密钥 #{{id}}",
        apiKeyEdit: "编辑 API 密钥 #{{id}}",
        usage: "用量",
        usageQuery: "用量汇总",
        notFound: "页面不存在",
    },
    format: {
        lessThanMinute: "不到一分钟",
        minutes: "{{minutes}} 分钟",
        hours: "{{hours}} 小时",
        hoursMinutes: "{{hours}} 小时 {{minutes}} 分钟",
        days: "{{days}} 天",
        daysHours: "{{days}} 天 {{hours}} 小时",
        expired: "已于 {{distance}}前过期 — 过期时间为 {{instant}}。",
        valid: "有效期还剩 {{distance}} — 将于 {{instant}}过期。",
        durationSeconds: "{{seconds}} 秒",
        durationMinutesSeconds: "{{minutes}} 分 {{seconds}} 秒",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "API 密钥",
            editAPIKey: "编辑 API 密钥",
            secret: "密钥",
            overview: "概览",
            details: "详情",
            name: "名称",
            created: "创建时间",
            status: "状态",
        },
        apiKeyEditScreen: {
            editAPIKey: "编辑 API 密钥",
            settings: "设置",
            rotatingMeansGenerateThenSaveMessage: "轮换密钥时，请先生成新密钥，再保存。",
            enabled: "已启用",
            replacingTheSecretRevokesTheOldValueOnMessage:
                "保存后，新密钥将取代旧密钥，旧密钥随即失效。",
            deleteThisKey: "删除此密钥",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "删除后密钥立即失效，持有此密钥的客户端将收到 401 响应。该密钥的模型权限规则也将移除，历史用量记录会保留。",
        },
        apiKeyNewScreen: {
            createAPIKey: "创建 API 密钥",
            oneSecretYourServingClientsShareMessage: "供推理客户端使用的密钥。",
            key: "密钥",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "密钥只能包含英文字母、数字和符号，不能包含空格，且不得与其他密钥重复。",
            enabled: "已启用",
            loadModelAccessInventoryBeforeCreatingAKeyMessage: "创建密钥前，请先加载模型权限清单。",
        },
        apiKeysScreen: {
            apiKeys: "API 密钥",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "推理密钥。OpenAI 客户端通过 Authorization: Bearer <key> 发送，Gemini 使用 x-goog-api-key，Anthropic 使用 x-api-key。打开密钥可查看其值和用量。",
            createAPIKey: "创建 API 密钥",
            noAPIKeysYet: "暂无 API 密钥",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "创建第一个密钥后，网关才能提供推理服务。",
            searchAPIKeys: "搜索 API 密钥",
            searchByNameOrKey: "按名称或密钥搜索",
            theSecretHere: "此处的密钥",
        },
        bulkPublicationScreen: {
            createAliases: "创建别名",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "选择物理模型并设置公共 ID。每个选中模型将创建一条已启用的直接路由；名称冲突会单独报告，不会自动重命名或覆盖。",
            noPhysicalModelsToAlias: "没有可创建别名的物理模型",
            discoverOrAddInventoryFirstMessage: "请先发现或添加模型。",
            openRoute: "打开路由",
            createSelectedAliases: "创建选中的别名",
        },
        homeScreen: {
            overview: "概览",
            theHealthOfThisGatewayAtAGlanceMessage: "一览网关的运行状态。",
            createProvider: "创建上游账户",
            providers: "上游账户",
            publicRoutes: "公共模型路由",
            inventoryReady: "就绪模型",
            apiKeys: "API 密钥",
            recentRequests24h: "最近请求（24 小时）",
            recentErrors24h: "最近错误（24 小时）",
            requestsPerDay: "每日请求数",
            theLast14DaysInUTCTheGatewaysMessage: "最近 14 天，按网关使用的 UTC 日界统计。",
            openUsage: "查看用量",
            storedSettingsReviewedMessage: "已检查保存的设置。",
            needsAttention: "需要处理",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "没有可用模型。请在已启用的上游账户上发现模型。",
            noAPIKeysYetMessage: "暂无 API 密钥。",
            endpoints: "端点",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "所有协议端点都可调用所有模型。将 OpenAI、Gemini 或 Anthropic 协议的客户端指向此实例即可。",
            theURLBesideIt: "旁边的 URL",
            noProvidersYet: "暂无上游账户",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "每个上游账户对应一个供应商账户。发现模型后，其模型将出现在清单中。",
            createYourFirstProvider: "创建第一个上游账户",
            connectedToTheGateway: "已连接网关",
            createAProvider: "创建上游账户",
            discoverItsModelsFromUpstream: "从上游发现模型",
            issueAnAPIKeyForServingClients: "为推理客户端创建 API 密钥",
            pointAClientAtAnEndpointBelow: "将客户端指向下方的端点",
        },
        modelDetailScreen: {
            model: "模型",
            capabilities: "能力",
            whatTheModelListingsDeclareForThisModelMessage:
                "模型列表对该模型声明的能力。这些值不限制请求，实际响应由上游决定。",
            reasoningLevels: "推理级别",
            maxInputTokens: "最大输入 token 数",
            maxOutputTokens: "最大输出 token 数",
            compaction: "压缩",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "此模型的压缩能力使用覆盖值，未设置时使用最近发现的值。两者均未声明的部分由供应商定义决定。",
            addAlias: "添加别名",
            editModel: "编辑模型",
            details: "详情",
            publicModelNames: "公共模型名称",
            directRoutes: "直接路由",
            upstreamID: "上游 ID",
            provider: "上游账户",
            discovery: "发现状态",
            status: "状态",
        },
        modelEditScreen: {
            editModel: "编辑模型",
            modelSettings: "模型设置",
            enabled: "已启用",
            deleteThisModel: "删除此模型",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "删除将移除此物理模型及其直接路由，账户池路由会保留。",
        },
        modelNewScreen: {
            createModel: "创建模型",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "手动添加无法发现的模型，或恢复上游不再报告的模型。",
            noProvidersYet: "暂无上游账户",
            aModelBelongsToAProviderAndThereMessage:
                "模型必须属于一个上游账户。请先创建上游账户，发现操作可能已能找到所需模型。",
            createProvider: "创建上游账户",
            aModelAddedByHandIsNeverRemovedMessage:
                "手动添加的模型不会自动移除，也可恢复发现时丢失的模型。",
            identity: "标识",
            provider: "上游账户",
            upstreamID: "上游 ID",
            modelSettings: "模型设置",
            enabled: "已启用",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "此模型名称由系统自动管理。编辑所属上游账户或账户池的前缀即可更改。",
            deleteThisPublicRoute: "删除此公共模型路由",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "请求此公共 ID 的客户端将收到模型不存在的错误。物理模型清单和账户池会保留。",
            publicRouteSettings: "公共模型路由设置",
            enabled: "已启用",
            publicID: "公共 ID",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "客户端请求的精确模型名称。重命名后，客户端也需要更新。",
            targetType: "目标类型",
            pool: "账户池",
            upstreamID: "上游 ID",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "向每个成员请求的精确标识。成员缺少模型不会阻止发布。",
            physicalModel: "物理模型",
            createPublicRoute: "创建公共模型路由",
        },
        modelsScreen: {
            models: "模型",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "模型通过独立上游账户或所属账户池的前缀自动发布。",
            createAliases: "创建别名",
            createModel: "创建模型",
            noModelsYet: "暂无模型",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "先创建上游账户，再发现其模型，也可手动添加。",
            createProvider: "创建上游账户",
            searchModels: "搜索模型",
            searchByUpstreamIDOrProviderName: "按上游 ID 或账户名称搜索",
            provider: "上游账户",
        },
        notFoundScreen: {
            pageNotFound: "页面不存在",
            goToOverview: "前往概览",
            providers: "上游账户",
            models: "模型",
        },
        oauthFlowView: {
            flowExpires: "流程过期时间",
            singleuseStateLabel: "一次性 state：",
            authorizationURL: "授权 URL",
            openTheAuthorizationPage: "打开授权页面",
            theURLAbove: "上方的 URL",
            copyTheURL: "复制 URL",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "state 只能使用一次，流程很快就会过期。批准访问后请及时完成。",
            mostLoginsEndOnAPageThatShowsMessage:
                "多数登录流程最后会显示空白页面，这是正常现象。请粘贴浏览器中的完整地址；如果页面显示授权码，则粘贴该授权码。",
            startOver: "重新开始",
            callbackURLOrAuthorizationCode: "回调 URL 或授权码",
            pasteTheWholeCallbackURL: "粘贴完整回调 URL",
            verificationCode: "验证码",
            theVerificationCode: "验证码",
            copyTheCode: "复制验证码",
            openTheVerificationPage: "打开验证页面",
            theVerificationURL: "验证 URL",
            approveTheCodeAtTheVerificationPageThisMessage:
                "在验证页面确认验证码。此页面会持续查询网关，并在流程完成后自动继续。",
        },
        poolAvailabilityCard: {
            refreshQuota: "刷新配额",
            resetRestrictions: "清除限制",
            quotaExhausted: "配额已耗尽",
            poolAvailability: "账户池可用性",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "显示已保存的观测结果。刷新会查询上游账户。清除限制会解除限制并设置刷新宽限期，不会重置上游配额。",
            thisPoolHasNoMembersMessage: "此账户池没有成员。",
        },
        providerDetailScreen: {
            provider: "上游账户",
            editProvider: "编辑上游账户",
            editPage: "编辑页面",
            storedAvailability: "已保存的可用性",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "此账户的所有已观测限制。清除限制会解除限制并设置刷新宽限期，不会重置上游配额。",
            details: "详情",
            modelPrefix: "模型前缀",
            type: "类型",
            definition: "供应商定义",
            upstreamBaseURL: "上游基础 URL",
            credentials: "凭据",
            status: "状态",
            discoveredFromUpstreamOrAddedByHandMessage: "从上游发现或手动添加的模型。",
            createModel: "创建模型",
            allModels: "所有模型",
        },
        providerEditScreen: {
            editProvider: "编辑上游账户",
            settings: "设置",
            credentials: "凭据",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "OAuth 凭据会过期，重新连接可替换凭据。",
            deleteThisProvider: "删除此上游账户",
        },
        providerNewScreen: {
            createProvider: "创建上游账户",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "创建一个上游账户。选择预设后即可按对应方式获取凭据。",
            readAgain: "重新读取",
            preset: "预设",
            switchPreset: "切换预设",
            keepMyEdit: "保留我的修改",
            identity: "标识",
            connection: "连接",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage: "密钥只可写入，网关不会返回其值。",
            connectTheAccount: "连接账户",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "在流程指向的页面批准访问，然后等待完成。此应用不会保存上游凭据。",
            upstreamBaseURL: "上游基础 URL",
            definition: "供应商定义",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "网关保存的连接配置。编辑已保存定义的 JSON 会注册新版本，不会改写原版本。",
            notStoredYetSubmittingCreatesVersion1Message: "尚未保存，提交将创建版本 1。",
            advancedTheConnectionRecipePrefilledByThePreset: "高级：连接配置（预设已填写）",
            definitionJSON: "定义 JSON",
            resetToPreset: "重置为预设",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "连接配置无效，请展开上方高级选项查看出错路径。",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "同一上游服务的可互换账户，按优先级依次尝试。",
            inspectUpstreamID: "检查上游 ID",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "检查已保存的成员关系。留空时仅显示账户级限制。",
            deleteThisPool: "删除此账户池",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "删除会移除账户池的自动条目，并恢复成员的独立前缀。请先删除或更改手动路由的目标。",
            poolSettings: "账户池设置",
            enabled: "已启用",
            name: "名称",
            modelPrefix: "模型前缀",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "账户池通过此前缀加上游 ID 自动发布成员模型。留空可使用精确上游 ID。更改前缀会重命名现有自动条目。",
            accountsInPriorityOrder: "按优先级排列的账户",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "成员必须提供同一上游服务。加入账户池后，其独立前缀条目会隐藏。新模型会自动出现在池中，并仅路由到拥有该模型的成员。账户池可为空。",
            moveUp: "上移",
            moveDown: "下移",
            remove: "移除",
            addAccount: "添加账户",
            addMember: "添加成员",
            createPool: "创建账户池",
        },
        providerQuotaCard: {
            upstreamQuota: "上游配额",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "配额属于上游账户，其中可能包含其他客户端产生的用量。",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage: "此供应商定义未声明配额查询。",
            checkingWhetherThisDefinitionSupportsQuotaReads: "正在检查此定义是否支持查询配额…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "尚未观测配额。刷新会查询上游账户。",
            readingUpstreamQuota: "正在读取上游配额…",
            showingThePreviousReadingMessage: "正在显示上一次读取结果。",
            observed: "观测时间",
            theUpstreamReturnedNoQuotaEntriesMessage: "上游未返回配额条目。",
        },
        providersScreen: {
            providers: "上游账户",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "上游账户。打开账户可查看模型和用量，并进一步进入编辑页面。",
            createProvider: "创建上游账户",
            noProvidersYet: "暂无上游账户",
            aProviderIsOneUpstreamAccountMessage: "每个上游账户对应一个供应商账户。",
            searchProviders: "搜索上游账户",
            searchByNameTypeOrFormat: "按名称、类型或格式搜索",
        },
        readNotice: {
            readAgain: "重新读取",
        },
        routingListsScreen: {
            pools: "账户池",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "由同一上游服务的可互换账户组成的模型集合。",
            createPool: "创建账户池",
            noPoolsYet: "暂无账户池",
            createANamedPoolWithAModelPrefixMessage:
                "创建账户池并设置模型前缀，成员模型将自动发布。",
            publicRoutes: "公共模型路由",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "客户端请求的模型名称。独立上游账户和账户池会自动发布模型名称，也可按需添加别名。",
            createPublicRoute: "创建公共模型路由",
            noPublicRoutesYet: "暂无公共模型路由",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "直接发布物理模型，或指定账户池和精确上游 ID。",
        },
        screenFrame: {
            readAgain: "重新读取",
        },
        usageQueryScreen: {
            usageAggregates: "用量汇总",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "按筛选条件中的维度组合汇总，只包含请求数和 token 总数，不包含请求内容。",
            viewRequests: "查看请求",
            question: "查询条件",
            groupBy: "分组维度",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "可选择任意维度。全部不选时汇总所有匹配记录。",
            window: "时间窗口",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "按当前时刻填写下方时间范围，起点为一个窗口之前，终点留空。仍可手动编辑。",
            from: "起始时间",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "包含此时刻，使用浏览器本地时区，例如 2026-09-23T10:00。留空表示不限制。",
            to: "结束时间",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "不包含此时刻，使用浏览器本地时区，例如 2026-09-23T10:00。留空表示不限制。",
            clearFilters: "清除筛选",
            nothingRecordedInThisWindowMessage: "此时间窗口内没有记录。",
            noRowsInThisWindowMessage: "此时间窗口内没有记录。",
            bucket: "分组",
            requests: "请求数",
            input: "输入",
            output: "输出",
            cached: "缓存读取",
            cacheWrite: "缓存写入",
            reasoning: "推理",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "token 总数跳过上游未报告用量的记录。缺失的明细不会增加或减少总数。",
        },
        usageScreen: {
            usage: "用量",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "所有已发送的请求，按最新优先排列，只包含数值，不包含内容。未报告任何可选用量值的记录不显示明细，不能将未报告的用量视为零。",
            viewAggregates: "查看汇总",
            filter: "筛选",
            theSharedAccountingFilterTheSameOneTheMessage: "与汇总页面共用的用量筛选条件。",
            window: "时间窗口",
            rollingResolvedWhenTheStreamIsReadMessage: "滚动时间窗口，读取记录时计算。",
            clearFilters: "清除筛选",
            noRequestsRecordedYet: "暂无请求记录",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "网关向上游发送每个推理请求时会生成一条记录。如果已有流量却没有记录，请检查网关配置中的 api_logs 部分。",
            noRecordedRequestsMatchThisFilterMessage: "没有符合筛选条件的请求记录。",
        },
        usageStats: {
            usage: "用量",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "每日请求数和 token 数，只包含数值，不包含内容。",
            openAggregates: "查看汇总",
            viewRequests: "查看请求",
            requests: "请求数",
            dispatchedAllDays: "所有日期的已发送请求",
            inputTokens: "输入 token 数",
            summedUsageReported: "已报告用量的合计",
            outputTokens: "输出 token 数",
            day: "日期",
            input: "输入",
            output: "输出",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage: "日期使用 UTC，即网关的日界。",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage: "上游不再报告此模型。",
            itServesNothingMeanwhileMessage: "此期间无法提供推理服务。",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "此账户已拥有该精确上游模型，请打开现有模型条目。",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "另一条公共模型路由已使用此公共 ID。请使用其他名称，或编辑现有路由。",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "另一账户池已使用此名称，请更换账户池名称。",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "公共模型路由仍引用此账户池。请先更改或删除这些路由。",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne: "另一密钥已使用此值，请生成新密钥。",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "已保存的版本发生变化。请使用顶部的刷新按钮重新读取版本，然后再次提交；表单中的修改会保留。",
        chooseAProviderNameNoOtherRowHolds: "请使用未被其他上游账户占用的名称。",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "流程完成前，另一上游账户已占用此名称。点击重新开始以结束流程并解锁名称，再使用其他名称创建。",
        chooseAnIdentifierNoOtherRowHolds: "请使用尚未被占用的标识。",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "若账户授权已过期，请在上游账户编辑页重新连接；若上游尚未提供模型列表，可从此页面手动添加模型。",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "若上游基础 URL 或 API 密钥不正确，请在账户编辑页修正；若上游尚未提供模型列表，可从此页面手动添加模型。",
        checkTheProvidersStateAndCredentials: "请检查上游账户状态和凭据。",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "上游拒绝了凭据交换，请从此页面重新开始流程。",
        checkTheProvidersCredentialsAndReachability: "请检查上游账户凭据和网络连接。",
        nothingAnsweredAtBaseUrl: "[[baseUrl]] 未响应。",
        theBrowserCouldNotCompleteTheConnection: "浏览器无法完成连接。",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "请检查地址，确认网关正在运行，且其 listen 地址可从此浏览器访问。",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl: "HTTPS 页面无法访问 HTTP 地址 [[baseUrl]]。",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "此页面使用 HTTPS，浏览器阻止了对 HTTP 地址的访问，因此请求未到达网关。",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "请也通过 HTTP 提供此应用，或在网关配置中添加 tls 部分，使网关通过 HTTPS 响应。",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "网关已响应，但浏览器阻止了来自 [[baseUrl]] 的响应。",
        theBrowserWithheldTheResponseFromThisPage: "浏览器未向此页面提供响应。",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "请将 [[origin]] 加入实例配置中的 [[setting]] 并重启网关。从网关自身的源提供此应用可避免跨源允许列表。",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "请检查地址，确认网关正在运行，且浏览器可通过此协议访问网关。",
        noAnswerFromBaseUrl: "未收到 [[baseUrl]] 的响应。",
        theBrowserReceivedNoAnswerItCouldRead: "浏览器未收到可读取的响应。",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "该地址可能没有服务在监听，或浏览器在页面读取前阻止了响应。请将 [[origin]] 加入 [[setting]] 并重启网关。",
        theGatewayRejectedThatManagementToken: "网关拒绝了此管理令牌。",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "令牌必须与实例配置中的 management_api.token 一致。网关会实时读取，更改令牌后无需重启。",
        theGatewayHoldsNoSuchRow: "网关中不存在此条目。",
        theGatewayRefusedAnIdentifierIsAlreadyTaken: "网关拒绝了请求：标识已被占用。",
        theUpstreamProviderRefusedTheCallOrWasUnreachable: "上游拒绝了请求，或无法连接。",
        theGatewayFailedInternally: "网关发生内部错误。",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "原因记录在网关的 stderr 日志中，内部错误详情不会返回此页面。",
        theGatewayRejectedTheRequest: "网关拒绝了请求。",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow: "网关返回的数据结构与此版本不一致。",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "此版本生成的类型绑定与网关管理 API 不一致。请重新生成绑定，或从匹配的版本构建前端。",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "网关返回 HTTP {{httpStatus}}，但未提供管理 API 错误信息。",
        checkTheGatewaysStderrLogForTheCause: "请查看网关 stderr 日志了解原因。",
        theCallFailed: "请求失败。",
    },
    notices: {
        savedName: "已保存 [[name]]。",
        credentialsReplaced: "凭据已替换。",
        expiryReplaced: "上游账户列表已更新为新的过期时间。",
        keyReplaced: "上游 API 密钥已替换。",
        previousSecretRevoked: "保存后旧密钥已失效，请更新所有使用旧密钥的客户端。",
        lastKeyDisabled: "这是最后一个已启用的密钥。启用密钥前，网关无法提供推理服务。",
        modelSaved: "模型已保存。",
        poolSaved: "账户池已保存。",
        routeSaved: "公共模型路由已保存。",
        discovered_other: "已从 [[providerName]] 发现 [[quantity]] 个模型。",
        nothingDiscovered: "未从 [[providerName]] 发现新模型。",
        manualUntouched_other: "保留了 {{quantity}} 个手动添加的模型。",
        vanished_other: "{{quantity}} 个模型已标记为不再报告。",
    },
    connectionValidation: {
        empty: "请输入 JanusGate 基础 URL。",
        notAbsolute: "请输入完整 URL，例如 https://gateway.example.com。",
        scheme: "请使用 http:// 或 https:// URL。",
        credentials: "请移除 URL 中的凭据。",
        notOrigin: "仅填写网关的源地址，不要包含路径、查询参数或片段。",
        managementToken: "请输入管理令牌。",
        nameEmpty: "名称不能为空。",
    },
    additional: {
        app: {
            readThisScreensStateFromTheGatewayAgain: "从网关重新读取此页面的状态",
            thisScreenReadsNothingFromTheGatewaySo: "此页面无需读取网关状态，没有可刷新的内容",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "断开连接并清除保存的令牌？重新连接时需要再次输入令牌。",
        },
        apiKeyAccess: {
            allow: "允许",
            deny: "拒绝",
            provider: "上游账户",
            pool: "账户池",
            automatic: "自动",
            alias: "别名",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "必填。标识使用此密钥的客户端。",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "必填。推理客户端通过此密钥在所有协议端点上进行身份验证。",
        },
        clipboard: {
            copyToClipboard: "复制到剪贴板",
            copied: "已复制",
            copy: "复制",
            theValueHere: "此处的值",
        },
        confirmDeleteCard: {
            deleting: "正在删除…",
            deleteForGood: "永久删除",
        },
        dirtyFormFooter: {
            saving: "正在保存…",
            saveChanges: "保存更改",
        },
        layout: {
            back: "返回",
        },
        modelForm: {
            followingDiscoveryCurrentlyUnknownSoListingsOmitItMessage:
                "使用发现的值，目前未知，模型列表中不显示。",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "使用发现的值。目前没有信息，由供应商定义的声明决定。",
        },
        providerForm: {
            verifiedLive: "已实测",
            unverified: "未验证",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "必填。此定义的请求所访问的上游地址。",
            upstreamAPIKey: "上游 API 密钥",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "必填。随每个请求发送给上游。网关不会返回其值。",
            showTheAPIKeyWhileTyping: "输入时显示 API 密钥",
        },
        providersTable: {
            on: "开启",
            off: "关闭",
        },
        usageFilterControls: {
            allEndpoints: "所有端点",
        },
        apiKeyDetailScreen: {
            apiKeys: "API 密钥",
            apiKey: "API 密钥",
            createAPIKey: "创建 API 密钥",
            enabled: "已启用",
            disabled: "已禁用",
        },
        apiKeyEditScreen: {
            apiKey: "API 密钥",
            editAPIKey: "编辑 API 密钥",
            showTheCurrentKey: "显示当前密钥",
        },
        apiKeyNewScreen: {
            creating: "正在创建…",
            createAPIKey: "创建 API 密钥",
        },
        apiKeysScreen: {
            on: "开启",
            off: "关闭",
        },
        bulkPublicationScreen: {
            publicIDIsRequiredMessage: "请输入公共 ID。",
            noDirectRoutes: "没有直接路由",
            modelVanishedItsNewRouteWillBeSuspended: "模型已不再报告，新路由将暂停服务",
            modelEnabled: "模型已启用",
            modelDisabled: "模型已禁用",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "输入时显示令牌",
            probingTheGateway: "正在验证网关…",
            connect: "连接",
        },
        homeScreen: {
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "请在公共模型路由中查看配额可用性。这些数值仅反映已保存的开关和模型清单。",
            noRequestsRecordedMessage: "没有请求记录。",
        },
        modelDetailScreen: {
            models: "模型",
            model: "模型",
            overriddenByAdministratorMessage: "已由管理员设置覆盖值。",
            fromTheLatestDiscoveryMessage: "来自最近一次发现。",
            unknownOmittedFromModelListingsMessage: "未知，模型列表中不显示。",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "此条目没有信息，由供应商定义的声明决定。",
            unknown: "未知",
            noAutomaticEntry: "没有自动条目",
            enabled: "已启用",
            disabled: "已禁用",
        },
        modelEditScreen: {
            model: "模型",
            editModel: "编辑模型",
        },
        modelNewScreen: {
            aModelBelongsToOneProviderMessage: "必填。模型属于一个上游账户。",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "必填。原样发送给上游，仅可包含英文字母、数字和符号。",
            carriedHereFromAModelTheUpstreamNoMessage:
                "必填。来自上游不再报告的模型，提交后将按下方设置恢复此模型。",
            creating: "正在创建…",
            createModel: "创建模型",
        },
        modelRouteEditorScreen: {
            createPublicRoute: "创建公共模型路由",
            editPublicRoute: "编辑公共模型路由",
            publicRoutes: "公共模型路由",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "由所属上游账户或账户池自动维护的模型名称。",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "客户端请求此公共 ID。更改目标不会更改此名称。",
            openPool: "打开账户池",
            openPhysicalModel: "打开物理模型",
            deleteThisPublicRoute: "删除此公共模型路由？",
            publicIDIsRequiredMessage: "请输入公共 ID。",
            publicID: "公共 ID",
            directPhysicalModel: "直接物理模型",
            pool: "账户池",
            chooseAPool: "选择账户池",
            upstreamIDIsRequiredMessage: "请输入上游 ID。",
            upstreamID: "上游 ID",
            chooseAPhysicalModel: "选择物理模型",
        },
        modelsScreen: {
            noModelsOnAnyProviderYetDiscoverOrMessage: "所有上游账户均无模型，请发现或添加模型。",
        },
        poolAvailabilityCard: {
            readingStoredAvailability: "正在读取已保存的可用性…",
            quotaNotObserved: "尚未观测配额",
            quotaReadsUnsupported: "不支持查询配额",
            administratorResetRefreshGraceInstalled: "管理员已清除限制，并设置刷新宽限期",
            accountEnabled: "账户已启用",
            updatingAvailability: "正在更新可用性…",
            allModels: "所有模型",
            resetUnknown: "重置时间未知",
            resetTimePassedRefreshRequired: "重置时间已过，需要刷新",
        },
        providerDetailScreen: {
            providers: "上游账户",
            provider: "上游账户",
            createProvider: "创建上游账户",
            enabled: "已启用",
            disabled: "已禁用",
            theOAuthCredentialsHaveExpiredMessage: "OAuth 凭据已过期。",
            noneExactUpstreamIDs: "无，使用精确上游 ID",
            discovering: "正在发现…",
            discoverModels: "发现模型",
            askingTheUpstreamWhatItReports: "正在查询上游报告的模型…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "此上游账户没有模型，请发现上游模型，或手动添加。",
        },
        providerEditScreen: {
            provider: "上游账户",
            editProvider: "编辑上游账户",
            modelPrefix: "模型前缀",
            aProviderNeedsANameMessage: "请输入上游账户名称。",
            starting: "正在开始…",
            reconnectTheAccount: "重新连接账户",
            replacing: "正在替换…",
            replaceTheCredentials: "替换凭据",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "删除将移除此账户及其账户池成员关系。此账户没有物理模型。",
        },
        providerNewScreen: {
            modelPrefix: "模型前缀",
            creating: "正在创建…",
            createProvider: "创建上游账户",
            connecting: "正在连接…",
            connectTheAccount: "连接账户",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "必填。此定义的请求所使用的上游地址，缺少地址时网关会拒绝启动授权流程。",
            starting: "正在开始…",
            startTheAuthorization: "开始授权",
        },
        providerPoolEditorScreen: {
            createPool: "创建账户池",
            editPool: "编辑账户池",
            pools: "账户池",
            deleteThisPoolItsMembershipIsRemovedMessage: "删除此账户池？成员关系将被移除。",
            nameIsRequiredMessage: "请输入名称。",
            modelPrefix: "模型前缀",
            chooseAnAccount: "选择账户",
        },
        providerQuotaCard: {
            periodUnknown: "周期未知",
            monthly: "每月",
            unknown: "未知",
            unlimited: "无限制",
            resetUnknown: "重置时间未知",
            readingQuota: "正在读取配额…",
            refreshingQuota: "正在刷新配额…",
            refreshQuota: "刷新配额",
            retryQuota: "重试配额查询",
        },
        routingListsScreen: {
            on: "开启",
            off: "关闭",
        },
        usageQueryScreen: {
            querying: "正在查询…",
            runQuery: "运行查询",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "未设置筛选或时间范围，空结果表示网关尚无用量记录。",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "没有符合筛选条件的记录。请扩大时间范围、移除筛选条件，或在上方表单中清除筛选。",
        },
        usageScreen: {
            reading: "正在读取…",
            applyFilters: "应用筛选",
            widenTheWindowDropAFilterOrReadMessage:
                "请扩大时间窗口、移除筛选条件，或在上方表单中清除筛选以读取全部记录。",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "筛选用于查找记录，不会校验值是否存在。不存在的 ID 或从未记录的状态不会匹配任何记录。",
            thatIsEveryRowThisFilterMatchesMessage: "已显示所有符合筛选条件的记录。",
            thatIsEveryRecordedRowMessage: "已显示所有记录。",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "正在重新读取记录，请等待完成后再加载更多。",
            loading: "正在加载…",
            loadMore: "加载更多",
        },
    },
    operationNotices: {
        flowFailed: "流程失败。",
        flowGone: "此授权流程已无法完成：state 已过期或已被使用。",
        freshFlow: "请重新开始授权流程并及时完成授权。",
        flowExpired: "流程已过期。",
        flowExpiryPassed: "等待结果期间，流程已超过有效期。",
        pollStopped: "结果查询持续失败，此页面已停止查询。",
        pollFailed: "上一次结果查询失败。",
        invalidDefinition: "定义 JSON 不符合网关要求。",
        fixDefinition: "请修正 JSON 后重新提交，尚未写入任何内容。",
        invalidFilter: "查询未运行：筛选值无效。",
        previousQuery: "下方仍显示上一次查询结果，有行内错误的字段即为问题所在。",
        fixFilter: "请修正该字段后重新查询。",
        discoveryPending: "正在从 [[providerName]] 发现模型…",
        discoveryQuery: "正在查询上游当前报告的模型。",
    },
    counts: {
        models_other: "{{total}} 个模型",
        filteredModels_other: "显示 {{total}} 个模型中的 {{shown}} 个",
        enabledKeys_other: "{{total}} 个密钥中有 {{enabled}} 个已启用",
        shownKeys_other: "显示 {{shown}} 个密钥",
        filteredKeys_other: "[[shownSentence]] · [[total]] 个密钥中有 [[enabled]] 个已启用",
        providerDeleted_other:
            "删除将移除此账户、账户池成员关系及其 {{total}} 个物理模型和直接路由。账户池路由会保留。",
        requests_other: "{{total}} 次请求",
        requestsPeak_other: "[[total]] 次请求 · [[day]]最多，为 [[peak]] 次",
    },
    helpers: {
        accessControl: {
            allowed: "已允许",
            denied: "已拒绝",
            modelOverride: "模型覆盖值",
            globallyDisabled: "全局已禁用",
            targetUnavailable: "目标不可用",
            targetVanished: "目标不再报告",
            globallyAvailable: "全局可用",
            noAvailablePoolMembers: "没有可用的账户池成员",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "基础 URL 必须使用 http:// 或 https://，且不能包含空格。",
        },
        presetQuotas: {
            primaryWindow: "主要窗口",
            secondaryWindow: "次要窗口",
            allowance: "配额",
            modelAllowance: "模型配额",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "使用静态密钥的任意 OpenAI Responses 协议端点。",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "使用静态密钥的任意 Chat Completions 协议端点。",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "使用静态密钥的任意 Gemini GenerateContent 协议端点。",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "使用静态密钥的任意 Gemini Interactions 协议端点。",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "使用静态密钥的任意 Anthropic Messages 协议端点。",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "使用静态密钥的 xAI Responses 端点。原生压缩使用独立的 /responses/compact 操作，不支持显式触发和自动压缩。独立压缩端点不代表支持原生触发或自动压缩。",
            claudeCodeClaudeAccount: "Claude Code（Claude 账户）",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "通过 Messages API 访问的 Claude 订阅账户。推理和模型发现均通过 Authorization: Bearer 发送 OAuth 令牌，并携带 oauth-2025-04-20 beta 请求头。",
            kimiCodeKimiAccount: "Kimi Code（Kimi 账户）",
            githubCopilotGitHubAccount: "GitHub Copilot（GitHub 账户）",
            devinDevinAccount: "Devin（Devin 账户）",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "通过 ConnectRPC 令牌交换获取静态 API 密钥和 API 服务 URL 的 Devin 账户，无需刷新、没有过期时间，模型需手动添加。",
            codexChatGPTAccount: "Codex（ChatGPT 账户）",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "通过 Codex Responses 端点访问的 ChatGPT 账户。上游强制使用流式传输，响应使用 OpenAI Responses 格式。压缩部分声明 Codex 后端支持的显式触发和自动阈值压缩。",
            antigravityGoogleAccount: "Antigravity（Google 账户）",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "通过 Antigravity Gemini 端点访问的 Google 账户，包含项目初始化和带外层封装的 GenerateContent 请求。",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "通过 OpenAI Chat Completions 协议和静态密钥访问 Zen。",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "通过 OpenAI Responses 协议和静态密钥访问 Zen。",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "通过 Anthropic Messages 协议和静态密钥访问 Zen。",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "通过 Gemini GenerateContent 协议和静态密钥访问 Zen。",
        },
        apiKeyEditScreen: {
            aKeyNeedsASecret: "请输入密钥值。",
        },
        bulkPublicationScreen: {
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "另一个选中条目已使用此公共 ID，请设置不同名称。",
        },
        homeScreen: {
            discoverModels: "发现模型",
            createAPIKey: "创建 API 密钥",
            openAndDiscover: "打开并发现模型",
            reviewModels: "检查模型",
            credentialNotFresh: "凭据未曾有效",
            credentialExpired: "凭据已过期",
            reconnect: "重新连接",
            openProvider: "打开上游账户",
        },
        modelDetailScreen: {
            noLevels: "没有推理级别",
        },
        modelNewScreen: {
            models: "模型",
            provider: "上游账户",
        },
        modelsScreen: {
            thisProviderHoldsNoModels: "此上游账户没有模型。",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "此上游账户已不在列表中，请在账户筛选中选择其他账户。",
        },
        oauthFlowView: {
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "请留在此页面直到流程完成。切换控制台页面会丢弃此流程。",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "粘贴的内容既不是授权码，也不是含授权码的 URL。请复制登录完成后的完整地址或其中的授权码。",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "流程仍在进行，此页面会持续查询结果。",
        },
        providerEditScreen: {
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "此供应商定义需要基础 URL，请恢复地址或还原修改。",
        },
        providerNewScreen: {
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "授权期间不可修改，账户将使用流程开始时提交的值创建。",
        },
        providerQuotaCard: {
            resetTimeReachedRefreshToUpdate: "已到重置时间，请刷新更新",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "网关中不存在此账户池，可能已被删除或地址有误。",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "网关中不存在此公共模型路由，可能已被删除或地址有误。",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "网关中不存在此 API 密钥，可能已被删除或地址有误。此页面无法恢复密钥。",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "网关中不存在此模型，可能已被删除或地址有误。已发现模型被删除后，下次发现时会作为新模型返回。",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "网关中不存在此上游账户，可能已被删除或地址有误。此页面无法恢复账户。",
        },
        usageLabels: {
            streamed: "流式",
            nonstreamed: "非流式",
        },
        usageQueryScreen: {
            theQueryDidNotRunAFilterValueIsInvalid: "查询未运行，筛选值无效。",
            total: "合计",
        },
        usageScreen: {
            thisRowIsNoLongerListed: "此条目已不在列表中。",
        },
        apiKeyAccess: {
            inherit: "继承",
            allow: "允许",
            deny: "拒绝",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels: "使用发现的值，目前未声明任何推理级别。",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "取代发现的推理级别，留空表示不支持推理。",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "使用发现的值，目前未知，模型列表中不显示。",
            replacesTheDiscoveredValueWithAPositiveInteger: "以正整数取代发现的值。",
            unset: "未设置",
            discoveredNothingKnown: "发现结果：暂无信息。",
        },
        modelsTable: {
            ready: "就绪",
            off: "关闭",
            suspended: "已暂停",
            heldByProvider: "上游账户已禁用",
            unknown: "未知",
            noAutomaticEntry: "没有自动条目",
            noDirectRoutes: "没有直接路由",
        },
        providersTable: {
            valid: "有效",
            expired: "已过期",
            stale: "未曾有效",
        },
        usageFilterControls: {
            allOutcomes: "所有结果",
            failuresUpstreamOrInternal: "失败（上游或内部错误）",
            any: "任意",
            streamed: "流式",
            nonstreamed: "非流式",
        },
        usageFilter: {
            allTime: "所有时间",
            last24Hours: "最近 24 小时",
            last7Days: "最近 7 天",
            last30Days: "最近 30 天",
        },
    },
    validation: {
        composition: {
            upstreamId: "上游 ID只能包含英文字母、数字和符号，不能包含空格或非 ASCII 字符。",
            publicId: "公共 ID只能包含英文字母、数字和符号，不能包含空格或非 ASCII 字符。",
            modelPrefix: "模型前缀只能包含英文字母、数字和符号，不能包含空格或非 ASCII 字符。",
            apiKey: "API 密钥只能包含英文字母、数字和符号，不能包含空格或非 ASCII 字符。",
            key: "密钥只能包含英文字母、数字和符号，不能包含空格或非 ASCII 字符。",
        },
        routing: {
            upstreamId:
                '上游 ID只能包含英文字母、数字和符号，不能包含空格、非 ASCII 字符、"/" 或 ":"。',
            publicId:
                '公共 ID只能包含英文字母、数字和符号，不能包含空格、非 ASCII 字符、"/" 或 ":"。',
            modelPrefix:
                '模型前缀只能包含英文字母、数字和符号，不能包含空格、非 ASCII 字符、"/" 或 ":"。',
            apiKey: 'API 密钥只能包含英文字母、数字和符号，不能包含空格、非 ASCII 字符、"/" 或 ":"。',
            key: '密钥只能包含英文字母、数字和符号，不能包含空格、非 ASCII 字符、"/" 或 ":"。',
        },
        positive: {
            maxInputOverride: "最大输入 token 数覆盖值必须是正整数。",
            maxOutputOverride: "最大输出 token 数覆盖值必须是正整数。",
        },
    },
    quotaMessages: {
        day_other: "{{quantity}} 天",
        hour_other: "{{quantity}} 小时",
        minute_other: "{{quantity}} 分钟",
        second_other: "{{quantity}} 秒",
        used: "已使用 {{percentage}}",
        counter: "已使用 [[used]] / [[limit]] [[unit]]",
        remaining: "剩余 [[remaining]] [[unit]]",
        dateReset: "[[date]] · 确切重置时间未知",
        atReset: "于 [[instant]] 重置 · [[distance]]",
        future: "{{duration}}后",
        observed: "观测于 [[instant]] · [[distance]]",
        inventory: {
            pool_disabled: "池已禁用",
            account_missing: "账户不存在",
            account_disabled: "账户已禁用",
            model_not_selected: "选择上游 ID 以查看模型可用性",
            model_missing: "模型不存在",
            model_disabled: "模型已禁用",
            model_vanished: "模型已消失",
            ready: "库存就绪",
        },
        failure: {
            credential_renewal: "凭据续期失败",
            timeout: "超时",
            transport: "传输失败",
            upstream_rejected: "上游拒绝了请求",
            malformed_observation: "观测数据格式错误",
        },
        quotaObserved: "配额观测于 [[instant]]",
        quotaFailed: "配额读取失败 · {{reason}}",
        deletedAccount: "已删除的账户 #{{id}}",
        priority: "优先级 {{priority}}",
        models: "模型：[[identifiers]]",
        resets: "于 [[instant]] 重置",
    },
    dynamicMessages: {
        following: "遵循发现结果：{{value}}。",
        levelsReplace: "替换已发现的等级集 — 客户端看到的是 {{value}}。",
        capReplace: "替换已发现的值 — 客户端看到的是 {{value}}。",
        mechanismFact: "{{mechanism}}：{{support}}",
        mechanismDiscovered: "已发现：{{support}}。",
        compactionReplace:
            "完整替换已发现的能力 — 未设置的机制先使用发现结果，再使用定义中的声明。",
        noExpiry: "此上游账户没有已存储的到期时间。",
        goneFlow: "此授权流程已无法完成 — 状态已过期或已被使用。",
        refusedClipboard: "此浏览器拒绝了剪贴板操作 — 请选中{{subject}}并手动复制。",
        enabledStored: "已存储 {{total}} 个中的已启用数量",
        readyListed: "已列出 {{total}} 个中的就绪数量",
        copyEndpoint: "复制 {{surface}} 端点 URL",
        noModels: "尚无模型。",
        noReadyModels: "没有就绪库存 — 所有模型均已禁用或消失。",
        disabledCredential: "{{expiry}} 上游账户已禁用 — 重新启用时请重新连接。",
        noKeysDetail: " 服务客户端需要 API 密钥进行认证 — 没有密钥时，网关没有就绪库存。",
    },
    remainingText: {
        notRefreshedYet: "尚未刷新",
        overrideMaxInputTokens: "覆盖最大输入 token 数",
        maxInputTokensOverride: "最大输入 token 数覆盖值",
        overrideMaxOutputTokens: "覆盖最大输出 token 数",
        maxOutputTokensOverride: "最大输出 token 数覆盖值",
        chooseAProvider: "选择上游账户",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "指定上游账户及上游不再报告的 ID，可使用以下设置重新认领该模型。模型会自动通过独立上游账户或所属池的前缀公开。",
        allProviders: "所有上游账户",
        replaceTheUpstreamAPIKey: "替换上游 API 密钥",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "只写：网关不会返回此值；留空将保留已存储的密钥。",
        backToPools: "返回池列表",
        publicRoute: "公共模型路由",
        publicRoutes: "公共模型路由",
        backToPublicRoutes: "返回公共模型路由列表",
        backToAPIKeys: "返回 API 密钥列表",
        backToModels: "返回模型列表",
        backToProviders: "返回上游账户列表",
        deletedRoute: "已删除的路由",
        deletedModel: "已删除的模型",
        deletedProvider: "已删除的上游账户",
        deletedKey: "已删除的密钥",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "下方仍为上次查询的结果 — 显示行内错误的字段说明了问题。",
        fromAndToDoNotApplyThere: " — 起始与结束时间不适用于该选项。",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "每个已派发的服务请求对应一行 — 从网关发往上游时开始记录。",
        showTheKeyWhileTyping: "输入时显示密钥",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "保存会用您的编辑覆盖网关版本；还原则采用网关版本。",
        theOAuthCredentialsWereNeverFresh: "OAuth 凭据从未处于有效状态。",
        allPublicRoutes: "所有公共模型路由",
        allPhysicalModels: "所有物理模型",
        allAPIKeys: "所有 API 密钥",
    },
    entityMessages: {
        editName: "编辑 [[name]]",
        editIdentifier: "编辑 [[identifier]]",
        deleteKey: "删除 [[name]]？持有此密钥的客户端将失去访问权限。",
        deleteLastKey:
            "删除 [[name]]？这是最后一个已启用的密钥 — 在启用其他密钥前，网关将无法提供服务。",
        deleteModel:
            "删除 [[identifier]]？其直接路由也将被删除。池路由会保留。重新发现或手动创建会产生新的物理模型行，不会恢复已删除的路由。",
        deleteProvider: "删除 [[name]]？",
        switchPreset: "切换到 [[name]]？您编辑的连接配方将被替换。",
        noKeysMatch: "没有匹配“[[query]]”的 API 密钥。",
        noProvidersMatch: "没有匹配“[[query]]”的上游账户。",
        noModelsMatch: "没有匹配“[[query]]”的模型。",
        noProviderModelsMatch: "此上游账户没有匹配“[[query]]”的模型。",
        missingPath: "[[path]] 没有内容。它可能已被删除，或地址输入有误。",
        aliasCreated: "已创建别名：[[identifier]]",
        addAlias: "为 [[identifier]] 添加别名",
    },
    modelCard: {
        heading: "模型 · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "您有未保存的编辑，此密钥已在网关上发生变更。",
            provider: "您有未保存的编辑，此上游账户已在网关上发生变更。",
            model: "您有未保存的编辑，此模型已在网关上发生变更。",
            pool: "您有未保存的编辑，此账户池已在网关上发生变更。",
            "public route": "您有未保存的编辑，此公共模型路由已在网关上发生变更。",
            "api key": "您有未保存的编辑，此API 密钥已在网关上发生变更。",
        },
        noRequests: {
            key: "此密钥尚无请求记录。",
            provider: "此上游账户尚无请求记录。",
            model: "此模型尚无请求记录。",
            pool: "此账户池尚无请求记录。",
            "public route": "此公共模型路由尚无请求记录。",
            "api key": "此API 密钥尚无请求记录。",
        },
        notFound: {
            api_key: "找不到 API 密钥",
            provider: "找不到上游账户",
            model: "找不到模型",
            pool: "找不到池",
            model_route: "找不到公共模型路由",
        },
        replaceSecret: {
            typed: "替换已输入的密钥？",
            current: "替换当前密钥？",
        },
        bound: {
            From: "起始时间必须为完整日期与时间，例如 2026-09-23T10:00；留空则不限制。",
            To: "结束时间必须为完整日期与时间，例如 2026-09-23T10:00；留空则不限制。",
        },
        updated: "更新于 {{instant}}",
        created: "创建于 {{instant}}",
        version: "已存储为版本 {{version}}。",
        unlisted: "{{value}}（已不在列表中）",
        accountUnlisted: "账户 #{{id}}（已不在列表中）",
        moveUp: "上移 {{name}}",
        moveDown: "下移 {{name}}",
        remove: "移除 {{name}}",
        publicIdFor: "{{subject}} 的公共 ID",
        source: {
            key: "来自密钥",
            provider: "来自上游账户",
            pool: "来自池",
            route: "模型覆盖",
        },
        deleted: {
            provider: "已删除的上游账户 #{{id}}",
            pool: "已删除的池 #{{id}}",
            route: "已删除的路由 #{{id}}",
        },
        status: "客户端 {{client}} · 上游 {{upstream}}",
        metrics: " · {{duration}} · 输入 {{input}} · 输出 {{output}}{{tail}}",
        optionalTokens: " · 缓存 {{cached}} · 缓存写入 {{write}} · 推理 {{reasoning}}",
    },
    routingMessages: {
        directRoutes_other: "{{quantity}} 条直接路由",
        poolMembers_other: "[[quantity]] 个账户成员 · 模型前缀：[[prefix]]",
        directMissing: "直接 · 物理模型 #[[id]]",
        direct: "直接 · [[provider]] · [[model]]",
        pool: "池 · [[pool]] · [[model]]",
        publicIds: "公共 ID [[identifiers]]",
        modelAccess: "[[identifier]] 的模型访问权限",
        ownerDefault: {
            provider: "[[name]] 的账户默认权限",
            pool: "[[name]] 的账户池默认权限",
        },
        directTarget: "直接 · 模型 #[[id]]",
        poolTarget: "池 · [[identifier]]",
        reconnect: "请在[[editLink]]重新连接 — 凭据卡片会替换凭据。",
        reconnectDisabled:
            "上游账户已禁用 — 重新启用时请重新连接；[[editLink]]上的凭据卡片会替换凭据。",
    },
    linkedMessages: {
        keyEndpoint:
            "请将客户端指向一个端点 — 每个协议接口都提供获准访问的公共条目；URL 位于[[overviewLink]]。",
        servingKey:
            "服务客户端使用 API 密钥认证 — 请先在[[keysLink]]中创建密钥，再连接此处的端点。",
        vanished:
            "此期间无法提供推理服务。上游重新列出模型时，请[[discoverLink]]，或[[claimLink]]。",
        discoverLabel: "在账户页面重新发现",
        claimLabel: "在创建模型页面重新认领",
        selected: "已选择 {{quantity}} 项。已成功的行不会再次提交。",
        kimiDescription:
            "通过 Coding 端点连接 Kimi 账户：设备授权流程在授权、轮询和续期时，均通过自定义 X-Msh-* 标头传递设备 ID。",
        githubDescription:
            "连接 GitHub 账户：重新配置时，用长期令牌换取短期 Copilot 会话令牌和账户的 API 端点。",
        chartPoint_other: "[[day]]：[[quantity]] 次请求",
        inventoryEmpty: "[[name]] 尚无模型。",
        inventoryUnready: "[[name]] 没有就绪库存 — 所有模型均已禁用或消失。",
    },
    vocabulary: {
        credentialKind: {
            api_key: "静态 API 密钥",
            oauth: "OAuth 账户",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "独立压缩",
            responses_explicit_trigger: "显式触发",
            responses_automatic_compaction: "自动压缩",
            messages_on_demand_compaction: "按需压缩",
            messages_threshold_compaction: "阈值压缩",
        },
        mechanismSupport: {
            supported: "支持",
            unsupported: "不支持",
            unknown: "未知",
        },
        discoveryState: {
            discovered: "已发现",
            vanished: "不再报告",
            manual: "手动添加",
        },
        reasoningLevel: {
            none: "无",
            minimal: "最低",
            low: "低",
            medium: "中",
            high: "高",
            xhigh: "极高",
            max: "最高",
        },
        apiLogOutcome: {
            completed: "已完成",
            client_aborted: "客户端已中止",
            upstream_error: "上游错误",
            internal: "内部错误",
        },
        apiLogEndpointType: {
            openai_chat_completions: "Chat Completions",
            openai_responses: "Responses",
            openai_responses_compact: "Responses 压缩",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interactions: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        usageGroupBy: {
            day: "日期",
            model: "物理模型",
            model_route: "公共模型路由",
            provider: "上游账户",
            api_key: "API 密钥",
            endpoint_type: "端点",
            outcome: "结果",
            is_stream: "流式模式",
            http_status: "HTTP 状态",
        },
    },
    common: {
        save: "保存更改",
        cancel: "取消",
        retry: "重试",
        language: "语言",
    },
    language: {
        system: "跟随浏览器",
        loading: "正在加载{{languageName}}…",
        failed: "无法加载{{languageName}}，仍显示当前语言。",
        startupFailed: "无法加载{{languageName}}，当前显示英语。",
    },
    models: {
        discoveredCount_other: "已发现 {{count}} 个模型。",
    },
} as const;
export default chinese;
