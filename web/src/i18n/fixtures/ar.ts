/** Representative Arabic test/preview resources; never imported by production. */
const arabic = {
    common: {
        save: "حفظ التغييرات",
        cancel: "إلغاء",
        retry: "إعادة المحاولة",
        language: "اللغة",
    },
    models: {
        discoveredCount_zero: "لم يُكتشف أي نموذج ({{count}}).",
        discoveredCount_one: "اكتُشف نموذج واحد ({{count}}).",
        discoveredCount_two: "اكتُشف نموذجان ({{count}}).",
        discoveredCount_few: "اكتُشفت {{count}} نماذج.",
        discoveredCount_many: "اكتُشف {{count}} نموذجًا.",
        discoveredCount_other: "اكتُشف {{count}} نموذج.",
    },
    rich: {
        connected: "متصل بـ [[baseUrl]] — يُحفظ الرمز في هذا المتصفح فقط.",
    },
    preview: {
        connect: "الاتصال بـ JanusGate",
        address: "عنوان البوابة",
        token: "رمز الإدارة",
        overview: "نظرة عامة",
        providers: "حسابات المنبع",
        models: "النماذج",
        keys: "مفاتيح API",
        usage: "الاستخدام",
        name: "الاسم",
        requiredName: "لا يمكن أن يكون الاسم فارغًا.",
        enabled: "مفعّل",
        delete: "حذف المفتاح",
        deleteQuestion: "هل تريد حذف هذا المفتاح نهائيًا؟",
        deleteConsequence:
            "يصبح المفتاح غير صالح فور الحذف. ستتلقى التطبيقات التي تستخدمه استجابة 401.",
        expired: "لم يتلق المتصفح استجابة يمكنه قراءتها.",
        reconnect: "تحقق من عنوان البوابة وإعدادات مشاركة الموارد بين الأصول.",
        requests: "الطلبات",
        input: "رموز الإدخال",
        output: "رموز الإخراج",
        day: "اليوم",
        theme: "المظهر",
        mixedName: "حساب العربية / 中文",
        draft: "مسودة غير محفوظة",
    },
} as const;
export default arabic;
