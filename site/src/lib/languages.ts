export const LANGUAGE_OPTIONS = [
    { value: "zh", label: "简体中文" },
    { value: "en", label: "English" },
    { value: "ja", label: "日本語" },
    { value: "ko", label: "한국어" },
    { value: "es", label: "Español" },
    { value: "fr", label: "Français" },
    { value: "de", label: "Deutsch" },
    { value: "pt", label: "Português" },
] as const;

export const LANGUAGE_CODES = LANGUAGE_OPTIONS.map(({ value }) => value);

export const normalizeLanguage = (language: string | null | undefined) =>
    typeof language === "string" ? language.toLowerCase().split("-")[0] : "";

export function choosePreferredLanguage({
    search = "",
    storedLanguage,
    browserLanguages = [],
    fallback = "en",
}: {
    search?: string;
    storedLanguage?: string | null;
    browserLanguages?: readonly string[];
    fallback?: string;
}) {
    const candidates = [
        new URLSearchParams(search).get("lang"),
        storedLanguage,
        ...browserLanguages,
    ];

    return (
        candidates
            .map(normalizeLanguage)
            .find((language) => LANGUAGE_CODES.some((code) => code === language)) ||
        fallback
    );
}

export function resolvePageLanguage(
    preferredLanguage: string,
    supportedLanguages: readonly string[],
    fallback = "en",
) {
    if (supportedLanguages.includes(preferredLanguage)) {
        return preferredLanguage;
    }
    if (supportedLanguages.includes(fallback)) {
        return fallback;
    }
    return supportedLanguages[0] || fallback;
}

export function withLanguage(url: string, language: string, base?: string) {
    const nextUrl = new URL(url, base);
    nextUrl.searchParams.set("lang", language);
    return nextUrl;
}
