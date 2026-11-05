//! Small cross-platform methods. These share the canonical on-demand registry;
//! they do not register tools, enable platforms, or change authorization.
use super::social::SocialSkill;

pub(super) const SKILLS: &[SocialSkill] = &[
    SocialSkill {
        name: "file-analysis",
        description: "Read and compare supplied documents, images and tables; answer from the actual files with clear coverage and references.",
        document: include_str!("file-analysis/SKILL.md"),
    },
    SocialSkill {
        name: "writing",
        description: "Turn a short instruction, supplied material or selected topic into a useful draft, rewrite or summary without unnecessary setup questions.",
        document: include_str!("writing/SKILL.md"),
    },
    SocialSkill {
        name: "result-delivery",
        description: "Organize an answer or requested downloadable document/table from real task results, preserving source identity and honest completeness.",
        document: include_str!("result-delivery/SKILL.md"),
    },
];

pub(super) fn catalog_prompt() -> String {
    let mut prompt = String::from("\n\nCommon methods are already available by default; no installation or user configuration is needed:\n");
    for skill in SKILLS {
        prompt.push_str(&format!("- `{}` — {}\n", skill.name, skill.description));
    }
    prompt.push_str("For questions about supplied files, load `file-analysis` before interpreting them. For drafting, rewriting or summarizing, load `writing`. For a requested downloadable file or a final multi-source report/table, load `result-delivery`. Load only the relevant methods, once per run unless their details are needed again. A short request is enough: infer ordinary defaults, use current-conversation context, and ask only for a missing choice that materially changes the result. Platform-specific collection methods still apply when new social evidence is needed; supplied-file tasks do not require browsing or research mode. Use only tools actually exposed in this run, within their schemas and the user's authorized scope. Do not promise file parsing, OCR, export, scheduling or sending merely because a method is available.\n");
    prompt
}
