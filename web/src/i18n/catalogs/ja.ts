/** Japanese administration messages. */
const japanese = {
    shell: {
        skipToContent: "コンテンツへスキップ",
        primary: "メイン",
        refresh: "更新",
        disconnect: "切断",
        keep: "保持",
    },
    connection: {
        connectToJanusGate: "JanusGate に接続",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "このアプリは JanusGate ゲートウェイを管理します。対象は上流アカウント、そのモデル、および認証情報です。",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "このブラウザーから到達できる、稼働中の JanusGate インスタンス。",
        theOriginBesideIt: "その横にあるオリジン",
        copyThisAppsOrigin: "このアプリのオリジンをコピー",
        gatewayAddress: "ゲートウェイのアドレス",
        schemeHostAndPortOnly: "スキーム、ホスト、ポートのみを指定します。",
        managementToken: "管理トークン",
        sentOnlyToThatInstanceAndKeptOnly:
            "そのインスタンスにのみ送信され、保存先もこのブラウザーだけです。",
        cancel: "キャンセル",
    },
    controls: {
        modelAccess: "モデルアクセス",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "既定値は、それを継承している既存のエントリに影響します。プールアクセスは、今後追加される対象メンバーにも及びます。グローバルな可用性は権限とは別に扱われます。",
        defaultModelAccessLabel: "既定のモデルアクセス：",
        defaultModelAccess: "既定のモデルアクセス",
        appliesToEntriesWithoutAProviderPoolOr:
            "プロバイダー、アカウントプール、モデルのいずれの上書きも持たないエントリに適用されます。今後追加されるエントリも対象です。",
        searchModelAccess: "モデルアクセスを検索",
        publicIDOrOwnerName: "公開 ID または所有者名",
        overridesOnly: "上書きのみ",
        removeOverride: "上書きを削除",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "保存する前に削除済みの参照を外すか、更新してインベントリの不一致を解消してください。",
        thisDefaultAppliesToFutureEntries: "この既定値は、今後追加されるエントリに適用されます。",
        noEntriesMatchThisViewHiddenOverridesAre:
            "この表示に一致するエントリはありません。非表示の上書きは保持されます。",
        name: "名前",
        key: "キー",
        skOrGenerateOne: "sk-… または生成",
        generate: "生成",
        replaceIt: "置き換え",
        keepIt: "そのまま保持",
        theSecretHere: "このシークレット",
        delete: "削除…",
        revert: "元に戻す",
        unsavedChanges: "未保存の変更があります",
        overrideReasoningLevels: "推論レベルを上書き",
        reasoningLevels: "推論レベル",
        pickEveryLevelThisModelDeclaresUntickAll:
            "このモデルが宣言するすべてのレベルを選択します。すべて外すと何も宣言しません。",
        aPositiveIntegerClearTheSwitchToFollow:
            "正の整数を指定します。スイッチを外すと、再び検出値に従います。",
        overrideCompactionFacts: "圧縮の情報を上書き",
        compactionMechanisms: "圧縮メカニズム",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            "このモデルが情報を置き換えるメカニズムをすべて指定します。「不明」は「情報なし」を明確に示し、そのメカニズムで検出済みの情報を打ち消します。",
        addAlias: "別名を追加",
        modelPrefix: "モデルプレフィックス",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "アカウントプールに属さない場合、モデルは「このプレフィックス + 上流 ID」で呼び出せます。メンバーはアカウントプールのプレフィックスを使います。空欄にすると上流 ID をそのまま使います。プレフィックスを変更すると、既存の自動モデル名も変わります。",
        upstreamBaseURL: "上流ベース URL",
        enabled: "有効",
        publicRoute: "公開ルート",
        provider: "プロバイダー",
        physicalModel: "物理モデル",
        apiKey: "API キー",
        endpoint: "エンドポイント",
        outcome: "結果",
        streamMode: "ストリームモード",
        httpStatuses: "HTTP ステータス",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "カンマ区切り（例：200, 429）。応答がなかったリクエストはどのステータスにも一致しません。",
    },
    navigation: {
        overview: "概要",
        providers: "プロバイダー",
        inventory: "インベントリ",
        pools: "アカウントプール",
        routes: "公開ルート",
        apiKeys: "API キー",
        usage: "使用量",
        lightTheme: "ライトテーマ — ダークに切り替え",
        darkTheme: "ダークテーマ — システムに合わせる",
        systemTheme: "システムテーマ — ライトに切り替え",
    },
    rich: {
        connected: "[[baseUrl]] に接続しました — トークンはこのブラウザーにのみ保存されます。",
        setupToken: "そのインスタンスの設定ファイルにある [[setting]] が管理トークンです。",
        setupCors:
            "そのインスタンスの [[setting]] でこのアプリのオリジンを許可してください — ただし、このアプリがゲートウェイ自身から配信されている場合は不要です。",
    },
    titles: {
        bulkPublication: "インベントリを公開",
        providerPools: "アカウントプール",
        providerPoolNew: "アカウントプールを作成",
        providerPoolEdit: "アカウントプール #{{id}} を編集",
        modelRoutes: "公開ルート",
        modelRouteNew: "公開ルートを作成",
        modelRouteEdit: "公開ルート #{{id}} を編集",
        home: "概要",
        providers: "プロバイダー",
        providerNew: "プロバイダーを作成",
        providerDetail: "プロバイダー #{{id}}",
        providerEdit: "プロバイダー #{{id}} を編集",
        models: "モデル",
        modelNew: "モデルを作成",
        modelDetail: "モデル #{{id}}",
        modelEdit: "モデル #{{id}} を編集",
        apiKeys: "API キー",
        apiKeyNew: "API キーを作成",
        apiKeyDetail: "API キー #{{id}}",
        apiKeyEdit: "API キー #{{id}} を編集",
        usage: "使用量",
        usageQuery: "使用量の集計",
        notFound: "ページが見つかりません",
    },
    format: {
        lessThanMinute: "1 分未満",
        minutes: "{{minutes}} 分",
        hours: "{{hours}} 時間",
        hoursMinutes: "{{hours}} 時間 {{minutes}} 分",
        days: "{{days}} 日",
        daysHours: "{{days}} 日 {{hours}} 時間",
        expired: "{{distance}}前に失効 — {{instant}}。",
        valid: "あと {{distance}} 有効 — {{instant}} まで。",
        durationSeconds: "{{seconds}} 秒",
        durationMinutesSeconds: "{{minutes}} 分 {{seconds}} 秒",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "API キー",
            editAPIKey: "API キーを編集",
            secret: "シークレット",
            overview: "概要",
            details: "詳細",
            name: "名前",
            created: "作成日時",
            status: "ステータス",
            apiKeys: "API キー",
            createAPIKey: "API キーを作成",
            enabled: "有効",
            disabled: "無効",
        },
        apiKeyEditScreen: {
            editAPIKey: "API キーを編集",
            settings: "設定",
            rotatingMeansGenerateThenSaveMessage:
                "ローテーションとは、生成してから保存することです。",
            enabled: "有効",
            replacingTheSecretRevokesTheOldValueOnMessage:
                "シークレットを置き換えると、保存時に以前の値は失効します。",
            aKeyNeedsASecret: "キーにはシークレットが必要です。",
            deleteThisKey: "このキーを削除",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "削除するとキーは即座に失効します。このキーを保持するクライアントは 401 を受け取るようになります。モデルアクセスのルールも削除されますが、過去の集計は残ります。",
            apiKey: "API キー",
            showTheCurrentKey: "現在のキーを表示",
        },
        apiKeyNewScreen: {
            createAPIKey: "API キーを作成",
            oneSecretYourServingClientsShareMessage:
                "推論クライアントが共有する 1 つのシークレットです。",
            apiKeysBackLabel: "API キー",
            key: "キー",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "シークレットに使えるのは英数字と記号のみで、空白は含められません。ほかのどのキーとも重複しない値にしてください。",
            enabled: "有効",
            loadModelAccessInventoryBeforeCreatingAKeyMessage:
                "キーを作成する前に、モデルアクセスのインベントリを読み込んでください。",
            creating: "作成しています…",
        },
        apiKeysScreen: {
            apiKeys: "API キー",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "推論用のキーです。OpenAI クライアントは Authorization: Bearer <key> として、Gemini は x-goog-api-key、Anthropic は x-api-key として送信します。キーを開くと、シークレットと使用量を確認できます。",
            createAPIKey: "API キーを作成",
            noAPIKeysYet: "API キーはまだありません",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "新規のゲートウェイは、最初のキーができるまで何も提供できません。",
            searchAPIKeys: "API キーを検索",
            searchByNameOrKey: "名前またはキーで検索",
            theSecretHere: "このシークレット",
            on: "オン",
            off: "オフ",
        },
        bulkPublicationScreen: {
            createAliases: "別名を作成",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "物理モデルを選び、その公開 ID を設定します。選択したモデルごとに有効な直接ルートが 1 つ作成されます。名前の競合は報告され、自動で改名や上書きは行われません。",
            inventoryBackLabel: "インベントリ",
            noPhysicalModelsToAlias: "別名を作成できる物理モデルがありません",
            discoverOrAddInventoryFirstMessage:
                "先にモデルを検出するか、インベントリを追加してください。",
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "別の選択行がこの公開 ID を使用しています。重複しない明示的な名前を選んでください。",
            openRoute: "ルートを開く",
            createSelectedAliases: "選択した別名を作成",
            publicIDIsRequiredMessage: "公開 ID は必須です。",
            noDirectRoutes: "直接ルートなし",
            modelVanishedItsNewRouteWillBeSuspended:
                "モデルが報告されなくなりました。新しいルートは一時停止されます",
            modelEnabled: "モデルは有効です",
            modelDisabled: "モデルは無効です",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "入力中はトークンを表示",
            probingTheGateway: "ゲートウェイを確認しています…",
            connect: "接続",
        },
        homeScreen: {
            overview: "概要",
            theHealthOfThisGatewayAtAGlanceMessage:
                "このゲートウェイの状態をひと目で確認できます。",
            createProvider: "プロバイダーを作成",
            providers: "プロバイダー",
            publicRoutes: "公開ルート",
            inventoryReady: "インベントリ準備完了",
            apiKeys: "API キー",
            recentRequests24h: "最近のリクエスト（24 時間）",
            recentErrors24h: "最近のエラー（24 時間）",
            requestsPerDay: "日別リクエスト数",
            theLast14DaysInUTCTheGatewaysMessage:
                "直近 14 日間を UTC、すなわちゲートウェイ自身の日付境界で表示します。",
            openUsage: "使用量を開く",
            storedSettingsReviewedMessage: "保存済みの設定を確認しました。",
            needsAttention: "対応が必要",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "利用できるモデルがありません。有効なプロバイダーでモデルを検出してください。",
            noAPIKeysYetMessage: "API キーはまだありません。",
            endpoints: "エンドポイント",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "すべてのインターフェースがすべてのモデルを提供します。OpenAI、Gemini、Anthropic のいずれかの形式のクライアントをこのインスタンスに向けてください。",
            theURLBesideIt: "その横にある URL",
            noProvidersYet: "プロバイダーはまだありません",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "プロバイダーは 1 つの上流アカウントです。モデルは検出した時点で取り込まれます。",
            createYourFirstProvider: "最初のプロバイダーを作成",
            connectedToTheGateway: "ゲートウェイに接続済み",
            discoverModels: "モデルを検出",
            createAPIKey: "API キーを作成",
            openAndDiscover: "開いて検出",
            reviewModels: "モデルを確認",
            credentialNotFresh: "認証情報が有効になったことがありません",
            credentialExpired: "認証情報が失効しています",
            reconnect: "再接続",
            openProvider: "プロバイダーを開く",
            createAProvider: "プロバイダーを作成",
            discoverItsModelsFromUpstream: "上流からモデルを検出",
            issueAnAPIKeyForServingClients: "推論クライアント用の API キーを発行",
            pointAClientAtAnEndpointBelow: "クライアントを以下のエンドポイントに向ける",
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "割り当ての可用性は公開ルートで確認してください。これらの数値は、保存済みのスイッチとインベントリを表すものです。",
            noRequestsRecordedMessage: "リクエストは記録されていません。",
        },
        modelDetailScreen: {
            model: "モデル",
            upstreamOnProvider: "{{provider}} の {{upstreamId}}。",
            modelsBackLabel: "モデル",
            capabilities: "機能",
            whatTheModelListingsDeclareForThisModelMessage:
                "モデル一覧がこのモデルについて宣言している内容です。ここでの設定がリクエストを制限することはなく、応答は上流が返すとおりになります。",
            reasoningLevels: "推論レベル",
            maxInputTokens: "最大入力トークン数",
            maxOutputTokens: "最大出力トークン数",
            compaction: "圧縮",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "この行自身の圧縮情報です。上書きがあればそれを使い、なければ直近の検出結果を使います。どちらにもない部分は、定義の宣言が決めます。",
            noLevels: "レベルなし",
            addAlias: "別名を追加",
            editModel: "モデルを編集",
            details: "詳細",
            publicModelNames: "公開モデル名",
            directRoutes: "直接ルート",
            upstreamID: "上流 ID",
            provider: "プロバイダー",
            discovery: "検出",
            status: "ステータス",
            models: "モデル",
            overriddenByAdministratorMessage: "管理者が上書きしています。",
            fromTheLatestDiscoveryMessage: "直近の検出結果です。",
            unknownOmittedFromModelListingsMessage: "不明 — モデル一覧では省略されます。",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "この行には情報がありません。定義の宣言が決めます。",
            unknown: "不明",
            noAutomaticEntry: "自動エントリなし",
            enabled: "有効",
            disabled: "無効",
        },
        modelEditScreen: {
            editModel: "モデルを編集",
            modelSettings: "モデル設定",
            enabled: "有効",
            deleteThisModel: "このモデルを削除",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "削除すると、この物理モデルとその直接ルートが削除されます。プールのルートは保存されたままです。",
            model: "モデル",
        },
        modelNewScreen: {
            createModel: "モデルを作成",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "検出では得られないモデル、または上流が報告しなくなったモデルを登録して、取り戻せます。",
            noProvidersYet: "プロバイダーはまだありません",
            aModelBelongsToAProviderAndThereMessage:
                "モデルはプロバイダーに属しますが、それを受け入れるプロバイダーがありません。先にプロバイダーを追加してください。検出によって、このモデルがすでに得られている場合もあります。",
            createProvider: "プロバイダーを作成",
            aModelAddedByHandIsNeverRemovedMessage:
                "手動で追加したモデルが自動で削除されることはありません。検出で失われたモデルも取り戻せます。",
            models: "モデル",
            identity: "識別情報",
            provider: "プロバイダー",
            upstreamID: "上流 ID",
            modelSettings: "モデル設定",
            enabled: "有効",
            aModelBelongsToOneProviderMessage: "必須。モデルは 1 つのプロバイダーに属します。",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "必須。上流へそのまま送信されます。英数字と記号のみを使用します。",
            carriedHereFromAModelTheUpstreamNoMessage:
                "必須。上流が報告しなくなったモデルから引き継いだものです。送信すると、以下の設定でそのモデルを取り戻します。",
            creating: "作成しています…",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "このモデル名は自動で管理されています。変更するには、所有するプロバイダーまたはアカウントプールのプレフィックスを編集してください。",
            deleteThisPublicRoute: "この公開ルートを削除",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "この公開 ID を要求するクライアントには model-not-found が返ります。物理インベントリとアカウントプールは保存されたままです。",
            publicRouteSettings: "公開ルート設定",
            enabled: "有効",
            publicID: "公開 ID",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "クライアントが要求する正確なモデル名です。名前を変更する場合は、クライアント側の更新も必要です。",
            targetType: "ターゲット種別",
            pool: "アカウントプール",
            upstreamID: "上流 ID",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "各メンバーに対して要求する正確な識別子です。モデルを持たないメンバーがあっても、公開は妨げられません。",
            physicalModel: "物理モデル",
            createPublicRoute: "公開ルートを作成",
            editPublicRoute: "公開ルートを編集",
            publicRoutes: "公開ルート",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "プロバイダーまたはアカウントプールによって自動的に維持されるモデル名です。",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "クライアントはこの公開 ID を要求します。ターゲットを変更しても、この名前はそのままです。",
            openPool: "アカウントプールを開く",
            openPhysicalModel: "物理モデルを開く",
            deleteThisPublicRouteQuestion: "この公開ルートを削除しますか？",
            publicIDIsRequiredMessage: "公開 ID は必須です。",
            directPhysicalModel: "物理モデルを直接指定",
            chooseAPool: "アカウントプールを選択",
            upstreamIDIsRequiredMessage: "上流 ID は必須です。",
            chooseAPhysicalModel: "物理モデルを選択",
        },
        modelsScreen: {
            models: "モデル",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "モデルは、単独プロバイダーのプレフィックスまたは所有するアカウントプールのプレフィックスを通じて自動的に公開されます。",
            createAliases: "別名を作成",
            createModel: "モデルを作成",
            noModelsYet: "モデルはまだありません",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "先にプロバイダーを追加してから、そのモデルを検出してください。手動で追加することもできます。",
            thisProviderHoldsNoModels: "このプロバイダーにはモデルがありません。",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "このプロバイダーは一覧にありません。プロバイダーの絞り込みで別のものを選んでください。",
            createProvider: "プロバイダーを作成",
            searchModels: "モデルを検索",
            searchByUpstreamIDOrProviderName: "上流 ID またはプロバイダー名で検索",
            provider: "プロバイダー",
            noModelsOnAnyProviderYetDiscoverOrMessage:
                "どのプロバイダーにもモデルがありません。検出するか、手動で追加してください。",
        },
        notFoundScreen: {
            pageNotFound: "ページが見つかりません",
            goToOverview: "概要へ移動",
            providers: "プロバイダー",
            models: "モデル",
        },
        providerDetailScreen: {
            provider: "プロバイダー",
            editProvider: "プロバイダーを編集",
            editPage: "編集ページ",
            storedAvailability: "保存済みの可用性",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "このアカウントで観測されたすべての制限です。リセットは制限を解除して更新の猶予を設定しますが、上流の割り当てはリセットしません。",
            details: "詳細",
            modelPrefix: "モデルプレフィックス",
            type: "種別",
            definition: "定義",
            upstreamBaseURL: "上流ベース URL",
            credentials: "認証情報",
            status: "ステータス",
            discoveredFromUpstreamOrAddedByHandMessage:
                "上流から検出したモデル、または手動で追加したモデルです。",
            createModel: "モデルを作成",
            allModels: "すべてのモデル",
            providers: "プロバイダー",
            createProvider: "プロバイダーを作成",
            enabled: "有効",
            disabled: "無効",
            theOAuthCredentialsHaveExpiredMessage: "OAuth の認証情報が失効しています。",
            noneExactUpstreamIDs: "なし — 上流 ID をそのまま使用",
            discovering: "検出しています…",
            discoverModels: "モデルを検出",
            askingTheUpstreamWhatItReports: "上流が報告する内容を問い合わせています…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "このプロバイダーにはまだモデルがありません。プロバイダーが報告する内容を検出するか、手動で追加してください。",
        },
        providerEditScreen: {
            editProvider: "プロバイダーを編集",
            settings: "設定",
            credentials: "認証情報",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "OAuth の認証情報は失効します。再接続すると置き換えられます。",
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "このプロバイダーの定義は、リクエストをベース URL に対して評価します。復元するか、「元に戻す」を選んでください。",
            deleteThisProvider: "このプロバイダーを削除",
            provider: "プロバイダー",
            modelPrefix: "モデルプレフィックス",
            aProviderNeedsANameMessage: "プロバイダーには名前が必要です。",
            starting: "開始しています…",
            reconnectTheAccount: "アカウントを再接続",
            replacing: "置き換えています…",
            replaceTheCredentials: "認証情報を置き換え",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "削除すると、このアカウントとそのアカウントプールのメンバーシップが削除されます。物理モデルはありません。",
        },
        providerNewScreen: {
            createProvider: "プロバイダーを作成",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "1 つの上流アカウントです。プリセットを選ぶと、それに沿って認証情報の経路が決まります。",
            providersBackLabel: "プロバイダー",
            readAgain: "もう一度読む",
            preset: "プリセット",
            switchPreset: "プリセットを切り替え",
            keepMyEdit: "編集内容を保持",
            identity: "識別情報",
            connection: "接続",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage:
                "キーは書き込み専用です。ゲートウェイが返すことはありません。",
            connectTheAccount: "アカウントを接続",
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "認可が有効な間はロックされます。プロバイダーは、フロー開始時に送信された内容から作成されます。",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "フローが示す場所でアクセスを承認し、そのまま完了させてください。このアプリが認証情報を保存することはありません。",
            upstreamBaseURL: "上流ベース URL",
            definition: "定義",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "ゲートウェイが保存する接続レシピです。保存済み定義の JSON を編集すると次のバージョンが登録され、保存済みのものが書き換えられることはありません。",
            notStoredYetSubmittingCreatesVersion1Message:
                "まだ保存されていません。送信するとバージョン 1 が作成されます。",
            advancedTheConnectionRecipePrefilledByThePreset:
                "詳細設定：接続レシピ（プリセットで入力済み）",
            definitionJSON: "定義 JSON",
            resetToPreset: "プリセットに戻す",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "接続レシピが無効です。問題のある箇所は、上記の詳細設定を開いて確認してください。",
            modelPrefix: "モデルプレフィックス",
            creating: "作成しています…",
            connecting: "接続しています…",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "必須。この定義のリクエストが評価の対象とする上流アドレスです。これがないと、ゲートウェイはフローの開始を拒否します。",
            starting: "開始しています…",
            startTheAuthorization: "認可を開始",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "同じ上流サービスを担う、互換可能なアカウントを優先順に試します。",
            inspectUpstreamID: "上流 ID を検査",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "保存済みのメンバーシップを検査します。空欄のままにすると、アカウント全体の制限のみを表示します。",
            deleteThisPool: "このアカウントプールを削除",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "削除すると、アカウントプールの自動エントリが削除され、メンバーの単独プレフィックスが復元されます。手動で作成したルートは、先に削除するかターゲットを変更してください。",
            poolSettings: "アカウントプール設定",
            enabled: "有効",
            name: "名前",
            modelPrefix: "モデルプレフィックス",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "アカウントプールは、メンバーのモデルを「このプレフィックス + 上流 ID」として自動的に公開します。空欄にすると上流 ID をそのまま使います。プレフィックスを変更すると、既存の自動エントリの名前が変わります。",
            accountsInPriorityOrder: "優先順に並んだアカウント",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "メンバーは同じ上流サービスを提供する必要があります。メンバー自身のプレフィックスのエントリは、プール所属中は非表示になります。新しいモデルは自動的にここに現れ、そのモデルを持つメンバーにのみルーティングされます。空のアカウントプールも許可されます。",
            moveUp: "上へ移動",
            moveDown: "下へ移動",
            remove: "削除",
            addAccount: "アカウントを追加",
            addMember: "メンバーを追加",
            createPool: "アカウントプールを作成",
            editPool: "アカウントプールを編集",
            pools: "アカウントプール",
            deleteThisPoolItsMembershipIsRemovedMessage:
                "このアカウントプールを削除しますか？メンバーシップが削除されます。",
            nameIsRequiredMessage: "名前は必須です。",
            chooseAnAccount: "アカウントを選択",
        },
        providerQuotaCard: {
            upstreamQuota: "上流の割り当て",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "割り当ては上流アカウントに属し、ほかのクライアントによる使用量を含む場合があります。",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage:
                "このプロバイダー定義は、割り当ての参照を宣言していません。",
            checkingWhetherThisDefinitionSupportsQuotaReads:
                "この定義が割り当ての読み取りに対応しているか確認しています…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "割り当ては未観測です。更新すると、上流アカウントに明示的に照会します。",
            readingUpstreamQuota: "上流の割り当てを読み取っています…",
            showingThePreviousReadingMessage: "前回の読み取り結果を表示しています。",
            observed: "観測日時",
            resetTimeReachedRefreshToUpdate: "リセット時刻に達しました。更新して反映してください",
            theUpstreamReturnedNoQuotaEntriesMessage: "上流は割り当ての項目を返しませんでした。",
            periodUnknown: "期間不明",
            monthly: "毎月",
            unknown: "不明",
            unlimited: "無制限",
            resetUnknown: "リセット時刻は不明",
            readingQuota: "割り当てを読み取っています…",
            refreshingQuota: "割り当てを更新しています…",
            refreshQuota: "割り当てを更新",
            retryQuota: "割り当てを再取得",
        },
        providersScreen: {
            providers: "プロバイダー",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "上流アカウントです。開くとモデルと使用量を確認でき、編集はさらに 1 段先にあります。",
            createProvider: "プロバイダーを作成",
            noProvidersYet: "プロバイダーはまだありません",
            aProviderIsOneUpstreamAccountMessage: "プロバイダーは 1 つの上流アカウントです。",
            searchProviders: "プロバイダーを検索",
            searchByNameTypeOrFormat: "名前、種別、形式で検索",
        },
        readNotice: {
            readAgain: "もう一度読む",
        },
        routingListsScreen: {
            pools: "アカウントプール",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "1 つの上流サービスを担う互換可能なアカウントからなる、名前付きのモデルカタログです。",
            createPool: "アカウントプールを作成",
            noPoolsYet: "アカウントプールはまだありません",
            createANamedPoolWithAModelPrefixMessage:
                "名前付きのアカウントプールを、モデルプレフィックス付きで作成します。メンバーのモデルは自動的に公開されます。",
            noModelPrefix: "なし",
            publicRoutes: "公開ルート",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "クライアントが要求するモデル名です。単独プロバイダーとアカウントプールはモデル名を自動的に公開します。追加の別名は任意です。",
            createPublicRoute: "公開ルートを作成",
            noPublicRoutesYet: "公開ルートはまだありません",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "物理モデルを直接公開するか、アカウントプールと正確な上流 ID を指定します。",
            automaticPrefix: "自動 · ",
            on: "オン",
            off: "オフ",
        },
        screenFrame: {
            readAgain: "もう一度読む",
        },
        usageQueryScreen: {
            usageAggregates: "使用量の集計",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "絞り込みの範囲内で、次元の組み合わせごとに 1 つの集計を作ります。含まれるのは件数とトークンの合計のみで、内容は含みません。",
            viewRequests: "リクエストを表示",
            question: "集計条件",
            groupBy: "グループ化",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "任意の次元を指定できます。何も選ばない場合は、一致するすべての行を合計します。",
            window: "期間",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "以下の範囲を入力します。開始は 1 期間前、終了は未指定のままです。計算は現在時刻で行い、各項目は引き続き手動で編集できます。",
            from: "開始",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "この値を含みます。このブラウザーのタイムゾーンで指定し、例：2026-09-23T10:00。空欄なら下限を設けません。",
            to: "終了",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "この値を含みません。このブラウザーのタイムゾーンで指定し、例：2026-09-23T10:00。空欄なら上限を設けません。",
            clearFilters: "絞り込みを解除",
            usageBackLabel: "使用量",
            theQueryDidNotRunAFilterValueIsInvalid:
                "クエリは実行されませんでした。絞り込みの値が無効です。",
            total: "合計",
            nothingRecordedInThisWindowMessage: "この期間には記録がありません。",
            noRowsInThisWindowMessage: "この期間には行がありません。",
            bucket: "集計単位",
            requests: "リクエスト",
            input: "入力",
            output: "出力",
            cached: "キャッシュ済み",
            cacheWrite: "キャッシュ書き込み",
            reasoning: "推論",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "トークンの合計は、上流が使用量を報告しなかった行を除外します。内訳がないからといって、合計が増えたり減ったりすることはありません。",
            querying: "クエリを実行しています…",
            runQuery: "クエリを実行",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "絞り込みも範囲も指定されていないため、空の結果はゲートウェイの全記録を意味します。まだ使用量がありません。",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "これらの絞り込みに一致する記録はありません。範囲を広げるか、絞り込みを外すか、絞り込みなしでクエリを実行してください。上のフォームに「絞り込みを解除」があります。",
        },
        usageScreen: {
            usage: "使用量",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "送信されたすべてのリクエストを新しい順に並べます。数値のみで、内容は含みません。任意の使用量値を 1 つも報告していない行では、それらをまとめて省略します。ここでは、すべてゼロの末尾と未報告とを区別できません。",
            viewAggregates: "集計を表示",
            filter: "絞り込み",
            theSharedAccountingFilterTheSameOneTheMessage:
                "集計ページと同じ、共通の集計用絞り込みです。",
            window: "期間",
            rollingResolvedWhenTheStreamIsReadMessage:
                "ローリング。ストリームを読み取った時点で確定します。",
            clearFilters: "絞り込みを解除",
            noRequestsRecordedYet: "リクエストはまだ記録されていません",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "ゲートウェイが上流へ送信した時点で、送信済みの推論リクエストごとに 1 行が表示されます。トラフィックがあるのに何も表示されない場合は、ゲートウェイの設定ファイルの api_logs セクションを確認してください。",
            noRecordedRequestsMatchThisFilterMessage:
                "この絞り込みに一致する記録済みのリクエストはありません。",
            filtersStayBehind: " — 期間とその他のフィルタはすべてこちらに残ります。",
            streamMark: " · ストリーミング",
            thisRowIsNoLongerListed: "この行は一覧にありません。",
            reading: "読み取っています…",
            applyFilters: "絞り込みを適用",
            widenTheWindowDropAFilterOrReadMessage:
                "期間を広げるか、絞り込みを外すか、ストリーム全体を読み取ってください。上のフォームに「絞り込みを解除」があります。",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "ゲートウェイの絞り込みは行を探すだけで、値の妥当性は検証しません。存在しない ID や、記録されていないステータスは、単に何にも一致しません。",
            thatIsEveryRowThisFilterMatchesMessage: "この絞り込みに一致する行はこれですべてです。",
            thatIsEveryRecordedRowMessage: "記録されている行はこれですべてです。",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "読み取りサイクルがストリームを置き換えています。サイクルが完了してから、続きを読み込んでください。",
            loading: "読み込んでいます…",
            loadMore: "さらに読み込む",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage: "上流はこのモデルを報告しなくなりました。",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "このアカウントはすでにその正確な上流モデルを保持しています。既存のインベントリ行を開いてください。",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "別の公開ルートがその公開 ID を使用しています。重複しない明示的な名前を選ぶか、既存のルートを編集してください。",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "別のアカウントプールがその名前を使用しています。別のアカウントプール名を選んでください。",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "公開ルートがまだこのアカウントプールを参照しています。アカウントプールを削除する前に、それらのルートを変更または削除してください。",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne:
            "別のキーがすでにそのシークレットを保持しています。新しいシークレットを生成してください。",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "保存済みのバージョンが移っています。上のバーにある更新を使ってください。保存済みのバージョンを読み直し、このフォームには手を加えません。そのうえで再度送信してください。",
        chooseAProviderNameNoOtherRowHolds:
            "ほかのどの行も使っていないプロバイダー名を選んでください。",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "フローが完了する前に、別のプロバイダーがこの名前を取得しました。「やり直す」でこのフローを終了して名前を解放し、ほかの行が使っていない名前で再度作成してください。",
        chooseAnIdentifierNoOtherRowHolds: "ほかのどの行も使っていない識別子を選んでください。",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "プロバイダーの承認が失効している場合は、プロバイダーの編集ページで再接続してください。上流がまだモデルを報告していない場合は、このページで手動で追加できます。",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "プロバイダーのベース URL または API キーが誤っている場合は、プロバイダーの編集ページで修正してください。上流がまだモデルを報告していない場合は、このページで手動で追加できます。",
        checkTheProvidersStateAndCredentials: "プロバイダーの状態と認証情報を確認してください。",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "プロバイダーが交換を拒否しました。このページからフローをやり直してください。",
        checkTheProvidersCredentialsAndReachability:
            "プロバイダーの認証情報と到達性を確認してください。",
        nothingAnsweredAtBaseUrl: "[[baseUrl]] では何も応答しませんでした。",
        theBrowserCouldNotCompleteTheConnection: "ブラウザーが接続を完了できませんでした。",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "アドレスを確認し、ゲートウェイが稼働していて、その listen アドレスにこのブラウザーから到達できることを確かめてください。",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl:
            "HTTPS のページから、HTTP のアドレス [[baseUrl]] は呼び出せません。",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "このページは HTTPS で配信されており、ブラウザーは HTTP アドレスへの呼び出しを全面的に遮断するため、ゲートウェイには一度も到達していません。",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "このアプリを HTTP でも配信するか、そのインスタンスの設定ファイルに tls セクションを追加して、ゲートウェイが HTTPS で応答するようにしてください。",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "ゲートウェイは応答しましたが、ブラウザーが [[baseUrl]] からの応答を遮断しました。",
        theBrowserWithheldTheResponseFromThisPage:
            "ブラウザーがこのページへの応答を渡しませんでした。",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "そのインスタンスの設定ファイルで [[setting]] に [[origin]] を追加し、ゲートウェイを再起動してください。このアプリをゲートウェイ自身のオリジンから配信すれば、許可リストは不要になります。",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "アドレスを確認し、ゲートウェイが稼働していて、このスキームでこのブラウザーから到達できることを確かめてください。",
        noAnswerFromBaseUrl: "[[baseUrl]] から応答がありません。",
        theBrowserReceivedNoAnswerItCouldRead: "ブラウザーは読み取れる応答を受け取りませんでした。",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "そのアドレスで何も待ち受けていないか、このページが読み取る前にブラウザーが応答を遮断した可能性があります。[[setting]] に [[origin]] を追加して、ゲートウェイを再起動してください。",
        theGatewayRejectedThatManagementToken: "ゲートウェイはその管理トークンを拒否しました。",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "トークンは、そのインスタンスの設定ファイルにある management_api.token と一致する必要があります。ゲートウェイは常に読み直すため、トークンを変更しても再起動は不要です。",
        theGatewayHoldsNoSuchRow: "ゲートウェイにそのような行はありません。",
        theGatewayRefusedAnIdentifierIsAlreadyTaken:
            "ゲートウェイが拒否しました：識別子はすでに使われています。",
        theUpstreamProviderRefusedTheCallOrWasUnreachable:
            "上流のプロバイダーが呼び出しを拒否したか、到達できませんでした。",
        theGatewayFailedInternally: "ゲートウェイ内部でエラーが発生しました。",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "原因はゲートウェイの stderr ログにあります。内部的な原因がこのページに返されることはありません。",
        theGatewayRejectedTheRequest: "ゲートウェイはリクエストを拒否しました。",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow:
            "ゲートウェイは、このビルドが知らない形式で応答しました。",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "このビルドが生成したバインディングとゲートウェイの管理 API が一致していません。バインディングを再生成するか、一致するリビジョンからこのフロントエンドを再ビルドしてください。",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "ゲートウェイは HTTP {{httpStatus}} を返しましたが、管理 API のエラー本文がありません。",
        checkTheGatewaysStderrLogForTheCause:
            "原因については、ゲートウェイの stderr ログを確認してください。",
        theCallFailed: "呼び出しに失敗しました。",
    },
    notices: {
        savedName: "[[name]] を保存しました。",
        credentialsReplaced: "認証情報を置き換えました。",
        expiryReplaced: "プロバイダーの一覧に、新しい有効期限が反映されました。",
        keyReplaced: "上流の API キーを置き換えました。",
        previousSecretRevoked:
            "この保存で、以前のシークレットは使えなくなりました。それを保持していたすべてのクライアントを更新してください。",
        lastKeyDisabled:
            "これが最後の有効なキーでした。キーを有効にするまで、ゲートウェイは何も提供できません。",
        modelSaved: "モデルを保存しました。",
        poolSaved: "アカウントプールを保存しました。",
        routeSaved: "公開ルートを保存しました。",
        discovered_other: "[[providerName]] から [[quantity]] 件のモデルを検出しました。",
        nothingDiscovered: "[[providerName]] から新しいモデルは検出されませんでした。",
        manualUntouched_other: "手動で追加した {{quantity}} 件は変更せずに残しました。",
        vanished_other: "{{quantity}} 件のモデルが、報告なしとして記録されました。",
    },
    connectionValidation: {
        empty: "JanusGate のベース URL を入力してください。",
        notAbsolute: "絶対 URL を入力してください。例：https://gateway.example.com。",
        scheme: "http:// または https:// の URL を使用してください。",
        credentials: "URL から認証情報を削除してください。",
        notOrigin:
            "ゲートウェイのオリジンのみを入力してください。パス、クエリ、フラグメントは含めません。",
        managementToken: "管理トークンを入力してください。",
        nameEmpty: "名前は空にできません。",
    },
    operationNotices: {
        flowFailed: "フローに失敗しました。",
        freshFlow: "新しいフローを開始し、速やかに承認してください。",
        flowExpired: "フローが失効しました。",
        flowExpiryPassed: "このページが結果を待つ間に、フローの有効期限が過ぎました。",
        pollStopped: "結果のポーリングが失敗し続けたため、このページは問い合わせを停止しました。",
        pollFailed: "最後の結果ポーリングに失敗しました。",
        invalidDefinition: "この定義 JSON は、ゲートウェイが保存できる定義ではありません。",
        fixDefinition: "JSON を修正して再度送信してください。何も書き込まれていません。",
        fixFilter: "その項目を修正してから、もう一度クエリを実行してください。",
        discoveryPending: "[[providerName]] からモデルを検出しています…",
        discoveryQuery: "上流が現在報告している内容を問い合わせています。",
    },
    counts: {
        models_other: "{{total}} 件のモデル",
        filteredModels_other: "{{total}} 件中 {{shown}} 件のモデル",
        enabledKeys_other: "{{total}} 件中 {{enabled}} 件のキーが有効",
        shownKeys_other: "{{shown}} 件のキーを表示",
        filteredKeys_other: "[[shownSentence]] · [[total]] 件中 [[enabled]] 件のキーが有効",
        providerDeleted_other:
            "削除すると、このアカウント、そのアカウントプールのメンバーシップ、および {{total}} 個の物理モデルとその直接ルートが削除されます。プールのルートは保存されたままです。",
        requests_other: "{{total}} 件のリクエスト",
        requestsPeak_other: "[[total]] 件のリクエスト · [[day]] のピークは [[peak]]",
    },
    helpers: {
        accessControl: {
            allowed: "許可",
            denied: "拒否",
            modelOverride: "モデルの上書き",
            globallyDisabled: "グローバルに無効",
            targetUnavailable: "ターゲットを利用できません",
            targetVanished: "ターゲットが報告されなくなりました",
            globallyAvailable: "グローバルに利用可能",
            noAvailablePoolMembers: "利用可能なアカウントプールのメンバーがありません",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "ベース URL は http:// または https:// のアドレスで、空白を含めないでください。",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "静的キーを使い、OpenAI Responses プロトコルを話す任意のエンドポイント。",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "静的キーを使い、Chat Completions プロトコルを話す任意のエンドポイント。",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "静的キーを使い、Gemini GenerateContent プロトコルを話す任意のエンドポイント。",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "静的キーを使い、Gemini Interactions プロトコルを話す任意のエンドポイント。",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "静的キーを使い、Anthropic Messages プロトコルを話す任意のエンドポイント。",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "静的キーを使う xAI Responses エンドポイント。ネイティブ圧縮は単独の /responses/compact 操作に乗り、明示的トリガーと自動圧縮は非対応と宣言されています。単独の compact エンドポイントがあるからといって、ネイティブのトリガーや自動圧縮を意味するわけではありません。",
            claudeCodeClaudeAccount: "Claude Code（Claude アカウント）",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "Messages API 経由の Claude サブスクリプション アカウント。推論でもモデル検出でも、OAuth トークンは oauth-2025-04-20 ベータ ヘッダーとともに Authorization: Bearer として送られます。",
            kimiCodeKimiAccount: "Kimi Code（Kimi アカウント）",
            githubCopilotGitHubAccount: "GitHub Copilot（GitHub アカウント）",
            devinDevinAccount: "Devin（Devin アカウント）",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "ConnectRPC のトークン交換が静的 API キーと API サーバー URL を返す Devin アカウント。更新も有効期限もなく、モデルは手動で追加します。",
            codexChatGPTAccount: "Codex（ChatGPT アカウント）",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "Codex Responses エンドポイント経由の ChatGPT アカウント。上流ではストリーミングが強制され、応答は OpenAI Responses 形式で描画されます。圧縮セクションでは、Codex バックエンドが受け入れる明示的トリガーと自動しきい値圧縮を宣言しています。",
            antigravityGoogleAccount: "Antigravity（Google アカウント）",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "Antigravity Gemini エンドポイント経由の Google アカウント。プロジェクトのオンボーディングと、封筒で包んだ GenerateContent リクエストを伴います。",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "静的キーを使い、OpenAI Chat Completions プロトコル経由で接続する Zen。",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "静的キーを使い、OpenAI Responses プロトコル経由で接続する Zen。",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "静的キーを使い、Anthropic Messages プロトコル経由で接続する Zen。",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "静的キーを使い、Gemini GenerateContent プロトコル経由で接続する Zen。",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "貼り付けられた内容は、コードそのものでも、コードを含む URL でもありません。ログイン後に到達したアドレス全体か、そのコードだけをコピーして、ここに貼り付けてください。",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "フローは有効なままで、このページは結果を問い合わせ続けます。",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "ゲートウェイにこのアカウントプールはありません。削除されたか、アドレスが誤っています。",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "ゲートウェイにこの公開ルートはありません。削除されたか、アドレスが誤っています。",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "ゲートウェイにこの API キーはありません。削除されたか、アドレスが誤っています。ここから復元することはできません。",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "ゲートウェイにこのモデルはありません。削除されたか、アドレスが誤っています。削除された検出済みモデルは、次回の検出で新規として戻ります。",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "ゲートウェイにこのプロバイダーはありません。削除されたか、アドレスが誤っています。ここから復元することはできません。",
        },
        app: {
            readThisScreensStateFromTheGatewayAgain: "この画面の状態をゲートウェイから読み直す",
            thisScreenReadsNothingFromTheGatewaySo:
                "この画面はゲートウェイから何も読み取らないため、更新する内容はありません",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "切断して保存済みのトークンを破棄しますか？再接続するには、もう一度入力が必要です。",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "必須。このキーを保持するクライアントです。",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "必須。推論クライアントは、すべてのインターフェースでこのキーによる認証を行います。",
        },
        clipboard: {
            copyToClipboard: "クリップボードにコピー",
            copied: "コピーしました",
            copy: "コピー",
            theValueHere: "この値",
        },
        confirmDeleteCard: {
            deleting: "削除しています…",
            deleteForGood: "完全に削除",
        },
        dirtyFormFooter: {
            saving: "保存しています…",
            saveChanges: "変更を保存",
        },
        layout: {
            back: "戻る",
        },
        providerForm: {
            verifiedLive: "実測で確認済み",
            unverified: "未確認",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "必須。この定義のリクエストが向かう上流アドレスです。",
            upstreamAPIKey: "上流の API キー",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "必須。すべてのリクエストとともに上流へ送信されます。ゲートウェイが返すことはありません。",
            showTheAPIKeyWhileTyping: "入力中は API キーを表示",
        },
        apiKeyAccess: {
            inherit: "継承",
            allow: "許可",
            deny: "拒否",
            provider: "プロバイダー",
            pool: "アカウントプール",
            automatic: "自動",
            alias: "別名",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels:
                "検出値に従います。現在はレベルを宣言していません。",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "検出されたセットを置き換えます。何も選ばないと、レベルを宣言しません。",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "検出値に従います。現在は不明なため、一覧では省略されます。",
            replacesTheDiscoveredValueWithAPositiveInteger:
                "検出された値を、正の整数で置き換えます。",
            unset: "未設定",
            discoveredNothingKnown: "検出結果：何も判明していません。",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "検出値に従います。現在は何も判明していないため、定義の宣言が決めます。",
        },
        modelsTable: {
            ready: "準備完了",
            off: "オフ",
            suspended: "一時停止",
            heldByProvider: "プロバイダーが無効",
            unknown: "不明",
            noAutomaticEntry: "自動エントリなし",
            noDirectRoutes: "直接ルートなし",
        },
        providersTable: {
            valid: "有効",
            expired: "期限切れ",
            stale: "有効になったことがない",
            on: "オン",
            off: "オフ",
        },
        usageFilterControls: {
            allOutcomes: "すべての結果",
            failuresUpstreamOrInternal: "失敗（上流または内部）",
            any: "任意",
            streamed: "ストリーミング",
            nonstreamed: "非ストリーミング",
            allEndpoints: "すべてのエンドポイント",
        },
        usageFilter: {
            allTime: "全期間",
            last24Hours: "直近 24 時間",
            last7Days: "直近 7 日間",
            last30Days: "直近 30 日間",
            invalidStatusList:
                "HTTP ステータスには、200, 429 のようなプレーンな整数をカンマ区切りで指定してください。ステータスで絞り込まない場合は空欄にします。",
        },
        oauthFlowView: {
            flowExpires: "フローの有効期限",
            singleuseStateLabel: "使い切り state：",
            authorizationURL: "認可 URL",
            openTheAuthorizationPage: "認可ページを開く",
            theURLAbove: "上記の URL",
            copyTheURL: "URL をコピー",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "state は使い切りで、フローの有効期限は短めです。アクセスを承認したら、速やかに完了してください。",
            mostLoginsEndOnAPageThatShowsMessage:
                "ほとんどのログインは、何も表示しないページで終わりますが、これは想定どおりです。ブラウザーに表示されたアドレス全体を貼り付けてください。コードを表示するページが開いた場合は、代わりにそのコードを貼り付けます。",
            startOver: "やり直す",
            callbackURLOrAuthorizationCode: "コールバック URL または認可コード",
            pasteTheWholeCallbackURL: "コールバック URL を丸ごと貼り付け",
            verificationCode: "確認コード",
            theVerificationCode: "確認コード",
            copyTheCode: "コードをコピー",
            openTheVerificationPage: "確認ページを開く",
            theVerificationURL: "確認 URL",
            approveTheCodeAtTheVerificationPageThisMessage:
                "確認ページでコードを承認してください。このページはゲートウェイに結果を問い合わせ続け、フローが完了するとすぐに先へ進みます。",
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "フローが完了するまで、このページに留まってください。コンソール内を移動すると、フローは破棄されます。",
        },
        poolAvailabilityCard: {
            refreshQuota: "割り当てを更新",
            resetRestrictions: "制限をリセット",
            quotaExhausted: "割り当てを使い切りました",
            poolAvailability: "アカウントプールの可用性",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "保存済みの観測結果です。更新すると、上流アカウントに明示的に照会します。リセットは制限を解除して更新の猶予を設定しますが、上流の割り当てはリセットしません。",
            thisPoolHasNoMembersMessage: "このアカウントプールにはメンバーがいません。",
            readingStoredAvailability: "保存済みの可用性を読み取っています…",
            quotaNotObserved: "割り当ては未観測",
            quotaReadsUnsupported: "割り当ての読み取りに非対応",
            administratorResetRefreshGraceInstalled:
                "管理者がリセット済み。更新の猶予を設定しました",
            accountEnabled: "アカウントは有効です",
            updatingAvailability: "可用性を更新しています…",
            allModels: "すべてのモデル",
            resetUnknown: "リセット時刻は不明",
            resetTimePassedRefreshRequired: "リセット時刻を過ぎました。更新が必要です",
        },
        usageStats: {
            usage: "使用量",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "日別のリクエスト数とトークン数です。数値のみで、内容は含みません。",
            openAggregates: "集計を開く",
            viewRequests: "リクエストを表示",
            requests: "リクエスト",
            dispatchedAllDays: "送信済み（全日）",
            inputTokens: "入力トークン",
            summedUsageReported: "合計（使用量の報告があったもの）",
            outputTokens: "出力トークン",
            day: "日付",
            input: "入力",
            output: "出力",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage:
                "日付は UTC、すなわちゲートウェイ自身の日付境界です。",
        },
    },
    validation: {
        composition: {
            upstreamId:
                "上流 ID に使えるのは英数字と記号のみです。空白や非 ASCII 文字は使用できません。",
            publicId:
                "公開 ID に使えるのは英数字と記号のみです。空白や非 ASCII 文字は使用できません。",
            modelPrefix:
                "モデルプレフィックスに使えるのは英数字と記号のみです。空白や非 ASCII 文字は使用できません。",
            apiKey: "API キーに使えるのは英数字と記号のみです。空白や非 ASCII 文字は使用できません。",
            key: "キーに使えるのは英数字と記号のみです。空白や非 ASCII 文字は使用できません。",
        },
        routing: {
            upstreamId:
                '上流 ID に使えるのは英数字と記号のみです。空白、非 ASCII 文字、"/"、":" は使用できません。',
            publicId:
                '公開 ID に使えるのは英数字と記号のみです。空白、非 ASCII 文字、"/"、":" は使用できません。',
            modelPrefix:
                'モデルプレフィックスに使えるのは英数字と記号のみです。空白、非 ASCII 文字、"/"、":" は使用できません。',
            apiKey: 'API キーに使えるのは英数字と記号のみです。空白、非 ASCII 文字、"/"、":" は使用できません。',
            key: 'キーに使えるのは英数字と記号のみです。空白、非 ASCII 文字、"/"、":" は使用できません。',
        },
        positive: {
            maxInputOverride: "最大入力トークンの上書き値は、正の整数にしてください。",
            maxOutputOverride: "最大出力トークンの上書き値は、正の整数にしてください。",
        },
    },
    quotaMessages: {
        day_other: "{{quantity}} 日",
        hour_other: "{{quantity}} 時間",
        minute_other: "{{quantity}} 分",
        second_other: "{{quantity}} 秒",
        used: "{{percentage}} 使用済み",
        counter: "[[used]] / [[limit]] [[unit]] 使用済み",
        remaining: "残り [[remaining]] [[unit]]",
        dateReset: "[[date]] · 正確なリセット時刻は不明",
        atReset: "[[instant]] にリセット · [[distance]]",
        future: "{{duration}}後",
        observed: "[[instant]] に観測 · [[distance]]",
        inventory: {
            pool_disabled: "プールが無効",
            account_missing: "アカウントがありません",
            account_disabled: "アカウントが無効",
            model_not_selected: "モデルの可用性を確認する上流 ID を選択してください",
            model_missing: "モデルがありません",
            model_disabled: "モデルが無効",
            model_vanished: "モデルが報告されなくなりました",
            ready: "インベントリ準備完了",
        },
        failure: {
            credential_renewal: "認証情報の更新に失敗",
            timeout: "タイムアウト",
            transport: "通信に失敗",
            upstream_rejected: "上流がリクエストを拒否",
            malformed_observation: "観測結果の形式が不正",
        },
        quotaObserved: "[[instant]] に割り当てを観測",
        quotaFailed: "割り当ての読み取りに失敗しました · {{reason}}",
        deletedAccount: "削除済みアカウント #{{id}}",
        priority: "優先度 {{priority}}",
        models: "モデル：[[identifiers]]",
        resets: "[[instant]] にリセット",
    },
    dynamicMessages: {
        following: "検出値に従います：{{value}}。",
        levelsReplace:
            "検出されたセットを置き換えます — クライアントには {{value}} が表示されます。",
        capReplace: "検出された値を置き換えます — クライアントには {{value}} が表示されます。",
        mechanismFact: "{{mechanism}}：{{support}}",
        mechanismDiscovered: "検出結果：{{support}}。",
        compactionReplace:
            "検出された情報をまとめて置き換えます。未設定のメカニズムは、まず検出結果に、次に定義の宣言にフォールバックします。",
        noExpiry: "このプロバイダーには有効期限が保存されていません。",
        goneFlow:
            "この認可フローは完了できなくなりました。state が失効したか、すでに使用されています。",
        refusedClipboard:
            "このブラウザーはクリップボードを拒否しました。{{subject}}を選択して、手動でコピーしてください。",
        enabledStored: "保存済み {{total}} 件のうち有効な件数",
        readyListed: "一覧 {{total}} 件のうち準備完了の件数",
        copyEndpoint: "{{surface}} エンドポイントの URL をコピー",
        copyApiKeySecret: "{{name}} のシークレットをコピー",
        disabledCredential: "{{expiry}} プロバイダーは無効です。有効に戻したら再接続してください。",
        noKeysDetail:
            " 推論クライアントは API キーで認証します。キーがないと、ゲートウェイに準備完了のインベントリがありません。",
    },
    remainingText: {
        notRefreshedYet: "まだ更新していません",
        overrideMaxInputTokens: "最大入力トークンを上書き",
        maxInputTokensOverride: "最大入力トークンの上書き",
        overrideMaxOutputTokens: "最大出力トークンを上書き",
        maxOutputTokensOverride: "最大出力トークンの上書き",
        chooseAProvider: "プロバイダーを選択",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "上流が報告しなくなったプロバイダーと上流 ID を指定すると、以下の設定でそのモデルを取り戻します。モデルは、単独プロバイダーのプレフィックスまたは所有するアカウントプールのプレフィックスを通じて自動的に公開されます。",
        allProviders: "すべてのプロバイダー",
        replaceTheUpstreamAPIKey: "上流の API キーを置き換え",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "書き込み専用です。ゲートウェイは返さないため、空欄のままにすると保存済みの値を保持します。",
        backToPools: "アカウントプールに戻る",
        publicRoute: "公開ルート",
        publicRoutes: "公開ルート",
        backToPublicRoutes: "公開ルートに戻る",
        backToAPIKeys: "API キーに戻る",
        backToModels: "モデルに戻る",
        backToProviders: "プロバイダーに戻る",
        deletedRoute: "削除済みルート",
        deletedModel: "削除済みモデル",
        deletedProvider: "削除済みプロバイダー",
        deletedKey: "削除済みキー",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "以下に表示されているのは、まだ前回のクエリの結果です — インライン エラーが出ている項目が問題箇所です。",
        fromAndToDoNotApplyThere: " — 開始と終了はそこには適用されません。",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "送信済みの推論リクエストごとに 1 行が表示されます — ゲートウェイが上流へ送信した時点です。",
        showTheKeyWhileTyping: "入力中はキーを表示",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "保存すると、あなたの編集がゲートウェイのバージョンに上書きされます。「元に戻す」はゲートウェイのバージョンを取り込みます。",
        theOAuthCredentialsWereNeverFresh: "OAuth の認証情報は、有効になったことがありません。",
        allPublicRoutes: "すべての公開ルート",
        allPhysicalModels: "すべての物理モデル",
        allAPIKeys: "すべての API キー",
    },
    entityMessages: {
        editName: "[[name]] を編集",
        editIdentifier: "[[identifier]] を編集",
        deleteKey:
            "[[name]] を削除しますか？このシークレットを保持するクライアントは、アクセスできなくなります。",
        deleteLastKey:
            "[[name]] を削除しますか？これは最後の有効なキーです — 別のキーを有効にするまで、ゲートウェイは何も提供できません。",
        deleteModel:
            "[[identifier]] を削除しますか？その直接ルートも削除されます。プールのルートは保存されたままです。再検出または手動作成では新しい物理行が作られ、削除されたルートは復元されません。",
        deleteProvider: "[[name]] を削除しますか？",
        switchPreset: "[[name]] に切り替えますか？編集した接続レシピは置き換えられます。",
        noKeysMatch: "「[[query]]」に一致する API キーはありません。",
        noProvidersMatch: "「[[query]]」に一致するプロバイダーはありません。",
        noModelsMatch: "「[[query]]」に一致するモデルはありません。",
        noProviderModelsMatch: "このプロバイダーに「[[query]]」に一致するモデルはありません。",
        missingPath:
            "[[path]] には何もありません。削除されたか、アドレスが誤っている可能性があります。",
        aliasCreated: "別名を作成しました：[[identifier]]",
        addAlias: "[[identifier]] の別名を追加",
    },
    modelCard: {
        heading: "モデル · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "このキーは、未保存の編集がある間にゲートウェイ上で変更されました。",
            provider: "このプロバイダーは、未保存の編集がある間にゲートウェイ上で変更されました。",
            model: "このモデルは、未保存の編集がある間にゲートウェイ上で変更されました。",
            pool: "このアカウントプールは、未保存の編集がある間にゲートウェイ上で変更されました。",
            "public route":
                "この公開ルートは、未保存の編集がある間にゲートウェイ上で変更されました。",
            "api key": "この API キーは、未保存の編集がある間にゲートウェイ上で変更されました。",
        },
        noRequests: {
            key: "このキーのリクエストは、まだ記録されていません。",
            provider: "このプロバイダーのリクエストは、まだ記録されていません。",
            model: "このモデルのリクエストは、まだ記録されていません。",
            pool: "このアカウントプールのリクエストは、まだ記録されていません。",
            "public route": "この公開ルートのリクエストは、まだ記録されていません。",
            "api key": "この API キーのリクエストは、まだ記録されていません。",
        },
        notFound: {
            api_key: "API キーが見つかりません",
            provider: "プロバイダーが見つかりません",
            model: "モデルが見つかりません",
            pool: "アカウントプールが見つかりません",
            model_route: "公開ルートが見つかりません",
        },
        replaceSecret: {
            typed: "入力したシークレットを置き換えますか？",
            current: "現在のシークレットを置き換えますか？",
        },
        bound: {
            From: "開始には、2026-09-23T10:00 のような完全な日時を指定してください。範囲を設けない場合は空欄にします。",
            To: "終了には、2026-09-23T10:00 のような完全な日時を指定してください。範囲を設けない場合は空欄にします。",
        },
        updated: "{{instant}} に更新",
        created: "{{instant}} に作成",
        version: "バージョン {{version}} として保存されています。",
        unlisted: "{{value}}（一覧にありません）",
        accountUnlisted: "アカウント #{{id}}（一覧にありません）",
        moveUp: "{{name}} を上へ移動",
        moveDown: "{{name}} を下へ移動",
        remove: "{{name}} を削除",
        publicIdFor: "{{subject}} の公開 ID",
        source: {
            key: "キー由来",
            provider: "プロバイダー由来",
            pool: "プール由来",
            route: "モデルの上書き",
        },
        deleted: {
            provider: "削除済みプロバイダー #{{id}}",
            pool: "削除済みアカウントプール #{{id}}",
            route: "削除済みルート #{{id}}",
        },
        status: "クライアント {{client}} · 上流 {{upstream}}",
        metrics: " · {{duration}} · 入力 {{input}} · 出力 {{output}}{{tail}}",
        optionalTokens:
            " · キャッシュ {{cached}} · キャッシュ書き込み {{write}} · 推論 {{reasoning}}",
    },
    routingMessages: {
        directRoutes_other: "{{quantity}} 件の直接ルート",
        poolMembers_other: "[[quantity]] 件のアカウントメンバー · モデルプレフィックス：[[prefix]]",
        directMissing: "直接 · 物理モデル #[[id]]",
        direct: "直接 · [[provider]] · [[model]]",
        pool: "アカウントプール · [[pool]] · [[model]]",
        publicIds: "公開 ID [[identifiers]]",
        modelAccess: "[[identifier]] のモデルアクセス",
        ownerDefault: {
            provider: "[[name]] のプロバイダー既定",
            pool: "[[name]] のアカウントプール既定",
        },
        directTarget: "直接 · モデル #[[id]]",
        poolTarget: "アカウントプール · [[identifier]]",
        reconnect: "[[editLink]]で再接続してください — 認証情報カードが置き換えます。",
        reconnectDisabled:
            "プロバイダーは無効です。再有効化したら再接続してください。[[editLink]]の認証情報カードが置き換えます。",
    },
    linkedMessages: {
        keyEndpoint:
            "クライアントをエンドポイントに向けてください — すべてのインターフェースが、許可された公開エントリを提供します。URL は [[overviewLink]] にあります。",
        servingKey:
            "推論クライアントは推論用 API キーで認証します。何かをここに向ける前に、[[keysLink]]でキーを作成してください。",
        vanished:
            "それまでの間、このモデルは何も提供しません。上流が再び一覧に載せたら[[discoverLink]]、または[[claimLink]]。",
        discoverLabel: "プロバイダーのページで再検出",
        claimLabel: "モデル作成ページで取り戻す",
        selected: "{{quantity}} 件を選択しました。成功した行は再度送信されません。",
        kimiDescription:
            "Coding エンドポイント経由の Kimi アカウント。デバイスコード フローで、デバイス ID のスロットは認可、ポーリング、更新のいずれでもカスタムの X-Msh-* ヘッダーで送られます。",
        githubDescription:
            "再プロビジョニングによって、長期トークンを短命の Copilot セッション トークンとアカウントの API エンドポイントに交換する GitHub アカウント。",
        chartPoint_other: "[[day]]：[[quantity]] 件のリクエスト",
        inventoryEmpty: "[[name]] にはまだモデルがありません。",
        inventoryUnready:
            "[[name]] には準備完了のインベントリがありません — すべてのモデルが無効か、報告されなくなっています。",
    },
    vocabulary: {
        credentialKind: {
            api_key: "静的 API キー",
            oauth: "OAuth アカウント",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "単独 compact",
            responses_explicit_trigger: "明示的トリガー",
            responses_automatic_compaction: "自動圧縮",
            messages_on_demand_compaction: "オンデマンド圧縮",
            messages_threshold_compaction: "しきい値圧縮",
        },
        mechanismSupport: {
            supported: "対応",
            unsupported: "非対応",
            unknown: "不明",
        },
        discoveryState: {
            discovered: "検出済み",
            vanished: "報告なし",
            manual: "手動",
        },
        reasoningLevel: {
            none: "なし",
            minimal: "最小",
            low: "低",
            medium: "中",
            high: "高",
            xhigh: "極高",
            max: "最大",
        },
        apiLogOutcome: {
            completed: "完了",
            client_aborted: "クライアント中断",
            upstream_error: "上流エラー",
            internal: "内部エラー",
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
            day: "日付",
            model: "物理モデル",
            model_route: "公開ルート",
            provider: "プロバイダー",
            api_key: "API キー",
            endpoint_type: "エンドポイント",
            outcome: "結果",
            is_stream: "ストリームモード",
            http_status: "HTTP ステータス",
        },
    },
    common: {
        save: "変更を保存",
        cancel: "キャンセル",
        retry: "再試行",
        language: "言語",
    },
    language: {
        system: "ブラウザーに合わせる",
        loading: "{{languageName}}を読み込んでいます…",
        failed: "{{languageName}}を読み込めませんでした。現在の言語を引き続き表示します。",
        startupFailed: "{{languageName}}を読み込めませんでした。現在は英語を表示しています。",
    },
    models: {
        discoveredCount_other: "{{count}} 件のモデルを検出しました。",
    },
} as const;
export default japanese;
