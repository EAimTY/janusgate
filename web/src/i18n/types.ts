import type english from "./catalogs/en";
import "i18next";

declare module "i18next" {
    interface CustomTypeOptions {
        defaultNS: "translation";
        resources: {
            translation: typeof english;
        };
        enableSelector: true;
        strictKeyChecks: true;
        returnNull: false;
        returnObjects: false;
    }
}

/** A catalog is immutable authored data, validated only by the development gate. */
export type Catalog = {
    readonly [key: string]: string | Catalog;
};
export type LocaleEntry = {
    readonly autonym: string;
    readonly load: () => Promise<Catalog>;
};
