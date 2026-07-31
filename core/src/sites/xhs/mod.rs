pub mod entities;
pub mod history;
mod media_manifest;
pub mod page;
mod page_diagnostics;
pub mod tools;

pub use self::entities::{parse_count_text, XhsAuthorProfile, XhsNote, XhsNoteCard};
pub use self::history::{HistoryEntry, HistorySnapshot, XhsHistoryStore};
pub use self::page::{ReadNoteOptions, XhsPageRuntime, XHS_HOME_URL};
pub use self::tools::{
    author_scan_command, close_open_note, ensure_search_ready, search_command,
    xhs_agent_instructions, xhs_agent_tools, xhs_default_agent_tools,
    xhs_macro_tools_with_llm_provider, xhs_tools, xhs_tools_with_llm_provider, XHS_KNOWLEDGE,
    XHS_SITE,
};
