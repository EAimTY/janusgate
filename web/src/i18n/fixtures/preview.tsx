/** Development-only visual fixture. The production entry never imports this module. */
import { type JSX, render } from "@solidjs/web";
import { createSignal, For } from "solid-js";
import { BucketCell, BucketHeader, UsageTable } from "/src/features/usage/table";
import { BarChart } from "/src/ui/bar-chart";
import { CopyButton } from "/src/ui/clipboard";
import { Button, SelectField, Switch, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { InlineConfirmQuestion } from "/src/ui/inline-confirm-question";
import { Card, CardHeader } from "/src/ui/layout";
import english from "../catalogs/en";
import chinese from "../catalogs/zh-CN";
import { createLocalizationController } from "../controller";
import arabic from "./ar";
import "/src/tailwind.css";

/** Renders the fixture's flat [[slot]] markers — the same grammar rich.tsx renders. */
function previewRich(template: string, slots: Readonly<Record<string, JSX.Element>>): JSX.Element {
    return template
        .split(/(\[\[[A-Za-z][A-Za-z0-9]*\]\])/g)
        .map((part) => (part.startsWith("[[") ? slots[part.slice(2, -2)] : part));
}

function previewRichText(template: string, slots: Readonly<Record<string, string>>): string {
    return template
        .split(/(\[\[[A-Za-z][A-Za-z0-9]*\]\])/g)
        .map((part) => (part.startsWith("[[") ? (slots[part.slice(2, -2)] ?? "") : part))
        .join("");
}

const arabicPreview = {
    ...arabic,
    entityMessages: {
        deleteKey: "هل تريد حذف [[name]]؟ ستفقد التطبيقات التي تحمل هذا السر إمكانية الوصول.",
    },
    helpers: {
        clipboard: {
            copy: "نسخ",
            copied: "تم النسخ",
            copyToClipboard: "نسخ إلى الحافظة",
            theValueHere: "القيمة هنا",
        },
    },
    dynamicMessages: {
        refusedClipboard: "رفض المتصفح الحافظة — حدد {{subject}} وانسخها يدويًا.",
    },
    diagnosis: {
        noAnswerFromBaseUrl: "لم تصل استجابة من [[baseUrl]].",
    },
    linkedMessages: {
        chartPoint_zero: "[[day]]: لا طلبات ([[quantity]])",
        chartPoint_one: "[[day]]: طلب واحد ([[quantity]])",
        chartPoint_two: "[[day]]: طلبان ([[quantity]])",
        chartPoint_few: "[[day]]: [[quantity]] طلبات",
        chartPoint_many: "[[day]]: [[quantity]] طلبًا",
        chartPoint_other: "[[day]]: [[quantity]] طلب",
    },
};
const previewCopy = {
    en: {
        connect: "Connect to JanusGate",
        address: "Gateway address",
        token: "Management token",
        overview: "Overview",
        providers: "Providers",
        models: "Models",
        keys: "API keys",
        usage: "Usage",
        name: "Name",
        requiredName: "Name cannot be empty.",
        enabled: "Enabled",
        delete: "Delete this key",
        expired: "The browser received no answer it could read.",
        reconnect: "Check the address and the gateway's configuration.",
        requests: "Requests",
        input: "Input tokens",
        output: "Output tokens",
        day: "Day",
        theme: "Theme",
        draft: "Unsaved changes",
    },
    "zh-CN": {
        connect: "连接 JanusGate",
        address: "网关地址",
        token: "管理令牌",
        overview: "概览",
        providers: "上游账户",
        models: "模型",
        keys: "API 密钥",
        usage: "使用情况",
        name: "名称",
        requiredName: "名称不能为空。",
        enabled: "已启用",
        delete: "删除此密钥",
        expired: "浏览器未收到可读取的响应。",
        reconnect: "检查地址及网关配置。",
        requests: "请求数",
        input: "输入 token",
        output: "输出 token",
        day: "日期",
        theme: "外观",
        draft: "未保存的更改",
    },
    ar: arabic.preview,
} as const;
const parameters = new URLSearchParams(location.search);
const registry = {
    en: {
        autonym: "English",
        load: async () => english,
    },
    "zh-CN": {
        autonym: "简体中文",
        load: async () => chinese,
    },
    ar: {
        autonym: "العربية",
        load: async () => arabicPreview,
    },
};
const controller = createLocalizationController({
    registry,
    defaults: {
        en: "en",
        zh: "zh-CN",
        ar: "ar",
    },
    readPreference: () => parameters.get("locale"),
    writePreference: () => {},
    browserLanguages: () => [
        "en",
    ],
    eventTarget: window,
    root: document.documentElement,
    deadlineMs: 15000,
});
document.documentElement.classList.toggle("dark", parameters.get("theme") === "dark");
await controller.initialize();
const root = document.getElementById("root");
if (root === null) throw new Error("Missing preview root");
const pointValues = [
    0,
    1,
    2,
    3,
    11,
    100,
];
const name = "العربية / 中文 service-a [[name]]";
const model = "gpt-5/العربية-01";
const url = "https://gateway.example.com:8443";
function Preview(): JSX.Element {
    const copy = () => previewCopy[controller.activeLocale()];
    const t = controller.translation();
    const number = (value: number): string =>
        new Intl.NumberFormat(controller.activeLocale()).format(value);
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [draft, setDraft] = createSignal("");
    return (
        <>
            <header class="border-b border-rule bg-surface">
                <div class="mx-auto flex max-w-6xl flex-wrap items-center gap-4 px-5 py-4">
                    <strong class="text-[17px]">JanusGate</strong>
                    <nav class="flex flex-wrap gap-3 text-[13px]">
                        <For
                            each={[
                                copy().overview,
                                copy().providers,
                                copy().models,
                                copy().keys,
                                copy().usage,
                            ]}
                        >
                            {(label) => (
                                <a
                                    href="#content"
                                    class="inline-flex items-center gap-1"
                                >
                                    <Glyph name="chev-right" />
                                    {label}
                                </a>
                            )}
                        </For>
                    </nav>
                    <div class="ms-auto min-w-44">
                        <SelectField
                            label={controller.translation()(($) => $.common.language)}
                            value={controller.activeLocale()}
                            options={Object.entries(registry).map(([value, entry]) => ({
                                value,
                                label: entry.autonym,
                            }))}
                            onChange={(value) => {
                                if (value === "en" || value === "zh-CN" || value === "ar")
                                    void controller.setPreference(value);
                            }}
                        />
                    </div>
                    <Button onClick={() => document.documentElement.classList.toggle("dark")}>
                        {copy().theme}
                    </Button>
                </div>
            </header>
            <main
                id="content"
                class="mx-auto grid max-w-6xl gap-5 px-5 py-6 md:grid-cols-2"
            >
                <Card>
                    <CardHeader title={copy().connect} />
                    <div class="mt-4 flex flex-col gap-4">
                        <TextField
                            label={copy().address}
                            value={url}
                            onInput={() => {}}
                        />
                        <TextField
                            label={copy().token}
                            value="mgmt-token_ASCII-001"
                            onInput={() => {}}
                            type="password"
                        />
                        <p class="text-[13px] text-ink-dim">
                            {previewRich(
                                t(($) => $.rich.connected),
                                {
                                    baseUrl: (
                                        <bdi
                                            dir="ltr"
                                            class="tnum break-all"
                                        >
                                            {url}
                                        </bdi>
                                    ),
                                },
                            )}
                        </p>
                        <CopyButton
                            translator={controller.translation()}
                            value={url}
                            label={
                                controller.activeLocale() === "ar"
                                    ? "نسخ العنوان"
                                    : controller.translation()(($) => $.helpers.clipboard.copy)
                            }
                            ariaLabel={copy().address}
                        />
                    </div>
                </Card>
                <Card>
                    <CardHeader title={<bdi dir="auto">{name}</bdi>} />
                    <div class="mt-4 flex flex-col gap-4">
                        <TextField
                            label={copy().name}
                            value={draft()}
                            isData={false}
                            onInput={setDraft}
                            error={draft() === "" ? copy().requiredName : null}
                        />
                        <Switch
                            label={copy().enabled}
                            isOn={isEnabled()}
                            onToggle={setIsEnabled}
                        />
                        <div class="flex flex-wrap gap-2">
                            <Button tone="primary">
                                {controller.translation()(($) => $.common.save)}
                            </Button>
                            <span class="text-warn">{copy().draft}</span>
                        </div>
                        <Notice
                            tone="bad"
                            title={previewRich(
                                t(($) => $.diagnosis.noAnswerFromBaseUrl),
                                {
                                    baseUrl: <bdi dir="ltr">{url}</bdi>,
                                },
                            )}
                            detail={copy().expired}
                            rawDetail="Failed to fetch <script> [[baseUrl]]"
                            fix={copy().reconnect}
                        />
                    </div>
                </Card>
                <Card>
                    <CardHeader title={copy().delete} />
                    <div class="mt-4 flex flex-col gap-3">
                        <InlineConfirmQuestion
                            question={previewRich(
                                t(($) => $.entityMessages.deleteKey),
                                {
                                    name: <bdi dir="auto">{name}</bdi>,
                                },
                            )}
                            focusArmed={() => {}}
                        />
                        <div class="flex flex-wrap gap-2">
                            <Button tone="danger">{copy().delete}</Button>
                            <Button>{controller.translation()(($) => $.common.cancel)}</Button>
                        </div>
                        <p class="text-ink-dim">
                            <bdi dir="ltr">{model}</bdi> · <bdi dir="auto">{name}</bdi>
                        </p>
                    </div>
                </Card>
                <Card>
                    <CardHeader title={copy().usage} />
                    <div class="mt-4">
                        <BarChart
                            points={pointValues.map((value, index) => ({
                                label: `2026-10-0${index + 1}`,
                                value,
                            }))}
                            describePoint={(point) => {
                                const template = t(($) => $.linkedMessages.chartPoint, {
                                    count: point.value,
                                });
                                return {
                                    accessibleName: previewRichText(template, {
                                        day: point.label,
                                        quantity: number(point.value),
                                    }),
                                    content: previewRich(template, {
                                        day: (
                                            <bdi
                                                dir="ltr"
                                                class="whitespace-nowrap"
                                            >
                                                {point.label}
                                            </bdi>
                                        ),
                                        quantity: <bdi dir="ltr">{number(point.value)}</bdi>,
                                    }),
                                };
                            }}
                            summary={copy().requests}
                        />
                    </div>
                    <UsageTable
                        minWidthClass="min-w-[480px]"
                        class="mt-4"
                    >
                        <thead>
                            <tr>
                                <BucketHeader label={copy().day} />
                                <BucketHeader
                                    label={copy().requests}
                                    isNumeric
                                />
                                <BucketHeader
                                    label={copy().input}
                                    isNumeric
                                />
                                <BucketHeader
                                    label={copy().output}
                                    isNumeric
                                />
                            </tr>
                        </thead>
                        <tbody>
                            <For each={pointValues}>
                                {(value, index) => (
                                    <tr class="border-t border-rule">
                                        <td class="px-3 py-2">
                                            <bdi dir="ltr">2026-10-0{index() + 1}</bdi>
                                        </td>
                                        <BucketCell value={number(value)} />
                                        <BucketCell value={number(value * 1234)} />
                                        <BucketCell
                                            value={value === 0 ? "—" : number(value * 123)}
                                        />
                                    </tr>
                                )}
                            </For>
                        </tbody>
                    </UsageTable>
                </Card>
            </main>
        </>
    );
}
render(() => <Preview />, root);
window.addEventListener("pagehide", controller.dispose, {
    once: true,
});
