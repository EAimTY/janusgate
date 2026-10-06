/** Spanish administration messages. */
const spanish = {
    shell: {
        skipToContent: "Ir al contenido",
        primary: "Principal",
        refresh: "Actualizar",
        disconnect: "Desconectar",
        keep: "Conservar",
    },
    connection: {
        connectToJanusGate: "Conectar con JanusGate",
        thisAppAdministersAJanusGateGatewayItsUpstream:
            "Esta aplicación administra una pasarela JanusGate: sus cuentas upstream, sus modelos y sus credenciales.",
        aRunningJanusGateInstanceReachableFromThisBrowser:
            "Una instancia de JanusGate en ejecución, accesible desde este navegador.",
        theOriginBesideIt: "el origen que figura al lado",
        copyThisAppsOrigin: "Copiar el origen de esta aplicación",
        gatewayAddress: "Dirección de la pasarela",
        schemeHostAndPortOnly: "Solo esquema, host y puerto.",
        managementToken: "Token de administración",
        sentOnlyToThatInstanceAndKeptOnly:
            "Se envía solo a esa instancia y se conserva solo en este navegador.",
        cancel: "Cancelar",
    },
    controls: {
        modelAccess: "Acceso a modelos",
        defaultsAffectExistingInheritingEntriesPoolAccessIncludes:
            "Los valores predeterminados afectan a las entradas existentes que los heredan. El acceso de grupo incluye a los futuros miembros aptos. La disponibilidad global es independiente del permiso.",
        defaultModelAccessLabel: "Acceso a modelos predeterminado:",
        defaultModelAccess: "Acceso a modelos predeterminado",
        appliesToEntriesWithoutAProviderPoolOr:
            "Se aplica a las entradas sin proveedor, grupo ni anulación de modelo, incluidas las futuras.",
        searchModelAccess: "Buscar acceso a modelos",
        publicIDOrOwnerName: "ID público o nombre del propietario",
        overridesOnly: "Solo anulaciones",
        removeOverride: "Quitar anulación",
        removeDeletedReferencesBeforeSavingOrRefreshTo:
            "Elimina las referencias borradas antes de guardar, o actualiza para resolver la discrepancia del inventario.",
        thisDefaultAppliesToFutureEntries:
            "Este valor predeterminado se aplica a las entradas futuras.",
        noEntriesMatchThisViewHiddenOverridesAre:
            "Ninguna entrada coincide con esta vista. Las anulaciones ocultas se conservan.",
        name: "Nombre",
        key: "Clave",
        skOrGenerateOne: "sk-… o generar una",
        generate: "Generar",
        replaceIt: "Reemplazarla",
        keepIt: "Conservarla",
        theSecretHere: "el secreto que figura aquí",
        delete: "Eliminar…",
        revert: "Revertir",
        unsavedChanges: "Cambios sin guardar",
        overrideReasoningLevels: "Anular niveles de razonamiento",
        reasoningLevels: "Niveles de razonamiento",
        pickEveryLevelThisModelDeclaresUntickAll:
            "Marca todos los niveles que declara este modelo. Desmárcalos todos para no declarar ninguno.",
        aPositiveIntegerClearTheSwitchToFollow:
            "Un número entero positivo. Desactiva el interruptor para volver a seguir el descubrimiento.",
        overrideCompactionFacts: "Anular datos de compactación",
        compactionMechanisms: "Mecanismos de compactación",
        stateEveryMechanismWhoseFactsThisModelReplaces:
            "Indica todos los mecanismos cuyos datos reemplaza este modelo. El estado «sin dato» es explícito: suprime el dato descubierto para ese mecanismo.",
        addAlias: "Añadir alias",
        modelPrefix: "Prefijo de modelo",
        outsidePoolsModelsAreCallableAsThisPrefix:
            "Fuera de los grupos, los modelos se invocan como este prefijo + el ID upstream. Los miembros usan los prefijos de su grupo. Déjalo vacío para usar los ID upstream directamente. Cambiar el prefijo modifica los nombres automáticos de modelos ya existentes.",
        upstreamBaseURL: "URL base upstream",
        enabled: "Activado",
        publicRoute: "Ruta pública",
        provider: "Proveedor",
        physicalModel: "Modelo físico",
        apiKey: "Clave de API",
        endpoint: "Punto de conexión",
        outcome: "Resultado",
        streamMode: "Modo de transmisión",
        httpStatuses: "Estados HTTP",
        commaseparatedEg200429NeveransweredRequestsMatchNo:
            "Separados por comas; por ejemplo, 200, 429. Las solicitudes nunca respondidas no coinciden con ningún estado.",
    },
    navigation: {
        overview: "Resumen",
        providers: "Proveedores",
        inventory: "Inventario",
        pools: "Grupos",
        routes: "Rutas públicas",
        apiKeys: "Claves de API",
        usage: "Uso",
        lightTheme: "Tema claro — cambiar a oscuro",
        darkTheme: "Tema oscuro — cambiar a sistema",
        systemTheme: "Tema del sistema — cambiar a claro",
    },
    rich: {
        connected: "Conectado a [[baseUrl]] — el token solo vive en este navegador.",
        setupToken:
            "Su token de administración: [[setting]] en el archivo de configuración de esa instancia.",
        setupCors:
            "Permite el origen de esta aplicación en [[setting]] de esa instancia, salvo si esta aplicación se sirve desde la propia pasarela.",
    },
    titles: {
        bulkPublication: "Publicar inventario",
        providerPools: "Grupos",
        providerPoolNew: "Crear grupo",
        providerPoolEdit: "Editar grupo n.º {{id}}",
        modelRoutes: "Rutas públicas",
        modelRouteNew: "Crear ruta pública",
        modelRouteEdit: "Editar ruta pública n.º {{id}}",
        home: "Resumen",
        providers: "Proveedores",
        providerNew: "Crear proveedor",
        providerDetail: "Proveedor n.º {{id}}",
        providerEdit: "Editar proveedor n.º {{id}}",
        models: "Modelos",
        modelNew: "Crear modelo",
        modelDetail: "Modelo n.º {{id}}",
        modelEdit: "Editar modelo n.º {{id}}",
        apiKeys: "Claves de API",
        apiKeyNew: "Crear clave de API",
        apiKeyDetail: "Clave de API n.º {{id}}",
        apiKeyEdit: "Editar clave de API n.º {{id}}",
        usage: "Uso",
        usageQuery: "Agregados de uso",
        notFound: "Página no encontrada",
    },
    format: {
        lessThanMinute: "menos de un minuto",
        minutes: "{{minutes}} min",
        hours: "{{hours}} h",
        hoursMinutes: "{{hours}} h {{minutes}} min",
        days: "{{days}} d",
        daysHours: "{{days}} d {{hours}} h",
        expired: "Caducó hace {{distance}} — el {{instant}}.",
        valid: "Válido durante {{distance}} — hasta {{instant}}.",
        durationSeconds: "{{seconds}} s",
        durationMinutesSeconds: "{{minutes}} min {{seconds}} s",
    },
    screens: {
        apiKeyDetailScreen: {
            apiKey: "Clave de API",
            editAPIKey: "Editar clave de API",
            secret: "Secreto",
            overview: "Resumen",
            details: "Detalles",
            name: "Nombre",
            created: "Creada",
            status: "Estado",
        },
        apiKeyEditScreen: {
            editAPIKey: "Editar clave de API",
            settings: "Ajustes",
            rotatingMeansGenerateThenSaveMessage: "Rotar significa generar y luego guardar.",
            enabled: "Activado",
            replacingTheSecretRevokesTheOldValueOnMessage:
                "Reemplazar el secreto revoca el valor anterior al guardar.",
            aKeyNeedsASecret: "Una clave necesita un secreto.",
            deleteThisKey: "Eliminar esta clave",
            deletingRevokesTheKeyAtOnceClientsHoldingMessage:
                "Eliminar revoca la clave de inmediato. Los clientes que la tengan empezarán a recibir errores 401. Sus reglas de acceso a modelos se eliminan; la contabilidad histórica se conserva.",
        },
        apiKeyNewScreen: {
            createAPIKey: "Crear clave de API",
            oneSecretYourServingClientsShareMessage:
                "Un único secreto que comparten tus clientes de servicio.",
            apiKeysBackLabel: "Claves de API",
            key: "Clave",
            theSecretHoldsLettersNumbersAndSymbolsOnlyMessage:
                "El secreto solo contiene letras, números y símbolos —sin espacios— y ninguna otra clave lo comparte.",
            enabled: "Activado",
            loadModelAccessInventoryBeforeCreatingAKeyMessage:
                "Carga el inventario de acceso a modelos antes de crear una clave.",
        },
        apiKeysScreen: {
            apiKeys: "Claves de API",
            servingKeysOpenAIClientsSendOneAsAuthorizationMessage:
                "Claves de servicio. Los clientes OpenAI envían una como Authorization: Bearer <key>, los de Gemini como x-goog-api-key y los de Anthropic como x-api-key; abre una para ver su secreto y su uso.",
            createAPIKey: "Crear clave de API",
            noAPIKeysYet: "Aún no hay claves de API",
            aFreshGatewayServesNothingUntilTheFirstMessage:
                "Una pasarela recién instalada no sirve nada hasta que exista la primera clave.",
            searchAPIKeys: "Buscar claves de API",
            searchByNameOrKey: "Buscar por nombre o clave",
            theSecretHere: "el secreto que figura aquí",
        },
        bulkPublicationScreen: {
            createAliases: "Crear alias",
            choosePhysicalModelsAndAuthorTheirPublicIDsMessage:
                "Elige modelos físicos y define sus ID públicos. Cada selección crea una ruta directa activada; los conflictos de nombres se notifican sin renombrar ni sobrescribir.",
            inventoryBackLabel: "Inventario",
            noPhysicalModelsToAlias: "No hay modelos físicos que asignar como alias",
            discoverOrAddInventoryFirstMessage: "Primero descubre o añade inventario.",
            anotherSelectedRowUsesThisPublicIDChooseDistinctExplicitNames:
                "Otra fila seleccionada usa este ID público. Elige nombres explícitos distintos.",
            openRoute: "Abrir ruta",
            createSelectedAliases: "Crear los alias seleccionados",
        },
        homeScreen: {
            overview: "Resumen",
            theHealthOfThisGatewayAtAGlanceMessage: "El estado de esta pasarela de un vistazo.",
            createProvider: "Crear proveedor",
            providers: "Proveedores",
            publicRoutes: "Rutas públicas",
            inventoryReady: "Inventario listo",
            apiKeys: "Claves de API",
            recentRequests24h: "Solicitudes recientes (24 h)",
            recentErrors24h: "Errores recientes (24 h)",
            requestsPerDay: "Solicitudes por día",
            theLast14DaysInUTCTheGatewaysMessage:
                "Los últimos 14 días, en UTC — el propio límite diario de la pasarela.",
            openUsage: "Abrir uso",
            storedSettingsReviewedMessage: "Ajustes almacenados revisados.",
            needsAttention: "Requiere atención",
            noModelsAreAvailableDiscoverModelsOnAnMessage:
                "No hay modelos disponibles. Descubre modelos en un proveedor activado.",
            noAPIKeysYetMessage: "Aún no hay claves de API.",
            endpoints: "Puntos de conexión",
            everySurfaceServesEveryModelPointAnyOpenAIMessage:
                "Todas las interfaces sirven todos los modelos. Apunta cualquier cliente con formato OpenAI, Gemini o Anthropic a esta instancia.",
            theURLBesideIt: "la URL que figura al lado",
            noProvidersYet: "Aún no hay proveedores",
            aProviderIsOneUpstreamAccountItsModelsMessage:
                "Un proveedor es una cuenta upstream. Sus modelos aparecen cuando los descubres.",
            createYourFirstProvider: "Crea tu primer proveedor",
            connectedToTheGateway: "Conectado a la pasarela",
            discoverModels: "Descubrir modelos",
            createAPIKey: "Crear clave de API",
            openAndDiscover: "Abrir y descubrir",
            reviewModels: "Revisar modelos",
            credentialNotFresh: "Credencial no vigente",
            credentialExpired: "Credencial caducada",
            reconnect: "Reconectar",
            openProvider: "Abrir proveedor",
            createAProvider: "Crea un proveedor",
            discoverItsModelsFromUpstream: "Descubre sus modelos desde upstream",
            issueAnAPIKeyForServingClients: "Emite una clave de API para los clientes de servicio",
            pointAClientAtAnEndpointBelow:
                "Apunta un cliente a uno de los puntos de conexión de abajo",
        },
        modelDetailScreen: {
            model: "Modelo",
            upstreamOnProvider: "{{upstreamId}} en {{provider}}.",
            modelsBackLabel: "Modelos",
            capabilities: "Capacidades",
            whatTheModelListingsDeclareForThisModelMessage:
                "Lo que los listados de modelos declaran para este modelo. Nada de aquí bloquea una solicitud: el upstream responde lo que responde.",
            reasoningLevels: "Niveles de razonamiento",
            maxInputTokens: "Máximo de tokens de entrada",
            maxOutputTokens: "Máximo de tokens de salida",
            compaction: "Compactación",
            theRowsOwnCompactionFactsTheOverrideElseMessage:
                "Los datos de compactación de la propia fila: la anulación o, si no, el último descubrimiento. La declaración de la definición decide todo lo que ambos dejen sin especificar.",
            noLevels: "Sin niveles",
            addAlias: "Añadir alias",
            editModel: "Editar modelo",
            details: "Detalles",
            publicModelNames: "Nombres públicos de modelos",
            directRoutes: "Rutas directas",
            upstreamID: "ID upstream",
            provider: "Proveedor",
            discovery: "Descubrimiento",
            status: "Estado",
        },
        modelEditScreen: {
            editModel: "Editar modelo",
            modelSettings: "Ajustes del modelo",
            enabled: "Activado",
            deleteThisModel: "Eliminar este modelo",
            deletingRemovesThisPhysicalModelAndItsDirectMessage:
                "Eliminar quita este modelo físico y sus rutas directas. Las rutas de grupo se conservan almacenadas.",
        },
        modelNewScreen: {
            createModel: "Crear modelo",
            somethingDiscoveryWouldNotProduceOrAModelMessage:
                "Algo que el descubrimiento no produciría, o un modelo del que el upstream ya no informa, para reclamarlo.",
            noProvidersYet: "Aún no hay proveedores",
            aModelBelongsToAProviderAndThereMessage:
                "Un modelo pertenece a un proveedor, y no hay ninguno que lo aloje. Añade primero un proveedor; puede que el descubrimiento ya produzca este modelo.",
            createProvider: "Crear proveedor",
            aModelAddedByHandIsNeverRemovedMessage:
                "Un modelo añadido a mano nunca se elimina automáticamente, y reclama lo que el descubrimiento perdió.",
            models: "Modelos",
            identity: "Identidad",
            provider: "Proveedor",
            upstreamID: "ID upstream",
            modelSettings: "Ajustes del modelo",
            enabled: "Activado",
        },
        modelRouteEditorScreen: {
            thisModelNameIsManagedAutomaticallyEditTheMessage:
                "Este nombre de modelo se gestiona automáticamente. Edita el prefijo del proveedor o del grupo propietario para cambiarlo.",
            deleteThisPublicRoute: "Eliminar esta ruta pública",
            clientsRequestingItsPublicIDWillReceiveModelnotfoundMessage:
                "Los clientes que soliciten su ID público recibirán un error de modelo no encontrado. El inventario físico y los grupos se conservan almacenados.",
            publicRouteSettings: "Ajustes de la ruta pública",
            enabled: "Activado",
            publicID: "ID público",
            theExactModelNameClientsRequestRenamingRequiresMessage:
                "El nombre exacto de modelo que solicitan los clientes. Renombrarlo obliga a los clientes a actualizarse.",
            targetType: "Tipo de destino",
            pool: "Grupo",
            upstreamID: "ID upstream",
            exactIdentifierRequestedOnEveryMemberMissingModelsMessage:
                "Identificador exacto solicitado en todos los miembros. Que falten modelos no impide la publicación.",
            physicalModel: "Modelo físico",
            createPublicRoute: "Crear ruta pública",
        },
        modelsScreen: {
            models: "Modelos",
            modelsAreAutomaticallyExposedThroughAStandaloneProviderMessage:
                "Los modelos se exponen automáticamente mediante el prefijo de un proveedor independiente o los prefijos de su grupo propietario.",
            createAliases: "Crear alias",
            createModel: "Crear modelo",
            noModelsYet: "Aún no hay modelos",
            addAProviderFirstThenDiscoverItsModelsMessage:
                "Añade primero un proveedor y luego descubre sus modelos, o añade uno a mano.",
            thisProviderHoldsNoModels: "Este proveedor no tiene modelos.",
            thisProviderIsNoLongerListedPickAnotherInTheProviderFilter:
                "Este proveedor ya no figura en la lista; elige otro en el filtro Proveedor.",
            createProvider: "Crear proveedor",
            searchModels: "Buscar modelos",
            searchByUpstreamIDOrProviderName: "Buscar por ID upstream o nombre de proveedor",
            provider: "Proveedor",
        },
        notFoundScreen: {
            pageNotFound: "Página no encontrada",
            goToOverview: "Ir al resumen",
            providers: "Proveedores",
            models: "Modelos",
        },
        oauthFlowView: {
            flowExpires: "El flujo caduca",
            singleuseStateLabel: "State de un solo uso:",
            authorizationURL: "URL de autorización",
            openTheAuthorizationPage: "Abrir la página de autorización",
            theURLAbove: "la URL de arriba",
            copyTheURL: "Copiar la URL",
            theStateIsSingleuseAndTheFlowExpiresMessage:
                "El state es de un solo uso y el flujo caduca enseguida; termínalo sin demora tras aprobar el acceso.",
            mostLoginsEndOnAPageThatShowsMessage:
                "La mayoría de los inicios de sesión terminan en una página que no muestra nada; es lo esperado. Pega la dirección completa que muestra el navegador. Si se abre una página y muestra un código, pega ese código en su lugar.",
            startOver: "Empezar de nuevo",
            callbackURLOrAuthorizationCode: "URL de callback o código de autorización",
            pasteTheWholeCallbackURL: "Pega la URL de callback completa",
            verificationCode: "Código de verificación",
            theVerificationCode: "el código de verificación",
            copyTheCode: "Copiar el código",
            openTheVerificationPage: "Abrir la página de verificación",
            theVerificationURL: "la URL de verificación",
            approveTheCodeAtTheVerificationPageThisMessage:
                "Aprueba el código en la página de verificación; esta página sigue preguntando a la pasarela por el resultado y continúa en cuanto el flujo termina.",
            stayOnThisPageUntilTheFlowCompletesNavigatingWithinTheConsoleDiscardsThe:
                "Permanece en esta página hasta que el flujo termine; navegar dentro de la consola descarta el flujo.",
        },
        poolAvailabilityCard: {
            refreshQuota: "Actualizar cuota",
            resetRestrictions: "Restablecer restricciones",
            quotaExhausted: "Cuota agotada",
            poolAvailability: "Disponibilidad del grupo",
            storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage:
                "Observaciones almacenadas. Actualizar consulta explícitamente la cuenta upstream. Restablecer borra las restricciones e instala una prórroga de actualización; no restablece la cuota del upstream.",
            thisPoolHasNoMembersMessage: "Este grupo no tiene miembros.",
        },
        providerDetailScreen: {
            provider: "Proveedor",
            editProvider: "Editar proveedor",
            editPage: "página de edición",
            storedAvailability: "Disponibilidad almacenada",
            allObservedRestrictionsOnThisAccountResetClearsMessage:
                "Todas las restricciones observadas en esta cuenta. Restablecer borra las restricciones e instala una prórroga de actualización; no restablece la cuota del upstream.",
            details: "Detalles",
            modelPrefix: "Prefijo de modelo",
            type: "Tipo",
            definition: "Definición",
            upstreamBaseURL: "URL base upstream",
            credentials: "Credenciales",
            status: "Estado",
            discoveredFromUpstreamOrAddedByHandMessage:
                "Descubiertos desde upstream o añadidos a mano.",
            createModel: "Crear modelo",
            allModels: "Todos los modelos",
        },
        providerEditScreen: {
            editProvider: "Editar proveedor",
            settings: "Ajustes",
            credentials: "Credenciales",
            oauthCredentialsExpireReconnectingReplacesThemMessage:
                "Las credenciales OAuth caducan; reconectar las reemplaza.",
            thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert:
                "La definición de este proveedor evalúa las solicitudes contra una URL base; restaura una, o usa Revertir.",
            deleteThisProvider: "Eliminar este proveedor",
        },
        providerNewScreen: {
            createProvider: "Crear proveedor",
            oneUpstreamAccountPickThePresetTheCredentialMessage:
                "Una cuenta upstream. Elige el preajuste y la ruta de credenciales vendrá dada.",
            providersBackLabel: "Proveedores",
            readAgain: "Leer de nuevo",
            preset: "Preajuste",
            switchPreset: "Cambiar de preajuste",
            keepMyEdit: "Conservar mi edición",
            identity: "Identidad",
            connection: "Conexión",
            theKeyIsWriteonlyTheGatewayNeverReturnsMessage:
                "La clave es de solo escritura: la pasarela nunca la devuelve.",
            connectTheAccount: "Conectar la cuenta",
            lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows:
                "Bloqueado mientras la autorización está activa; el proveedor se crea a partir de lo que ya envió el inicio del flujo.",
            approveAccessWhereTheFlowPointsThenLetMessage:
                "Aprueba el acceso donde indique el flujo y deja que se complete. Esta aplicación nunca almacena las credenciales.",
            upstreamBaseURL: "URL base upstream",
            definition: "Definición",
            theConnectionRecipeTheGatewayStoresEditingTheMessage:
                "La receta de conexión que almacena la pasarela. Editar el JSON de una definición almacenada registra la siguiente versión; la almacenada nunca se reescribe.",
            notStoredYetSubmittingCreatesVersion1Message:
                "Aún no está almacenada; enviarla crea la versión 1.",
            advancedTheConnectionRecipePrefilledByThePreset:
                "Avanzado: la receta de conexión, rellenada previamente por el preajuste",
            definitionJSON: "JSON de la definición",
            resetToPreset: "Restablecer al preajuste",
            theConnectionRecipeIsInvalidOpenTheAdvancedMessage:
                "La receta de conexión no es válida; abre el apartado Avanzado de arriba para ver la ruta del error.",
        },
        providerPoolEditorScreen: {
            interchangeableAccountsForTheSameUpstreamServiceTriedMessage:
                "Cuentas intercambiables para el mismo servicio upstream, probadas por orden de prioridad.",
            inspectUpstreamID: "Inspeccionar ID upstream",
            inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage:
                "Inspecciona la pertenencia guardada. Dejarlo vacío muestra solo las restricciones de toda la cuenta.",
            deleteThisPool: "Eliminar este grupo",
            deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage:
                "Eliminar quita las entradas automáticas del grupo y restaura los prefijos independientes de los miembros. Las rutas creadas a mano deben eliminarse o redirigirse primero.",
            poolSettings: "Ajustes del grupo",
            enabled: "Activado",
            name: "Nombre",
            modelPrefix: "Prefijo de modelo",
            thePoolAutomaticallyExposesMemberModelsAsThisMessage:
                "El grupo expone automáticamente los modelos de sus miembros como este prefijo + el ID upstream. Déjalo vacío para usar los ID upstream exactos. Cambiar el prefijo renombra las entradas automáticas existentes.",
            accountsInPriorityOrder: "Cuentas por orden de prioridad",
            membersMustServeTheSameUpstreamServiceTheirMessage:
                "Los miembros deben servir el mismo servicio upstream. Sus propias entradas de prefijo se ocultan mientras están agrupadas. Los modelos nuevos aparecen aquí automáticamente y solo se enrutan a los miembros que los tengan. Se permiten grupos vacíos.",
            moveUp: "Subir",
            moveDown: "Bajar",
            remove: "Quitar",
            addAccount: "Añadir cuenta",
            addMember: "Añadir miembro",
            createPool: "Crear grupo",
        },
        providerQuotaCard: {
            upstreamQuota: "Cuota del upstream",
            allowancesBelongToTheUpstreamAccountAndMayMessage:
                "Las asignaciones pertenecen a la cuenta upstream y pueden incluir uso de otros clientes.",
            thisProviderDefinitionDoesNotDeclareQuotaViewingMessage:
                "La definición de este proveedor no declara la consulta de cuota.",
            checkingWhetherThisDefinitionSupportsQuotaReads:
                "Comprobando si esta definición admite lecturas de cuota…",
            quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage:
                "Cuota no observada. Actualizar consulta explícitamente la cuenta upstream.",
            readingUpstreamQuota: "Leyendo la cuota del upstream…",
            showingThePreviousReadingMessage: "Se muestra la lectura anterior.",
            observed: "Observada",
            resetTimeReachedRefreshToUpdate:
                "Se alcanzó la hora de reinicio; actualiza para ver el estado al día",
            theUpstreamReturnedNoQuotaEntriesMessage: "El upstream no devolvió entradas de cuota.",
        },
        providersScreen: {
            providers: "Proveedores",
            upstreamAccountsOpenOneForItsModelsAndMessage:
                "Cuentas upstream. Abre una para ver sus modelos y su uso; la edición está un paso más adentro.",
            createProvider: "Crear proveedor",
            noProvidersYet: "Aún no hay proveedores",
            aProviderIsOneUpstreamAccountMessage: "Un proveedor es una cuenta upstream.",
            searchProviders: "Buscar proveedores",
            searchByNameTypeOrFormat: "Buscar por nombre, tipo o formato",
        },
        readNotice: {
            readAgain: "Leer de nuevo",
        },
        routingListsScreen: {
            pools: "Grupos",
            namedModelCatalogsBuiltFromInterchangeableAccountsForMessage:
                "Catálogos de modelos con nombre, formados por cuentas intercambiables de un mismo servicio upstream.",
            createPool: "Crear grupo",
            noPoolsYet: "Aún no hay grupos",
            createANamedPoolWithAModelPrefixMessage:
                "Crea un grupo con nombre y un prefijo de modelo. Los modelos de sus miembros se exponen automáticamente.",
            noModelPrefix: "ninguno",
            publicRoutes: "Rutas públicas",
            modelNamesClientsRequestStandaloneProvidersAndPoolsMessage:
                "Nombres de modelo que solicitan los clientes. Los proveedores independientes y los grupos exponen nombres de modelo automáticamente. Los alias adicionales son opcionales.",
            createPublicRoute: "Crear ruta pública",
            noPublicRoutesYet: "Aún no hay rutas públicas",
            publishAPhysicalModelDirectlyOrTargetAMessage:
                "Publica un modelo físico directamente o apunta a un grupo y a un ID upstream exacto.",
            automaticPrefix: "Automático · ",
        },
        screenFrame: {
            readAgain: "Leer de nuevo",
        },
        usageQueryScreen: {
            usageAggregates: "Agregados de uso",
            oneBucketPerDimensionCombinationInsideTheFilterMessage:
                "Un intervalo por combinación de dimensiones dentro del filtro: recuentos y sumas de tokens, nunca contenido.",
            viewRequests: "Ver solicitudes",
            question: "Consulta",
            groupBy: "Agrupar por",
            anyDimensionSetWorksNoneSelectedTotalsEveryMessage:
                "Cualquier conjunto de dimensiones sirve; si no se selecciona ninguna, se totalizan todas las filas coincidentes.",
            window: "Ventana",
            fillsTheBoundsBelowFromOneWindowBackMessage:
                "Rellena los límites de abajo: desde una ventana atrás hasta el extremo abierto, resuelto ahora; los campos siguen siendo editables a mano.",
            from: "Desde",
            inclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "Inclusivo, en la zona horaria de este navegador; por ejemplo, 2026-09-23T10:00. En blanco no fija límite.",
            to: "Hasta",
            exclusiveInThisBrowsersZoneLike20260923T1000BlankMessage:
                "Exclusivo, en la zona horaria de este navegador; por ejemplo, 2026-09-23T10:00. En blanco no fija límite.",
            clearFilters: "Borrar filtros",
            usageBackLabel: "Uso",
            theQueryDidNotRunAFilterValueIsInvalid:
                "La consulta no se ejecutó: el valor de un filtro no es válido.",
            total: "Total",
            nothingRecordedInThisWindowMessage: "No hay nada registrado en esta ventana.",
            noRowsInThisWindowMessage: "No hay filas en esta ventana.",
            bucket: "Intervalo",
            requests: "Solicitudes",
            input: "Entrada",
            output: "Salida",
            cached: "En caché",
            cacheWrite: "Escritura de caché",
            reasoning: "Razonamiento",
            tokenTotalsSkipRowsWhereTheUpstreamReportedMessage:
                "Los totales de tokens omiten las filas en las que el upstream no informó de uso; un desglose ausente nunca infla ni desinfla un total.",
        },
        usageScreen: {
            usage: "Uso",
            everyDispatchedRequestNewestFirstNumbersOnlyNeverMessage:
                "Todas las solicitudes despachadas, de la más reciente a la más antigua: solo números, nunca contenido. Una fila que no informe de ninguna de las cifras de uso opcionales las omite por completo: aquí no se puede distinguir una cola con todo a cero de una no informada.",
            viewAggregates: "Ver agregados",
            filter: "Filtro",
            theSharedAccountingFilterTheSameOneTheMessage:
                "El filtro contable compartido, el mismo que acota la página de agregados.",
            window: "Ventana",
            rollingResolvedWhenTheStreamIsReadMessage: "Móvil, resuelta al leer la transmisión.",
            clearFilters: "Borrar filtros",
            noRequestsRecordedYet: "Aún no hay solicitudes registradas",
            oneRowAppearsPerDispatchedServingRequestTheMessage:
                "Aparece una fila por cada solicitud de servicio despachada, en el momento en que la pasarela la envía al upstream. Si hay tráfico y no aparece nada, revisa la sección api_logs del archivo de configuración de la pasarela.",
            noRecordedRequestsMatchThisFilterMessage:
                "Ninguna solicitud registrada coincide con este filtro.",
            filtersStayBehind: " — la ventana y todos los demás filtros se quedan atrás.",
            streamMark: " · Transmisión",
            thisRowIsNoLongerListed: "Esta fila ya no figura en la lista.",
        },
        usageStats: {
            usage: "Uso",
            requestsAndTokensPerDayNumbersOnlyNeverMessage:
                "Solicitudes y tokens por día: solo números, nunca contenido.",
            openAggregates: "Abrir agregados",
            viewRequests: "Ver solicitudes",
            requests: "Solicitudes",
            dispatchedAllDays: "Despachadas, todos los días",
            inputTokens: "Tokens de entrada",
            summedUsageReported: "Sumados, con uso informado",
            outputTokens: "Tokens de salida",
            day: "Día",
            input: "Entrada",
            output: "Salida",
        },
        usageTable: {
            daysAreUTCTheGatewaysOwnDayBoundaryMessage:
                "Los días son en UTC: el propio límite diario de la pasarela.",
        },
        vanishedModelNotice: {
            theUpstreamNoLongerReportsThisModelMessage: "El upstream ya no informa de este modelo.",
        },
    },
    diagnosis: {
        thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow:
            "Esta cuenta ya aloja exactamente ese modelo upstream. Abre su fila de inventario existente.",
        anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit:
            "Otra ruta pública tiene ese ID público. Elige un nombre diferente y explícito, o edita la ruta existente.",
        anotherPoolHoldsThatNameChooseADifferentPoolName:
            "Otro grupo tiene ese nombre. Elige un nombre de grupo diferente.",
        publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe:
            "Todavía hay rutas públicas que hacen referencia a este grupo. Cámbialas o elimínalas antes de eliminar el grupo.",
        anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne:
            "Otra clave ya tiene ese secreto; genera uno nuevo.",
        theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored:
            "La versión almacenada ha cambiado: usa Actualizar en la barra de arriba; vuelve a leer las versiones almacenadas y deja este formulario intacto, y luego envía de nuevo.",
        chooseAProviderNameNoOtherRowHolds:
            "Elige un nombre de proveedor que no tenga ninguna otra fila.",
        anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis:
            "Otro proveedor tomó este nombre antes de que el flujo pudiera terminar. Empezar de nuevo termina este flujo y libera el nombre; vuelve a crear con un nombre que no tenga ninguna otra fila.",
        chooseAnIdentifierNoOtherRowHolds: "Elige un identificador que no tenga ninguna otra fila.",
        ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe:
            "Si la autorización del proveedor caducó, reconéctala en la página de edición del proveedor; si el upstream realmente aún no informa de ningún modelo, añade uno a mano desde esta página.",
        ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe:
            "Si la URL base o la clave de API del proveedor son incorrectas, corrígelas en la página de edición del proveedor; si el upstream realmente aún no informa de ningún modelo, añade uno a mano desde esta página.",
        checkTheProvidersStateAndCredentials:
            "Comprueba el estado y las credenciales del proveedor.",
        theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage:
            "El proveedor rechazó el intercambio; inicia el flujo de nuevo desde esta página.",
        checkTheProvidersCredentialsAndReachability:
            "Comprueba las credenciales y la accesibilidad del proveedor.",
        nothingAnsweredAtBaseUrl: "Nada respondió en [[baseUrl]].",
        theBrowserCouldNotCompleteTheConnection: "El navegador no pudo completar la conexión.",
        checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs:
            "Comprueba la dirección, que la pasarela esté en ejecución y que su dirección `listen` sea accesible desde este navegador.",
        anHTTPSPageCannotCallTheHTTPAddressBaseUrl:
            "Una página HTTPS no puede llamar a la dirección HTTP [[baseUrl]].",
        thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP:
            "Esta página se sirve por HTTPS y el navegador bloquea por completo sus llamadas a direcciones HTTP, así que nunca se contactó con la pasarela.",
        serveThisAppOverHTTPAsWellOrAddATlsSectionToThat:
            "Sirve esta aplicación también por HTTP, o añade una sección `tls` al archivo de configuración de esa instancia para que la pasarela responda por HTTPS.",
        theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl:
            "La pasarela respondió, pero el navegador bloqueó la respuesta de [[baseUrl]].",
        theBrowserWithheldTheResponseFromThisPage:
            "El navegador retuvo la respuesta destinada a esta página.",
        addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing:
            "Añade [[origin]] a `[[setting]]` en el archivo de configuración de esa instancia y reinicia la pasarela. Servir esta aplicación desde el propio origen de la pasarela evita por completo la lista de permitidos.",
        checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom:
            "Comprueba la dirección, que la pasarela esté en ejecución y que sea accesible desde este navegador con este esquema.",
        noAnswerFromBaseUrl: "Sin respuesta de [[baseUrl]].",
        theBrowserReceivedNoAnswerItCouldRead:
            "El navegador no recibió ninguna respuesta que pudiera leer.",
        eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore:
            "O no hay nada escuchando en esa dirección, o el navegador bloqueó la respuesta antes de que esta página pudiera leerla: añade [[origin]] a `[[setting]]` y reinicia la pasarela.",
        theGatewayRejectedThatManagementToken: "La pasarela rechazó ese token de administración.",
        theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt:
            "El token debe coincidir con `management_api.token` en el archivo de configuración de esa instancia; la pasarela lo lee en vivo, así que un token cambiado no requiere reinicio.",
        theGatewayHoldsNoSuchRow: "La pasarela no tiene esa fila.",
        theGatewayRefusedAnIdentifierIsAlreadyTaken:
            "La pasarela lo rechazó: un identificador ya está en uso.",
        theUpstreamProviderRefusedTheCallOrWasUnreachable:
            "El proveedor upstream rechazó la llamada o no era accesible.",
        theGatewayFailedInternally: "La pasarela falló internamente.",
        theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo:
            "La causa está en el registro stderr de la pasarela; las causas internas nunca se devuelven a esta página.",
        theGatewayRejectedTheRequest: "La pasarela rechazó la solicitud.",
        theGatewayAnsweredWithAShapeThisBuildDoesNotKnow:
            "La pasarela respondió con una forma que esta compilación no conoce.",
        thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr:
            "Los enlaces generados de esta compilación y la API de administración de la pasarela no coinciden; regenera los enlaces o recompila este frontend desde una revisión que coincida.",
        theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody:
            "La pasarela respondió HTTP {{httpStatus}} sin un cuerpo de error de administración.",
        checkTheGatewaysStderrLogForTheCause:
            "Revisa el registro stderr de la pasarela para ver la causa.",
        theCallFailed: "La llamada falló.",
    },
    notices: {
        savedName: "Se guardó [[name]].",
        credentialsReplaced: "Credenciales reemplazadas.",
        expiryReplaced: "El listado del proveedor ahora lleva la nueva caducidad.",
        keyReplaced: "Se reemplazó la clave de API upstream.",
        previousSecretRevoked:
            "El secreto anterior dejó de funcionar con esta acción; actualiza todos los clientes que lo tuvieran.",
        lastKeyDisabled:
            "Era la última clave activada: la pasarela ya no sirve nada hasta que se active otra clave.",
        modelSaved: "Modelo guardado.",
        poolSaved: "Grupo guardado.",
        routeSaved: "Ruta pública guardada.",
        discovered_one: "[[quantity]] modelo descubierto desde [[providerName]].",
        discovered_many: "[[quantity]] modelos descubiertos desde [[providerName]].",
        discovered_other: "[[quantity]] modelos descubiertos desde [[providerName]].",
        nothingDiscovered: "Nada nuevo descubierto desde [[providerName]].",
        manualUntouched_one: "{{quantity}} añadido a mano quedó intacto.",
        manualUntouched_many: "{{quantity}} añadidos a mano quedaron intactos.",
        manualUntouched_other: "{{quantity}} añadidos a mano quedaron intactos.",
        vanished_one: "{{quantity}} modelo figura ahora como ya no informado.",
        vanished_many: "{{quantity}} modelos figuran ahora como ya no informados.",
        vanished_other: "{{quantity}} modelos figuran ahora como ya no informados.",
    },
    connectionValidation: {
        empty: "Introduce la URL base de JanusGate.",
        notAbsolute: "Introduce una URL absoluta; por ejemplo, https://gateway.example.com.",
        scheme: "Usa una URL http:// o https://.",
        credentials: "Quita las credenciales de la URL.",
        notOrigin: "Introduce solo el origen de la pasarela, sin ruta, consulta ni fragmento.",
        managementToken: "Introduce el token de administración.",
        nameEmpty: "El nombre no puede estar vacío.",
    },
    additional: {
        app: {
            readThisScreensStateFromTheGatewayAgain:
                "Volver a leer el estado de esta pantalla desde la pasarela",
            thisScreenReadsNothingFromTheGatewaySo:
                "Esta pantalla no lee nada de la pasarela, así que no hay nada que actualizar",
            disconnectAndForgetTheStoredTokenReconnectingMeansMessage:
                "¿Desconectar y olvidar el token almacenado? Volver a conectar implica introducirlo de nuevo.",
        },
        apiKeyAccess: {
            allow: "Permitir",
            deny: "Denegar",
            provider: "Proveedor",
            pool: "Grupo",
            automatic: "Automático",
            alias: "Alias",
        },
        apiKeyForm: {
            whichClientHoldsThisKeyMessage: "Obligatorio. Qué cliente posee esta clave.",
            aServingClientAuthenticatesWithItOnEveryMessage:
                "Obligatorio. Un cliente de servicio se autentica con ella en todas las interfaces.",
        },
        clipboard: {
            copyToClipboard: "Copiar al portapapeles",
            copied: "Copiado",
            copy: "Copiar",
            theValueHere: "el valor que figura aquí",
        },
        confirmDeleteCard: {
            deleting: "Eliminando…",
            deleteForGood: "Eliminar definitivamente",
        },
        dirtyFormFooter: {
            saving: "Guardando…",
            saveChanges: "Guardar cambios",
        },
        layout: {
            back: "Volver",
        },
        modelForm: {
            followingDiscoveryCurrentlyUnknownSoListingsOmitItMessage:
                "Sigue el descubrimiento; actualmente se desconoce, así que los listados lo omiten.",
            followingDiscoveryCurrentlyNothingIsKnownSoTheMessage:
                "Sigue el descubrimiento; actualmente no se sabe nada, así que decide la declaración de la definición.",
        },
        providerForm: {
            verifiedLive: "Verificado en vivo",
            unverified: "Sin verificar",
            theUpstreamAddressThisDefinitionsRequestsPointAtMessage:
                "Obligatorio. La dirección upstream a la que apuntan las solicitudes de esta definición.",
            upstreamAPIKey: "Clave de API upstream",
            sentUpstreamWithEveryRequestNeverReturnedByMessage:
                "Obligatorio. Se envía al upstream con cada solicitud. La pasarela nunca la devuelve.",
            showTheAPIKeyWhileTyping: "Mostrar la clave de API mientras se escribe",
        },
        providersTable: {
            on: "Activado",
            off: "Desactivado",
        },
        usageFilterControls: {
            allEndpoints: "Todos los puntos de conexión",
        },
        apiKeyDetailScreen: {
            apiKeys: "Claves de API",
            apiKey: "Clave de API",
            createAPIKey: "Crear clave de API",
            enabled: "Activado",
            disabled: "Desactivado",
        },
        apiKeyEditScreen: {
            apiKey: "Clave de API",
            editAPIKey: "Editar clave de API",
            showTheCurrentKey: "Mostrar la clave actual",
        },
        apiKeyNewScreen: {
            creating: "Creando…",
            createAPIKey: "Crear clave de API",
        },
        apiKeysScreen: {
            on: "Activado",
            off: "Desactivado",
        },
        bulkPublicationScreen: {
            publicIDIsRequiredMessage: "El ID público es obligatorio.",
            noDirectRoutes: "Sin rutas directas",
            modelVanishedItsNewRouteWillBeSuspended:
                "El modelo ya no se informa; su nueva ruta se suspenderá",
            modelEnabled: "Modelo activado",
            modelDisabled: "Modelo desactivado",
        },
        connectionScreen: {
            showTheTokenWhileTyping: "Mostrar el token mientras se escribe",
            probingTheGateway: "Sondeando la pasarela…",
            connect: "Conectar",
        },
        homeScreen: {
            inspectPublicRoutesForQuotaAvailabilityTheseFiguresMessage:
                "Inspecciona las rutas públicas para ver la disponibilidad de cuota. Estas cifras describen los interruptores y el inventario almacenados.",
            noRequestsRecordedMessage: "No hay solicitudes registradas.",
        },
        modelDetailScreen: {
            models: "Modelos",
            model: "Modelo",
            overriddenByAdministratorMessage: "Anulado por el administrador.",
            fromTheLatestDiscoveryMessage: "Del último descubrimiento.",
            unknownOmittedFromModelListingsMessage:
                "Desconocido: se omite en los listados de modelos.",
            noFactOnThisRowTheDefinitionsDeclarationMessage:
                "Sin dato en esta fila; decide la declaración de la definición.",
            unknown: "Desconocido",
            noAutomaticEntry: "Sin entrada automática",
            enabled: "Activado",
            disabled: "Desactivado",
        },
        modelEditScreen: {
            model: "Modelo",
            editModel: "Editar modelo",
        },
        modelNewScreen: {
            aModelBelongsToOneProviderMessage:
                "Obligatorio. Un modelo pertenece a un solo proveedor.",
            sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage:
                "Obligatorio. Se envía al upstream tal cual: solo letras, números y símbolos.",
            carriedHereFromAModelTheUpstreamNoMessage:
                "Obligatorio. Arrastrado hasta aquí desde un modelo del que el upstream ya no informa; al enviarlo lo reclamas con los ajustes de abajo.",
            creating: "Creando…",
            createModel: "Crear modelo",
        },
        modelRouteEditorScreen: {
            createPublicRoute: "Crear ruta pública",
            editPublicRoute: "Editar ruta pública",
            publicRoutes: "Rutas públicas",
            aModelNameMaintainedAutomaticallyByItsProviderMessage:
                "Un nombre de modelo mantenido automáticamente por su proveedor o grupo.",
            clientsRequestThisPublicIDChangingItsTargetMessage:
                "Los clientes solicitan este ID público. Cambiar su destino deja ese nombre intacto.",
            openPool: "Abrir grupo",
            openPhysicalModel: "Abrir modelo físico",
            deleteThisPublicRoute: "¿Eliminar esta ruta pública?",
            publicIDIsRequiredMessage: "El ID público es obligatorio.",
            publicID: "ID público",
            directPhysicalModel: "Modelo físico directo",
            pool: "Grupo",
            chooseAPool: "Elegir un grupo",
            upstreamIDIsRequiredMessage: "El ID upstream es obligatorio.",
            upstreamID: "ID upstream",
            chooseAPhysicalModel: "Elegir un modelo físico",
        },
        modelsScreen: {
            noModelsOnAnyProviderYetDiscoverOrMessage:
                "Aún no hay modelos en ningún proveedor; descúbrelos o añade uno.",
        },
        poolAvailabilityCard: {
            readingStoredAvailability: "Leyendo la disponibilidad almacenada…",
            quotaNotObserved: "Cuota no observada",
            quotaReadsUnsupported: "Lecturas de cuota no compatibles",
            administratorResetRefreshGraceInstalled:
                "Restablecido por el administrador; prórroga de actualización instalada",
            accountEnabled: "Cuenta activada",
            updatingAvailability: "Actualizando la disponibilidad…",
            allModels: "Todos los modelos",
            resetUnknown: "Reinicio desconocido",
            resetTimePassedRefreshRequired: "Pasó la hora de reinicio; se requiere actualizar",
        },
        providerDetailScreen: {
            providers: "Proveedores",
            provider: "Proveedor",
            createProvider: "Crear proveedor",
            enabled: "Activado",
            disabled: "Desactivado",
            theOAuthCredentialsHaveExpiredMessage: "Las credenciales OAuth han caducado.",
            noneExactUpstreamIDs: "Ninguno; ID upstream exactos",
            discovering: "Descubriendo…",
            discoverModels: "Descubrir modelos",
            askingTheUpstreamWhatItReports: "Preguntando al upstream qué informa…",
            noModelsOnThisProviderYetDiscoverWhatMessage:
                "Aún no hay modelos en este proveedor; descubre qué informa el proveedor o añade uno a mano.",
        },
        providerEditScreen: {
            provider: "Proveedor",
            editProvider: "Editar proveedor",
            modelPrefix: "Prefijo de modelo",
            aProviderNeedsANameMessage: "Un proveedor necesita un nombre.",
            starting: "Iniciando…",
            reconnectTheAccount: "Reconectar la cuenta",
            replacing: "Reemplazando…",
            replaceTheCredentials: "Reemplazar las credenciales",
            deletingRemovesTheAccountAndItsPoolMembershipsMessage:
                "Eliminar quita la cuenta y sus pertenencias a grupos. No tiene modelos físicos.",
        },
        providerNewScreen: {
            modelPrefix: "Prefijo de modelo",
            creating: "Creando…",
            createProvider: "Crear proveedor",
            connecting: "Conectando…",
            connectTheAccount: "Conectar la cuenta",
            theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage:
                "Obligatorio. La dirección upstream contra la que se evalúan las solicitudes de esta definición; sin ella, la pasarela rechaza el inicio de un flujo.",
            starting: "Iniciando…",
            startTheAuthorization: "Iniciar la autorización",
        },
        providerPoolEditorScreen: {
            createPool: "Crear grupo",
            editPool: "Editar grupo",
            pools: "Grupos",
            deleteThisPoolItsMembershipIsRemovedMessage:
                "¿Eliminar este grupo? Su pertenencia se elimina.",
            nameIsRequiredMessage: "El nombre es obligatorio.",
            modelPrefix: "Prefijo de modelo",
            chooseAnAccount: "Elegir una cuenta",
        },
        providerQuotaCard: {
            periodUnknown: "Periodo desconocido",
            monthly: "Mensual",
            unknown: "Desconocido",
            unlimited: "Ilimitado",
            resetUnknown: "Reinicio desconocido",
            readingQuota: "Leyendo la cuota…",
            refreshingQuota: "Actualizando la cuota…",
            refreshQuota: "Actualizar cuota",
            retryQuota: "Reintentar cuota",
        },
        routingListsScreen: {
            on: "Activado",
            off: "Desactivado",
        },
        usageQueryScreen: {
            querying: "Consultando…",
            runQuery: "Ejecutar consulta",
            noFilterOrBoundNarrowsTheQuestionSoMessage:
                "Ningún filtro ni límite acota la consulta, así que una respuesta vacía es el registro completo de la pasarela: aún no contiene uso.",
            nothingRecordedMatchesTheseFiltersWidenTheBoundsMessage:
                "Nada registrado coincide con estos filtros; amplía los límites, quita un filtro o consulta sin ninguno. Borrar filtros está en el formulario de arriba.",
        },
        usageScreen: {
            reading: "Leyendo…",
            applyFilters: "Aplicar filtros",
            widenTheWindowDropAFilterOrReadMessage:
                "Amplía la ventana, quita un filtro o lee toda la transmisión; Borrar filtros está en el formulario de arriba.",
            theGatewaysFilterLocatesRowsItNeverValidatesMessage:
                "El filtro de la pasarela localiza filas; nunca valida un valor contra ellas, así que un id desconocido o un estado nunca registrado simplemente no coincide con nada.",
            thatIsEveryRowThisFilterMatchesMessage:
                "Esas son todas las filas que coinciden con este filtro.",
            thatIsEveryRecordedRowMessage: "Esas son todas las filas registradas.",
            aReadCycleIsReplacingTheStreamExtendMessage:
                "Un ciclo de lectura está reemplazando la transmisión; amplíala cuando termine el ciclo.",
            loading: "Cargando…",
            loadMore: "Cargar más",
        },
    },
    operationNotices: {
        flowFailed: "El flujo falló.",
        freshFlow: "Inicia un flujo nuevo y autorízalo sin demora.",
        flowExpired: "El flujo caducó.",
        flowExpiryPassed: "La caducidad del flujo pasó mientras esta página esperaba el resultado.",
        pollStopped:
            "La consulta del resultado seguía fallando, así que esta página dejó de preguntar.",
        pollFailed: "La última consulta del resultado falló.",
        invalidDefinition:
            "El JSON de la definición no es una definición que la pasarela pudiera almacenar.",
        fixDefinition: "Corrige el JSON y envía de nuevo; no se escribió nada.",
        fixFilter: "Corrige ese campo y ejecuta la consulta de nuevo.",
        discoveryPending: "Descubriendo modelos desde [[providerName]]…",
        discoveryQuery: "Preguntando al upstream qué informa ahora mismo.",
    },
    counts: {
        models_one: "{{total}} modelo",
        models_many: "{{total}} modelos",
        models_other: "{{total}} modelos",
        filteredModels_one: "{{shown}} de {{total}} modelo",
        filteredModels_many: "{{shown}} de {{total}} modelos",
        filteredModels_other: "{{shown}} de {{total}} modelos",
        enabledKeys_one: "{{enabled}} de {{total}} clave activada",
        enabledKeys_many: "{{enabled}} de {{total}} claves activadas",
        enabledKeys_other: "{{enabled}} de {{total}} claves activadas",
        shownKeys_one: "{{shown}} clave mostrada",
        shownKeys_many: "{{shown}} claves mostradas",
        shownKeys_other: "{{shown}} claves mostradas",
        filteredKeys_one: "[[shownSentence]] · [[enabled]] de [[total]] clave activada",
        filteredKeys_many: "[[shownSentence]] · [[enabled]] de [[total]] claves activadas",
        filteredKeys_other: "[[shownSentence]] · [[enabled]] de [[total]] claves activadas",
        providerDeleted_one:
            "Eliminar quita la cuenta, sus pertenencias a grupos y su {{total}} modelo físico con sus rutas directas. Las rutas de grupo se conservan almacenadas.",
        providerDeleted_many:
            "Eliminar quita la cuenta, sus pertenencias a grupos y sus {{total}} modelos físicos con sus rutas directas. Las rutas de grupo se conservan almacenadas.",
        providerDeleted_other:
            "Eliminar quita la cuenta, sus pertenencias a grupos y sus {{total}} modelos físicos con sus rutas directas. Las rutas de grupo se conservan almacenadas.",
        requests_one: "{{total}} solicitud",
        requests_many: "{{total}} solicitudes",
        requests_other: "{{total}} solicitudes",
        requestsPeak_one: "[[total]] solicitud · pico [[peak]] el [[day]]",
        requestsPeak_many: "[[total]] solicitudes · pico [[peak]] el [[day]]",
        requestsPeak_other: "[[total]] solicitudes · pico [[peak]] el [[day]]",
    },
    helpers: {
        accessControl: {
            allowed: "Permitido",
            denied: "Denegado",
            modelOverride: "Anulación de modelo",
            globallyDisabled: "Desactivado globalmente",
            targetUnavailable: "Destino no disponible",
            targetVanished: "Destino ya no informado",
            globallyAvailable: "Disponible globalmente",
            noAvailablePoolMembers: "Sin miembros de grupo disponibles",
        },
        identifiers: {
            baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces:
                "La URL base debe ser una dirección http:// o https:// sin espacios.",
        },
        presets: {
            anyEndpointSpeakingTheOpenAIResponsesProtocolWithAStaticKey:
                "Cualquier punto de conexión que hable el protocolo OpenAI Responses con una clave estática.",
            anyEndpointSpeakingTheChatCompletionsProtocolWithAStaticKey:
                "Cualquier punto de conexión que hable el protocolo Chat Completions con una clave estática.",
            anyEndpointSpeakingTheGeminiGenerateContentProtocolWithAStaticKey:
                "Cualquier punto de conexión que hable el protocolo Gemini GenerateContent con una clave estática.",
            anyEndpointSpeakingTheGeminiInteractionsProtocolWithAStaticKey:
                "Cualquier punto de conexión que hable el protocolo Gemini Interactions con una clave estática.",
            anyEndpointSpeakingTheAnthropicMessagesProtocolWithAStaticKey:
                "Cualquier punto de conexión que hable el protocolo Anthropic Messages con una clave estática.",
            theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact:
                "El punto de conexión xAI Responses con una clave estática: la compactación nativa usa la operación independiente `/responses/compact`, mientras que el desencadenante explícito y la compactación automática se declaran no compatibles; un punto de conexión de compactación independiente no implica desencadenante nativo ni compactación automática.",
            claudeCodeClaudeAccount: "Claude Code (cuenta de Claude)",
            aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer:
                "Una cuenta de suscripción de Claude sobre la API Messages: el token OAuth viaja en `Authorization: Bearer` junto a la cabecera beta `oauth-2025-04-20`, tanto en la inferencia como en el descubrimiento de modelos.",
            kimiCodeKimiAccount: "Kimi Code (cuenta de Kimi)",
            githubCopilotGitHubAccount: "GitHub Copilot (cuenta de GitHub)",
            devinDevinAccount: "Devin (cuenta de Devin)",
            aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI:
                "Una cuenta de Devin cuyo intercambio de token ConnectRPC devuelve una clave de API estática y la URL del servidor de API: sin renovación ni caducidad; los modelos se añaden a mano.",
            codexChatGPTAccount: "Codex (cuenta de ChatGPT)",
            aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses:
                "Una cuenta de ChatGPT sobre el punto de conexión Codex Responses. La transmisión se fuerza hacia upstream y las respuestas se representan con el formato OpenAI Responses. La sección de compactación declara el desencadenante explícito y la compactación automática por umbral que acepta el backend de Codex.",
            antigravityGoogleAccount: "Antigravity (cuenta de Google)",
            aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped:
                "Una cuenta de Google sobre el punto de conexión Antigravity Gemini, con incorporación de proyecto y una solicitud GenerateContent envuelta.",
            zenOverTheOpenAIChatCompletionsProtocolWithAStaticKey:
                "Zen sobre el protocolo OpenAI Chat Completions con una clave estática.",
            zenOverTheOpenAIResponsesProtocolWithAStaticKey:
                "Zen sobre el protocolo OpenAI Responses con una clave estática.",
            zenOverTheAnthropicMessagesProtocolWithAStaticKey:
                "Zen sobre el protocolo Anthropic Messages con una clave estática.",
            zenOverTheGeminiGenerateContentProtocolWithAStaticKey:
                "Zen sobre el protocolo Gemini GenerateContent con una clave estática.",
        },
        oauthFlow: {
            thatPasteHoldsNeitherABareCodeNorAURLWithACodeCopy:
                "Eso que has pegado no contiene ni un código suelto ni una URL con un código; copia la dirección completa en la que terminó el inicio de sesión, o solo su código, y pégala aquí.",
            theFlowStaysLiveThisPageKeepsAskingForTheOutcome:
                "El flujo sigue activo; esta página sigue preguntando por el resultado.",
        },
        rowNotFound: {
            theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped:
                "La pasarela no tiene ningún grupo aquí. Se eliminó, o la dirección se escribió mal.",
            theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas:
                "La pasarela no tiene ninguna ruta pública aquí. Se eliminó, o la dirección se escribió mal.",
            theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas:
                "La pasarela no tiene ninguna clave de API aquí. Se eliminó, o la dirección se escribió mal. Nada de aquí puede recuperarla.",
            theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped:
                "La pasarela no tiene ningún modelo aquí. Se eliminó, o la dirección se escribió mal; un modelo descubierto y eliminado vuelve como nuevo en el siguiente descubrimiento.",
            theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped:
                "La pasarela no tiene ningún proveedor aquí. Se eliminó, o la dirección se escribió mal. Nada de aquí puede recuperarlo.",
        },
        usageLabels: {
            streamed: "Con transmisión",
            nonstreamed: "Sin transmisión",
        },
        apiKeyAccess: {
            inherit: "Heredar",
            allow: "Permitir",
            deny: "Denegar",
        },
        modelForm: {
            followingDiscoveryCurrentlyDeclaresNoLevels:
                "Sigue el descubrimiento; actualmente no declara ningún nivel.",
            replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels:
                "Reemplaza el conjunto descubierto; una selección vacía no declara ningún nivel.",
            followingDiscoveryCurrentlyUnknownSoListingsOmitIt:
                "Sigue el descubrimiento; actualmente se desconoce, así que los listados lo omiten.",
            replacesTheDiscoveredValueWithAPositiveInteger:
                "Reemplaza el valor descubierto por un número entero positivo.",
            unset: "Sin definir",
            discoveredNothingKnown: "Descubierto: no se sabe nada.",
        },
        modelsTable: {
            ready: "Listo",
            off: "Desactivado",
            suspended: "Suspendido",
            heldByProvider: "Retenido por el proveedor",
            unknown: "Desconocido",
            noAutomaticEntry: "Sin entrada automática",
            noDirectRoutes: "Sin rutas directas",
        },
        providersTable: {
            valid: "Válida",
            expired: "Caducada",
            stale: "Obsoleta",
        },
        usageFilterControls: {
            allOutcomes: "Todos los resultados",
            failuresUpstreamOrInternal: "Fallos (upstream o internos)",
            any: "Cualquiera",
            streamed: "Con transmisión",
            nonstreamed: "Sin transmisión",
        },
        usageFilter: {
            allTime: "Todo el tiempo",
            last24Hours: "Últimas 24 horas",
            last7Days: "Últimos 7 días",
            last30Days: "Últimos 30 días",
            invalidStatusList:
                "Los estados HTTP deben ser enteros sencillos separados por comas, como 200, 429, o quedar en blanco para no filtrar por estado.",
        },
    },
    validation: {
        composition: {
            upstreamId:
                "El ID upstream solo admite letras, números y símbolos: sin espacios ni texto no ASCII.",
            publicId:
                "El ID público solo admite letras, números y símbolos: sin espacios ni texto no ASCII.",
            modelPrefix:
                "El prefijo de modelo solo admite letras, números y símbolos: sin espacios ni texto no ASCII.",
            apiKey: "La clave de API solo admite letras, números y símbolos: sin espacios ni texto no ASCII.",
            key: "La clave solo admite letras, números y símbolos: sin espacios ni texto no ASCII.",
        },
        routing: {
            upstreamId:
                'El ID upstream solo admite letras, números y símbolos: sin espacios, texto no ASCII, "/" ni ":".',
            publicId:
                'El ID público solo admite letras, números y símbolos: sin espacios, texto no ASCII, "/" ni ":".',
            modelPrefix:
                'El prefijo de modelo solo admite letras, números y símbolos: sin espacios, texto no ASCII, "/" ni ":".',
            apiKey: 'La clave de API solo admite letras, números y símbolos: sin espacios, texto no ASCII, "/" ni ":".',
            key: 'La clave solo admite letras, números y símbolos: sin espacios, texto no ASCII, "/" ni ":".',
        },
        positive: {
            maxInputOverride:
                "La anulación del máximo de tokens de entrada debe ser un número entero positivo.",
            maxOutputOverride:
                "La anulación del máximo de tokens de salida debe ser un número entero positivo.",
        },
    },
    quotaMessages: {
        day_one: "{{quantity}} día",
        day_many: "{{quantity}} días",
        day_other: "{{quantity}} días",
        hour_one: "{{quantity}} hora",
        hour_many: "{{quantity}} horas",
        hour_other: "{{quantity}} horas",
        minute_one: "{{quantity}} minuto",
        minute_many: "{{quantity}} minutos",
        minute_other: "{{quantity}} minutos",
        second_one: "{{quantity}} segundo",
        second_many: "{{quantity}} segundos",
        second_other: "{{quantity}} segundos",
        used: "{{percentage}} usado",
        counter: "[[used]] / [[limit]] [[unit]] usados",
        remaining: "[[remaining]] [[unit]] restantes",
        dateReset: "[[date]] · Se desconoce la hora exacta de reinicio",
        atReset: "Se reinicia [[instant]] · [[distance]]",
        future: "dentro de {{duration}}",
        observed: "Observado [[instant]] · [[distance]]",
        inventory: {
            pool_disabled: "Grupo desactivado",
            account_missing: "Cuenta ausente",
            account_disabled: "Cuenta desactivada",
            model_not_selected:
                "Elige un ID upstream para inspeccionar la disponibilidad del modelo",
            model_missing: "Modelo ausente",
            model_disabled: "Modelo desactivado",
            model_vanished: "Modelo ya no informado",
            ready: "Inventario listo",
        },
        failure: {
            credential_renewal: "falló la renovación de credenciales",
            timeout: "tiempo de espera agotado",
            transport: "fallo de transporte",
            upstream_rejected: "el upstream rechazó la solicitud",
            malformed_observation: "observación mal formada",
        },
        quotaObserved: "Cuota observada [[instant]]",
        quotaFailed: "Falló la lectura de la cuota · {{reason}}",
        deletedAccount: "Cuenta eliminada n.º {{id}}",
        priority: "Prioridad {{priority}}",
        models: "Modelos: [[identifiers]]",
        resets: "Se reinicia [[instant]]",
    },
    dynamicMessages: {
        following: "Sigue el descubrimiento: {{value}}.",
        levelsReplace: "Reemplaza el conjunto descubierto: los clientes ven {{value}}.",
        capReplace: "Reemplaza el valor descubierto: los clientes ven {{value}}.",
        mechanismFact: "{{mechanism}}: {{support}}",
        mechanismDiscovered: "Descubierto: {{support}}.",
        compactionReplace:
            "Reemplaza por completo los datos descubiertos: un mecanismo sin definir recurre al descubrimiento y, después, a la declaración de la definición.",
        noExpiry: "No hay caducidad almacenada para este proveedor.",
        goneFlow:
            "Este flujo de autorización ya no se puede completar: su state caducó o ya se usó.",
        refusedClipboard:
            "Este navegador rechazó el portapapeles: selecciona {{subject}} y cópialo a mano.",
        enabledStored: "activados de {{total}} almacenados",
        readyListed: "listos de {{total}} en la lista",
        copyEndpoint: "Copiar la URL del punto de conexión de {{surface}}",
        disabledCredential:
            "{{expiry}} El proveedor está desactivado; reconéctalo cuando lo vuelvas a activar.",
        noKeysDetail:
            " Los clientes de servicio se autentican con una; sin ella, la pasarela no tiene inventario listo.",
    },
    remainingText: {
        notRefreshedYet: "Aún sin actualizar",
        overrideMaxInputTokens: "Anular el máximo de tokens de entrada",
        maxInputTokensOverride: "Anulación del máximo de tokens de entrada",
        overrideMaxOutputTokens: "Anular el máximo de tokens de salida",
        maxOutputTokensOverride: "Anulación del máximo de tokens de salida",
        chooseAProvider: "Elegir un proveedor",
        namingAProviderAndUpstreamIDTheUpstreamNoLonger:
            "Indicar un proveedor y un ID upstream de los que el upstream ya no informa reclama ese modelo con los ajustes de abajo. El modelo se expone automáticamente mediante el prefijo de su proveedor independiente o los prefijos de su grupo propietario.",
        allProviders: "Todos los proveedores",
        replaceTheUpstreamAPIKey: "Reemplazar la clave de API upstream",
        writeOnlyTheGatewayNeverReturnsItSoLeavingThis:
            "Solo escritura: la pasarela nunca la devuelve, así que dejar esto vacío conserva lo almacenado.",
        backToPools: "Volver a los grupos",
        publicRoute: "Ruta pública",
        publicRoutes: "Rutas públicas",
        backToPublicRoutes: "Volver a las rutas públicas",
        backToAPIKeys: "Volver a las claves de API",
        backToModels: "Volver a los modelos",
        backToProviders: "Volver a los proveedores",
        deletedRoute: "Ruta eliminada",
        deletedModel: "Modelo eliminado",
        deletedProvider: "Proveedor eliminado",
        deletedKey: "Clave eliminada",
        theAnswerBelowIsStillThePreviousQuerySThe:
            "La respuesta de abajo sigue siendo la de la consulta anterior; el campo que muestra un error en línea indica el problema.",
        fromAndToDoNotApplyThere: " — Desde y Hasta no se aplican ahí.",
        oneRowAppearsPerDispatchedServingRequestTheMomentThe:
            "Aparece una fila por cada solicitud de servicio despachada, en el momento en que la pasarela la envía al upstream.",
        showTheKeyWhileTyping: "Mostrar la clave mientras se escribe",
        saveWritesYourEditsOverTheGatewaySVersionRevert:
            "Guardar escribe tus cambios sobre la versión de la pasarela; Revertir adopta la versión de la pasarela.",
        theOAuthCredentialsWereNeverFresh: "Las credenciales OAuth nunca estuvieron vigentes.",
        allPublicRoutes: "Todas las rutas públicas",
        allPhysicalModels: "Todos los modelos físicos",
        allAPIKeys: "Todas las claves de API",
    },
    entityMessages: {
        editName: "Editar [[name]]",
        editIdentifier: "Editar [[identifier]]",
        deleteKey: "¿Eliminar [[name]]? Los clientes que tengan este secreto perderán el acceso.",
        deleteLastKey:
            "¿Eliminar [[name]]? Es la última clave activada: la pasarela no servirá nada hasta que se active otra clave.",
        deleteModel:
            "¿Eliminar [[identifier]]? Sus rutas directas también se eliminan. Las rutas de grupo se conservan almacenadas. Un nuevo descubrimiento o una creación manual produce una nueva fila física y no restaura las rutas eliminadas.",
        deleteProvider: "¿Eliminar [[name]]?",
        switchPreset: "¿Cambiar a [[name]]? Tu receta de conexión editada se reemplazará.",
        noKeysMatch: 'Ninguna clave de API coincide con "[[query]]".',
        noProvidersMatch: 'Ningún proveedor coincide con "[[query]]".',
        noModelsMatch: 'Ningún modelo coincide con "[[query]]".',
        noProviderModelsMatch: 'Ningún modelo de este proveedor coincide con "[[query]]".',
        missingPath:
            "No hay nada en [[path]]. Puede que se haya eliminado o que la dirección se escribiera mal.",
        aliasCreated: "Alias creado: [[identifier]]",
        addAlias: "Añadir alias para [[identifier]]",
    },
    modelCard: {
        heading: "Modelos · {{total}}",
    },
    domainMessages: {
        hold: {
            key: "Esta clave cambió en la pasarela mientras tienes cambios sin guardar.",
            provider: "Este proveedor cambió en la pasarela mientras tienes cambios sin guardar.",
            model: "Este modelo cambió en la pasarela mientras tienes cambios sin guardar.",
            pool: "Este grupo cambió en la pasarela mientras tienes cambios sin guardar.",
            "public route":
                "Esta ruta pública cambió en la pasarela mientras tienes cambios sin guardar.",
            "api key":
                "Esta clave de API cambió en la pasarela mientras tienes cambios sin guardar.",
        },
        noRequests: {
            key: "Aún no hay solicitudes registradas para esta clave.",
            provider: "Aún no hay solicitudes registradas para este proveedor.",
            model: "Aún no hay solicitudes registradas para este modelo.",
            pool: "Aún no hay solicitudes registradas para este grupo.",
            "public route": "Aún no hay solicitudes registradas para esta ruta pública.",
            "api key": "Aún no hay solicitudes registradas para esta clave de API.",
        },
        notFound: {
            api_key: "Clave de API no encontrada",
            provider: "Proveedor no encontrado",
            model: "Modelo no encontrado",
            pool: "Grupo no encontrado",
            model_route: "Ruta pública no encontrada",
        },
        replaceSecret: {
            typed: "¿Reemplazar el secreto escrito?",
            current: "¿Reemplazar el secreto actual?",
        },
        bound: {
            From: "Desde debe ser una fecha y hora completas, como 2026-09-23T10:00, o quedar en blanco para no fijar límite.",
            To: "Hasta debe ser una fecha y hora completas, como 2026-09-23T10:00, o quedar en blanco para no fijar límite.",
        },
        updated: "Actualizado {{instant}}",
        created: "Creado {{instant}}",
        version: "Almacenado como versión {{version}}.",
        unlisted: "{{value}} (ya no figura en la lista)",
        accountUnlisted: "Cuenta n.º {{id}} (ya no figura en la lista)",
        moveUp: "Subir {{name}}",
        moveDown: "Bajar {{name}}",
        remove: "Quitar {{name}}",
        publicIdFor: "ID público de {{subject}}",
        source: {
            key: "de la clave",
            provider: "del proveedor",
            pool: "del grupo",
            route: "anulación de modelo",
        },
        deleted: {
            provider: "Proveedor eliminado n.º {{id}}",
            pool: "Grupo eliminado n.º {{id}}",
            route: "Ruta eliminada n.º {{id}}",
        },
        status: "Cliente {{client}} · Upstream {{upstream}}",
        metrics: " · {{duration}} · entrada {{input}} · salida {{output}}{{tail}}",
        optionalTokens:
            " · en caché {{cached}} · escritura de caché {{write}} · razonamiento {{reasoning}}",
    },
    routingMessages: {
        directRoutes_one: "{{quantity}} ruta directa",
        directRoutes_many: "{{quantity}} rutas directas",
        directRoutes_other: "{{quantity}} rutas directas",
        poolMembers_one: "[[quantity]] cuenta miembro · Prefijo de modelo: [[prefix]]",
        poolMembers_many: "[[quantity]] cuentas miembro · Prefijo de modelo: [[prefix]]",
        poolMembers_other: "[[quantity]] cuentas miembro · Prefijo de modelo: [[prefix]]",
        directMissing: "Directa · Modelo físico n.º [[id]]",
        direct: "Directa · [[provider]] · [[model]]",
        pool: "Grupo · [[pool]] · [[model]]",
        publicIds: "ID público [[identifiers]]",
        modelAccess: "Acceso a modelos de [[identifier]]",
        ownerDefault: {
            provider: "Valor predeterminado del proveedor [[name]]",
            pool: "Valor predeterminado del grupo [[name]]",
        },
        directTarget: "Directa · modelo n.º [[id]]",
        poolTarget: "Grupo · [[identifier]]",
        reconnect: "Reconéctalo en la [[editLink]]; su tarjeta Credenciales las reemplaza.",
        reconnectDisabled:
            "El proveedor está desactivado; reconéctalo cuando lo vuelvas a activar; la tarjeta Credenciales de la [[editLink]] las reemplaza.",
    },
    linkedMessages: {
        keyEndpoint:
            "Apunta tu cliente a un punto de conexión: todas las interfaces sirven las entradas públicas permitidas; las URL están en la [[overviewLink]].",
        servingKey:
            "Un cliente de servicio se autentica con una clave de API de servicio; crea una en [[keysLink]] antes de apuntar nada aquí.",
        vanished:
            "Mientras tanto, no sirve nada. [[discoverLink]] cuando el upstream lo incluya, o [[claimLink]].",
        discoverLabel: "Volver a descubrir en la página del proveedor",
        claimLabel: "reclamarlo desde la página Crear modelo",
        selected: "{{quantity}} seleccionados. Las filas correctas no se enviarán de nuevo.",
        kimiDescription:
            "Una cuenta de Kimi sobre el punto de conexión Coding: un flujo de código de dispositivo cuyo hueco de ID de dispositivo viaja en las cabeceras personalizadas `X-Msh-*` tanto en la autorización como en el sondeo y la renovación.",
        githubDescription:
            "Una cuenta de GitHub cuyo token duradero se intercambia reaprovisionando para obtener el token de sesión de Copilot de corta duración y el punto de conexión de API de la cuenta.",
        chartPoint_one: "[[day]]: [[quantity]] solicitud",
        chartPoint_many: "[[day]]: [[quantity]] solicitudes",
        chartPoint_other: "[[day]]: [[quantity]] solicitudes",
        inventoryEmpty: "[[name]] aún no tiene modelos.",
        inventoryUnready:
            "[[name]] no tiene inventario listo: todos los modelos están desactivados o ya no se informan.",
    },
    vocabulary: {
        credentialKind: {
            api_key: "Clave de API estática",
            oauth: "Cuenta OAuth",
        },
        upstreamFormat: {
            openai_response: "OpenAI Responses",
            openai_chat_completion: "OpenAI Chat Completions",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interaction: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        compactionMechanism: {
            responses_standalone_compact: "Compactación independiente",
            responses_explicit_trigger: "Desencadenante explícito",
            responses_automatic_compaction: "Compactación automática",
            messages_on_demand_compaction: "Compactación a demanda",
            messages_threshold_compaction: "Compactación por umbral",
        },
        mechanismSupport: {
            supported: "Compatible",
            unsupported: "No compatible",
            unknown: "Desconocido",
        },
        discoveryState: {
            discovered: "Descubierto",
            vanished: "Ya no informado",
            manual: "Manual",
        },
        reasoningLevel: {
            none: "ninguno",
            minimal: "mínimo",
            low: "bajo",
            medium: "medio",
            high: "alto",
            xhigh: "muy alto",
            max: "máximo",
        },
        apiLogOutcome: {
            completed: "Completada",
            client_aborted: "Cancelada por el cliente",
            upstream_error: "Error del upstream",
            internal: "Interno",
        },
        apiLogEndpointType: {
            openai_chat_completions: "Chat Completions",
            openai_responses: "Responses",
            openai_responses_compact: "Responses compacto",
            gemini_generate_content: "Gemini GenerateContent",
            gemini_interactions: "Gemini Interactions",
            anthropic_messages: "Anthropic Messages",
        },
        usageGroupBy: {
            day: "Día",
            model: "Modelo físico",
            model_route: "Ruta pública",
            provider: "Proveedor",
            api_key: "Clave de API",
            endpoint_type: "Punto de conexión",
            outcome: "Resultado",
            is_stream: "Modo de transmisión",
            http_status: "Estado HTTP",
        },
    },
    common: {
        save: "Guardar cambios",
        cancel: "Cancelar",
        retry: "Reintentar",
        language: "Idioma",
    },
    language: {
        system: "Seguir el navegador",
        loading: "Cargando {{languageName}}…",
        failed: "No se pudo cargar {{languageName}}. Tu idioma actual sigue mostrándose.",
        startupFailed: "No se pudo cargar {{languageName}}. Actualmente se muestra inglés.",
    },
    models: {
        discoveredCount_one: "Se descubrió {{count}} modelo.",
        discoveredCount_many: "Se descubrieron {{count}} modelos.",
        discoveredCount_other: "Se descubrieron {{count}} modelos.",
    },
} as const;
export default spanish;
