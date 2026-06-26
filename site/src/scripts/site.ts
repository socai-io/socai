// Unified client runtime for every page. It reads the page-local dictionary
// embedded in #site-i18n and drives the language toggle, all data-i18n* markers
// (text / html / aria-label / content / alt, with {value} interpolation), the
// clipboard copy buttons, and the hero typewriter. Each feature no-ops when its
// markup is absent, so one script serves the home, connect, and contact pages.

const i18nElement = document.getElementById("site-i18n");
const dictionary = JSON.parse(i18nElement?.textContent || "{}");
const languageKey = "socai-language";
const languageOptions = Array.from(
    document.querySelectorAll("[data-lang-option]"),
);
const supportedLanguages = Object.keys(dictionary);
const typewriter = document.querySelector("[data-typewriter]");
const prefersReducedMotion = window.matchMedia(
    "(prefers-reduced-motion: reduce)",
).matches;
let typewriterRun = 0;

const isSupportedLanguage = (language) => supportedLanguages.includes(language);

const getMessage = (language, path) => {
    const table = dictionary[language] || dictionary.zh || dictionary.en || {};
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

const sleep = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));

const renderTypewriter = (chars, runId) => {
    if (runId !== typewriterRun || !(typewriter instanceof HTMLElement)) {
        return false;
    }

    typewriter.textContent = chars.join("");
    return true;
};

const startTypewriter = (phrases) => {
    typewriterRun += 1;
    const runId = typewriterRun;

    if (!(typewriter instanceof HTMLElement)) {
        return;
    }

    const nextPhrases = Array.isArray(phrases)
        ? phrases.filter(
              (phrase) => typeof phrase === "string" && phrase.length > 0,
          )
        : [];
    typewriter.dataset.phrases = JSON.stringify(nextPhrases);
    typewriter.textContent = nextPhrases[0] || "";

    if (prefersReducedMotion || nextPhrases.length < 2) {
        return;
    }

    const run = async () => {
        let phraseIndex = 0;
        await sleep(2200);

        while (runId === typewriterRun) {
            const currentPhrase = Array.from(nextPhrases[phraseIndex]);

            for (let i = currentPhrase.length; i >= 0; i -= 1) {
                if (!renderTypewriter(currentPhrase.slice(0, i), runId)) {
                    return;
                }
                await sleep(20);
            }

            phraseIndex = (phraseIndex + 1) % nextPhrases.length;
            const nextPhrase = Array.from(nextPhrases[phraseIndex]);
            await sleep(280);

            for (let i = 1; i <= nextPhrase.length; i += 1) {
                if (!renderTypewriter(nextPhrase.slice(0, i), runId)) {
                    return;
                }
                await sleep(44 + Math.random() * 26);
            }

            await sleep(2600);
        }
    };

    run();
};

const chooseInitialLanguage = () => {
    try {
        const storedLanguage = window.localStorage.getItem(languageKey);
        if (storedLanguage && isSupportedLanguage(storedLanguage)) {
            return storedLanguage;
        }
    } catch {
        // Ignore storage errors and fall back to the default language.
    }

    return "zh";
};

const applyLanguage = (language, shouldPersist = false) => {
    const nextLanguage = isSupportedLanguage(language) ? language : "zh";
    const htmlLanguage = nextLanguage === "zh" ? "zh-CN" : "en";

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

    // Blog index: show only the posts written in the current language.
    document.querySelectorAll("[data-post-lang]").forEach((element) => {
        (element as HTMLElement).hidden =
            element.getAttribute("data-post-lang") !== nextLanguage;
    });

    startTypewriter(
        dictionary[nextLanguage]?.prompts || dictionary.zh?.prompts || [],
    );

    if (shouldPersist) {
        try {
            window.localStorage.setItem(languageKey, nextLanguage);
        } catch {
            // Ignore storage errors; the toggle should still update the page.
        }
    }
};

languageOptions.forEach((option) => {
    option.addEventListener("click", () => {
        applyLanguage(option.getAttribute("data-lang-option") || "zh", true);
    });
});

// chrome:// addresses can't be opened from a link, so the address is copied to
// the clipboard for the user to paste instead.
document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", () => {
        const text = button.getAttribute("data-copy") || "";
        const restore = () => {
            const language = document.documentElement.dataset.language || "zh";
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

applyLanguage(chooseInitialLanguage());
