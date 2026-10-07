// Unified client runtime for every page. It reads the page-local dictionary
// embedded in #site-i18n and drives the language toggle, all data-i18n* markers
// (text / html / aria-label / content / alt, with {value} interpolation), the
// clipboard copy buttons. Each feature no-ops when its
// markup is absent, so one script serves the home, connect, and contact pages.

import { track } from "@vercel/analytics";
import {
    choosePreferredLanguage,
    resolvePageLanguage,
    withLanguage,
} from "../lib/languages";

const i18nElement = document.getElementById("site-i18n");
const dictionary = JSON.parse(i18nElement?.textContent || "{}");
const languageKey = "socai-language";
const languageOptions = Array.from(
    document.querySelectorAll("[data-lang-option]"),
);
const languageSelect = document.querySelector("[data-language-select]");
const supportedLanguages = Object.keys(dictionary);
const isSupportedLanguage = (language) => supportedLanguages.includes(language);

const getMessage = (language, path) => {
    const table = dictionary[language] || dictionary.en || dictionary.zh || {};
    const value = path
        .split(".")
        .reduce((cursor, key) => cursor?.[key], table);
    return typeof value === "string" ? value : "";
};

const getValues = (element) => {
    const value = element.getAttribute("data-i18n-values");

    if (!value) {
        return {};
    }

    try {
        return JSON.parse(value);
    } catch {
        return {};
    }
};

const interpolate = (value, replacements = {}) =>
    value.replace(/\{(\w+)\}/g, (_, key) => replacements[key] ?? "");

const chooseInitialLanguage = () => {
    let storedLanguage;
    try {
        storedLanguage = window.localStorage.getItem(languageKey);
    } catch {
        // Ignore storage errors and continue with URL or browser preferences.
    }

    return choosePreferredLanguage({
        search: window.location.search,
        storedLanguage,
        browserLanguages: [...(navigator.languages || []), navigator.language],
    });
};

const updateLanguageLinks = (language) => {
    document.querySelectorAll("[data-language-link]").forEach((link) => {
        if (!(link instanceof HTMLAnchorElement)) {
            return;
        }

        const url = withLanguage(link.href, language, window.location.href);
        link.href = url.toString();
    });
};

const updateCurrentLanguageParameter = (language) => {
    const url = withLanguage(window.location.href, language);
    window.history.replaceState(
        window.history.state,
        "",
        `${url.pathname}${url.search}${url.hash}`,
    );
};

const persistLanguage = (language) => {
    try {
        window.localStorage.setItem(languageKey, language);
    } catch {
        // Ignore storage errors; the active page can still switch languages.
    }
};

const applyLanguage = (language, shouldPersist = false) => {
    const nextLanguage = isSupportedLanguage(language) ? language : "en";
    const htmlLanguage = nextLanguage === "zh" ? "zh-CN" : nextLanguage;

    document.documentElement.lang = htmlLanguage;
    document.documentElement.dataset.language = nextLanguage;
    document.title = getMessage(nextLanguage, "meta.title");

    document.querySelectorAll("[data-i18n]").forEach((element) => {
        const path = element.getAttribute("data-i18n");
        const value = path ? getMessage(nextLanguage, path) : "";

        if (value) {
            element.textContent = interpolate(value, getValues(element));
        }
    });

    document.querySelectorAll("[data-i18n-html]").forEach((element) => {
        const path = element.getAttribute("data-i18n-html");
        const value = path ? getMessage(nextLanguage, path) : "";

        if (value) {
            element.innerHTML = value;
        }
    });

    [
        ["data-i18n-aria-label", "aria-label"],
        ["data-i18n-content", "content"],
        ["data-i18n-alt", "alt"],
        ["data-i18n-placeholder", "placeholder"],
        ["data-i18n-title", "title"],
    ].forEach(([marker, attribute]) => {
        document.querySelectorAll(`[${marker}]`).forEach((element) => {
            const path = element.getAttribute(marker);
            const value = path ? getMessage(nextLanguage, path) : "";

            if (value) {
                element.setAttribute(
                    attribute,
                    interpolate(value, getValues(element)),
                );
            }
        });
    });

    languageOptions.forEach((option) => {
        const isActive =
            option.getAttribute("data-lang-option") === nextLanguage;
        option.setAttribute("aria-pressed", String(isActive));
    });
    if (languageSelect instanceof HTMLSelectElement) {
        languageSelect.value = nextLanguage;
    }
    updateLanguageLinks(nextLanguage);

    // Blog index: show only the posts written in the current language.
    document.querySelectorAll("[data-post-lang]").forEach((element) => {
        (element as HTMLElement).hidden =
            element.getAttribute("data-post-lang") !== nextLanguage;
    });


    if (shouldPersist) {
        persistLanguage(nextLanguage);
    }
};

languageOptions.forEach((option) => {
    option.addEventListener("click", () => {
        applyLanguage(option.getAttribute("data-lang-option") || "en", true);
    });
});

languageSelect?.addEventListener("change", () => {
    if (languageSelect instanceof HTMLSelectElement) {
        updateCurrentLanguageParameter(languageSelect.value);
        applyLanguage(languageSelect.value, true);
    }
});

document.querySelectorAll("[data-download-platform]").forEach((link) => {
    link.addEventListener("click", () => {
        track("download_click", {
            platform:
                link.getAttribute("data-download-platform") || "unknown",
            language: document.documentElement.dataset.language || "en",
        });
    });
});

// chrome:// addresses can't be opened from a link, so the address is copied to
// the clipboard for the user to paste instead.
document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", () => {
        const text = button.getAttribute("data-copy") || "";
        const restore = () => {
            const language = document.documentElement.dataset.language || "en";
            const copiedPath = button.getAttribute("data-i18n-copied");
            const labelPath = button.getAttribute("data-i18n");

            button.classList.add("is-copied");
            if (copiedPath) {
                button.textContent = getMessage(language, copiedPath);
            }
            window.setTimeout(() => {
                button.classList.remove("is-copied");
                if (labelPath) {
                    button.textContent = getMessage(language, labelPath);
                }
            }, 1600);
        };

        if (navigator.clipboard?.writeText) {
            navigator.clipboard.writeText(text).then(restore).catch(restore);
        } else {
            restore();
        }
    });
});

const preferredLanguage = chooseInitialLanguage();
persistLanguage(preferredLanguage);
applyLanguage(resolvePageLanguage(preferredLanguage, supportedLanguages));
